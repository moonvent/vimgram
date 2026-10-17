//! Local archive of every message the client sees, so messages deleted
//! later can still be shown, marked as deleted.
//!
//! Messages are kept whole (as JSON), and every media file the client
//! downloads is hard-linked into `media/`: TDLib deletes the files of
//! deleted messages, the links keep them. Links of messages that were not
//! deleted within `MEDIA_DAYS` are dropped again.
//!
//! Writes go through a background thread and are batched into one
//! transaction; reads open their own connection (WAL allows both at once).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use rusqlite::{Connection, params};

use crate::tg::{Msg, Sender};

/// What the archive keeps of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub chat_id: i64,
    pub id: i64,
    pub sender: Sender,
    pub outgoing: bool,
    pub date: i32,
    /// Text, or a description of the media.
    pub text: String,
    /// Unix time the deletion was seen, if deleted.
    pub deleted_at: Option<i64>,
    /// The whole message as JSON (`Msg`); absent in old records.
    pub data: Option<String>,
    /// Archived copies of the media files: (file id, path).
    pub files: Vec<(i32, String)>,
}

impl Record {
    pub fn of(msg: &Msg) -> Self {
        Record {
            chat_id: msg.chat_id,
            id: msg.id,
            sender: msg.sender,
            outgoing: msg.outgoing,
            date: msg.date,
            text: msg.full_summary(),
            deleted_at: None,
            data: serde_json::to_string(msg).ok(),
            files: Vec::new(),
        }
    }
}

enum Command {
    Save(Vec<Record>),
    Delete { chat_id: i64, ids: Vec<i64>, at: i64 },
    /// Keep a downloaded file of a message.
    Link { chat_id: i64, msg_id: i64, file_id: i32, path: String },
    /// Reply once everything before it is committed.
    Flush(mpsc::Sender<()>),
}

#[derive(Clone)]
pub struct Archive {
    path: PathBuf,
    writer: mpsc::Sender<Command>,
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS messages (
            chat_id INTEGER NOT NULL,
            id INTEGER NOT NULL,
            sender_is_chat INTEGER NOT NULL,
            sender_id INTEGER NOT NULL,
            outgoing INTEGER NOT NULL,
            date INTEGER NOT NULL,
            text TEXT NOT NULL,
            deleted_at INTEGER,
            PRIMARY KEY (chat_id, id)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS files (
            chat_id INTEGER NOT NULL,
            msg_id INTEGER NOT NULL,
            file_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            linked_at INTEGER NOT NULL,
            PRIMARY KEY (chat_id, msg_id, file_id)
        ) WITHOUT ROWID;",
    )?;
    let has_data = conn
        .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'data'")?
        .exists([])?;
    if !has_data {
        conn.execute_batch("ALTER TABLE messages ADD COLUMN data TEXT")?;
    }
    Ok(conn)
}

impl Archive {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut conn = connect(&path).map_err(|e| e.to_string())?;
        let media = path.with_file_name("media");
        let (writer, commands) = mpsc::channel();
        let dir = media;
        std::thread::Builder::new()
            .name("archive".into())
            .spawn(move || {
                let _ = prune_media(&conn, MEDIA_DAYS);
                write_loop(&mut conn, &dir, &commands);
            })
            .map_err(|e| e.to_string())?;
        Ok(Archive { path, writer })
    }

    /// Keep a copy (hard link) of a downloaded media file of a message.
    pub fn link(&self, chat_id: i64, msg_id: i64, file_id: i32, path: String) {
        let _ = self.writer.send(Command::Link { chat_id, msg_id, file_id, path });
    }


    pub fn save(&self, msgs: &[Msg]) {
        if !msgs.is_empty() {
            let _ = self.writer.send(Command::Save(msgs.iter().map(Record::of).collect()));
        }
    }

    pub fn mark_deleted(&self, chat_id: i64, ids: Vec<i64>) {
        let at = chrono::Utc::now().timestamp();
        let _ = self.writer.send(Command::Delete { chat_id, ids, at });
    }

    /// Deleted messages of a chat with ids in `from..=to` (blocking).
    pub fn deleted_between(&self, chat_id: i64, from: i64, to: i64) -> Result<Vec<Record>, String> {
        let conn = connect(&self.path).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT chat_id, id, sender_is_chat, sender_id, outgoing, date, text, deleted_at, data
                 FROM messages
                 WHERE chat_id = ?1 AND id BETWEEN ?2 AND ?3 AND deleted_at IS NOT NULL
                 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![chat_id, from, to], |row| {
                let sender_id: i64 = row.get(3)?;
                Ok(Record {
                    chat_id: row.get(0)?,
                    id: row.get(1)?,
                    sender: if row.get::<_, bool>(2)? {
                        Sender::Chat(sender_id)
                    } else {
                        Sender::User(sender_id)
                    },
                    outgoing: row.get(4)?,
                    date: row.get(5)?,
                    text: row.get(6)?,
                    deleted_at: row.get(7)?,
                    data: row.get(8)?,
                    files: Vec::new(),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut records: Vec<Record> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        let mut files = conn
            .prepare("SELECT file_id, path FROM files WHERE chat_id = ?1 AND msg_id = ?2")
            .map_err(|e| e.to_string())?;
        for record in &mut records {
            record.files = files
                .query_map(params![chat_id, record.id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .filter(|(_, path): &(i32, String)| Path::new(path).exists())
                .collect();
        }
        Ok(records)
    }

    /// Wait until everything sent so far is written.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.writer.send(Command::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(5));
        }
    }
}

/// Linked files of messages that were not deleted are dropped after this.
const MEDIA_DAYS: i64 = 30;

/// Remove links to media of messages still alive after `days`.
fn prune_media(conn: &Connection, days: i64) -> rusqlite::Result<usize> {
    let cutoff = chrono::Utc::now().timestamp() - days * 86_400;
    let mut stmt = conn.prepare(
        "SELECT f.chat_id, f.msg_id, f.file_id, f.path FROM files f
         LEFT JOIN messages m ON m.chat_id = f.chat_id AND m.id = f.msg_id
         WHERE f.linked_at < ?1 AND m.deleted_at IS NULL",
    )?;
    let stale: Vec<(i64, i64, i32, String)> = stmt
        .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .filter_map(Result::ok)
        .collect();
    for (chat_id, msg_id, file_id, path) in &stale {
        let _ = std::fs::remove_file(path);
        conn.execute(
            "DELETE FROM files WHERE chat_id = ?1 AND msg_id = ?2 AND file_id = ?3",
            params![chat_id, msg_id, file_id],
        )?;
    }
    Ok(stale.len())
}

/// Hard link (same file system, no extra space) or copy `src` into the
/// media directory.
fn keep_file(dir: &Path, chat_id: i64, msg_id: i64, file_id: i32, src: &str) -> std::io::Result<PathBuf> {
    let src = Path::new(src);
    let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let chat_dir = dir.join(chat_id.to_string());
    std::fs::create_dir_all(&chat_dir)?;
    let dest = chat_dir.join(format!("{msg_id}-{file_id}{ext}"));
    if !dest.exists() && std::fs::hard_link(src, &dest).is_err() {
        std::fs::copy(src, &dest)?;
    }
    Ok(dest)
}

fn write_loop(conn: &mut Connection, media: &Path, commands: &mpsc::Receiver<Command>) {
    while let Ok(first) = commands.recv() {
        // Batch whatever else is queued into the same transaction.
        let batch: Vec<Command> = std::iter::once(first).chain(commands.try_iter()).collect();
        let Ok(tx) = conn.transaction() else { continue };
        let mut flushed = Vec::new();
        for command in batch {
            match command {
                Command::Flush(done) => flushed.push(done),
                command => {
                    let _ = apply(&tx, media, command);
                }
            }
        }
        let _ = tx.commit();
        for done in flushed {
            let _ = done.send(());
        }
    }
}

fn apply(tx: &rusqlite::Transaction<'_>, media: &Path, command: Command) -> rusqlite::Result<()> {
    match command {
        Command::Save(records) => {
            // Updating keeps `deleted_at`: a message cannot come back.
            let mut stmt = tx.prepare_cached(
                "INSERT INTO messages (chat_id, id, sender_is_chat, sender_id, outgoing, date, text, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (chat_id, id) DO UPDATE
                 SET text = excluded.text, data = coalesce(excluded.data, data)",
            )?;
            for r in records {
                let (is_chat, sender_id) = match r.sender {
                    Sender::User(id) => (false, id),
                    Sender::Chat(id) => (true, id),
                };
                stmt.execute(params![r.chat_id, r.id, is_chat, sender_id, r.outgoing, r.date, r.text, r.data])?;
            }
        }
        Command::Flush(_) => {}
        Command::Link { chat_id, msg_id, file_id, path } => {
            let exists: bool = tx
                .prepare_cached("SELECT 1 FROM files WHERE chat_id = ?1 AND msg_id = ?2 AND file_id = ?3")?
                .exists(params![chat_id, msg_id, file_id])?;
            if !exists && let Ok(kept) = keep_file(media, chat_id, msg_id, file_id, &path) {
                tx.prepare_cached(
                    "INSERT OR REPLACE INTO files (chat_id, msg_id, file_id, path, linked_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?
                .execute(params![chat_id, msg_id, file_id, kept.to_string_lossy(), chrono::Utc::now().timestamp()])?;
            }
        }
        Command::Delete { chat_id, ids, at } => {
            let mut stmt = tx.prepare_cached(
                "UPDATE messages SET deleted_at = ?3
                 WHERE chat_id = ?1 AND id = ?2 AND deleted_at IS NULL",
            )?;
            for id in ids {
                stmt.execute(params![chat_id, id, at])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(chat_id: i64, id: i64, text: &str) -> Msg {
        Msg {
            id,
            chat_id,
            sender: Sender::User(7),
            outgoing: false,
            date: 1_700_000_000,
            edited: false,
            text: text.into(),
            media: Vec::new(),
            album: 0,
            members: vec![id],
            reply_to: None,
            sending: false,
            scheduled: false,
            deleted: false,
            local_files: Vec::new(),
            reactions: Vec::new(),
            forward: None,
            links: Vec::new(),
            styles: Vec::new(),
            spoiler: false,
        }
    }

    #[test]
    fn keeps_deleted_messages() {
        let path = std::env::temp_dir().join(format!("vimgram-archive-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let archive = Archive::open(path.clone()).unwrap();
        archive.save(&[msg(1, 10, "hello"), msg(1, 11, "secret"), msg(2, 11, "other chat")]);
        archive.mark_deleted(1, vec![11]);
        // Seeing the message again (e.g. from cache) must not undelete it.
        archive.save(&[msg(1, 11, "secret, edited")]);
        archive.flush();

        let deleted = archive.deleted_between(1, 0, 100).unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].id, 11);
        assert_eq!(deleted[0].text, "secret, edited");
        assert!(deleted[0].deleted_at.is_some());
        assert!(archive.deleted_between(2, 0, 100).unwrap().is_empty());
        assert!(archive.deleted_between(1, 0, 10).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn keeps_media_of_deleted_messages() {
        let dir = std::env::temp_dir().join(format!("vimgram-archive-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = Archive::open(dir.join("archive.sqlite")).unwrap();

        // A voice message whose file TDLib downloaded…
        let tdlib_file = dir.join("voice.oga");
        std::fs::write(&tdlib_file, b"OggS").unwrap();
        let mut voice = msg(1, 20, "");
        voice.media = vec![crate::tg::Media::Voice { file: 77, duration: 3, waveform: vec![1, 2, 3], listened: false }];
        archive.save(&[voice]);
        archive.link(1, 20, 77, tdlib_file.to_string_lossy().into_owned());
        archive.mark_deleted(1, vec![20]);
        archive.flush();
        // …and then deleted it along with the message.
        std::fs::remove_file(&tdlib_file).unwrap();

        let records = archive.deleted_between(1, 0, 100).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].files.len(), 1);
        let restored = Msg::from(records[0].clone());
        assert!(restored.deleted);
        let crate::tg::Media::Voice { file, duration, waveform, .. } = &restored.media[0] else {
            panic!("voice expected, got {:?}", restored.media);
        };
        assert_eq!((*duration, waveform.as_slice()), (3, &[1u8, 2, 3][..]));
        assert!(*file < 0, "points at the archive's copy");
        let path = futures::executor::block_on(crate::tg::download(*file, 1)).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"OggS");

        // Links of messages that are still alive get pruned eventually.
        archive.save(&[msg(1, 21, "alive")]);
        let other = dir.join("photo.jpg");
        std::fs::write(&other, b"jpeg").unwrap();
        archive.link(1, 21, 78, other.to_string_lossy().into_owned());
        archive.flush();
        let conn = connect(&dir.join("archive.sqlite")).unwrap();
        assert_eq!(prune_media(&conn, -1).unwrap(), 1, "only the live message's file");
        assert_eq!(archive.deleted_between(1, 0, 100).unwrap()[0].files.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
