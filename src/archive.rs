//! Local archive of every message the client sees, so messages deleted
//! later can still be shown, marked as deleted.
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
}

impl Record {
    pub fn of(msg: &Msg) -> Self {
        Record {
            chat_id: msg.chat_id,
            id: msg.id,
            sender: msg.sender,
            outgoing: msg.outgoing,
            date: msg.date,
            text: msg.summary(),
            deleted_at: None,
        }
    }
}

enum Command {
    Save(Vec<Record>),
    Delete { chat_id: i64, ids: Vec<i64>, at: i64 },
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
        ) WITHOUT ROWID;",
    )?;
    Ok(conn)
}

impl Archive {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut conn = connect(&path).map_err(|e| e.to_string())?;
        let (writer, commands) = mpsc::channel();
        std::thread::Builder::new()
            .name("archive".into())
            .spawn(move || write_loop(&mut conn, &commands))
            .map_err(|e| e.to_string())?;
        Ok(Archive { path, writer })
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
                "SELECT chat_id, id, sender_is_chat, sender_id, outgoing, date, text, deleted_at
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
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Wait until everything sent so far is written.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.writer.send(Command::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(5));
        }
    }
}

fn write_loop(conn: &mut Connection, commands: &mpsc::Receiver<Command>) {
    while let Ok(first) = commands.recv() {
        // Batch whatever else is queued into the same transaction.
        let batch: Vec<Command> = std::iter::once(first).chain(commands.try_iter()).collect();
        let Ok(tx) = conn.transaction() else { continue };
        let mut flushed = Vec::new();
        for command in batch {
            match command {
                Command::Flush(done) => flushed.push(done),
                command => {
                    let _ = apply(&tx, command);
                }
            }
        }
        let _ = tx.commit();
        for done in flushed {
            let _ = done.send(());
        }
    }
}

fn apply(tx: &rusqlite::Transaction<'_>, command: Command) -> rusqlite::Result<()> {
    match command {
        Command::Save(records) => {
            // Updating keeps `deleted_at`: a message cannot come back.
            let mut stmt = tx.prepare_cached(
                "INSERT INTO messages (chat_id, id, sender_is_chat, sender_id, outgoing, date, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (chat_id, id) DO UPDATE SET text = excluded.text",
            )?;
            for r in records {
                let (is_chat, sender_id) = match r.sender {
                    Sender::User(id) => (false, id),
                    Sender::Chat(id) => (true, id),
                };
                stmt.execute(params![r.chat_id, r.id, is_chat, sender_id, r.outgoing, r.date, r.text])?;
            }
        }
        Command::Flush(_) => {}
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
}
