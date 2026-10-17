//! Bridge between TDLib and the UI.
//!
//! `tdlib_rs::receive()` must be polled continuously: it delivers both updates
//! and the responses that resolve pending `functions::*` futures. It runs on a
//! dedicated thread and its updates are exposed as an iced subscription stream.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iced::futures::channel::mpsc;
use iced::futures::{Stream, StreamExt};
use iced::widget::image::Handle;
use serde::{Deserialize, Serialize};
use tdlib_rs::enums::{
    self, ChatList, InputMessageContent, MessageContent, MessageSender, StickerFormat,
    ThumbnailFormat, Update,
};
use tdlib_rs::{functions, types};

use crate::archive::Record;
use crate::config::{self, Config};
use crate::voice::Recording;

static CLIENT: OnceLock<i32> = OnceLock::new();

pub fn client() -> i32 {
    *CLIENT.get_or_init(tdlib_rs::create_client)
}

/// Stream of TDLib updates, delivered in batches so a burst at startup
/// (thousands of chats/users) costs one UI update per batch.
pub fn updates() -> impl Stream<Item = Vec<Update>> {
    let (tx, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name("tdlib-receive".into())
        .spawn(move || {
            while !tx.is_closed() {
                if let Some((update, _)) = tdlib_rs::receive() {
                    let _ = tx.unbounded_send(update);
                }
            }
        })
        .expect("failed to spawn TDLib receive thread");
    rx.ready_chunks(512)
}

type Res<T> = Result<T, String>;

fn err(e: types::Error) -> String {
    format!("{} ({})", e.message, e.code)
}

/// Any request starts update delivery for the client; this one also silences
/// TDLib's very chatty default logging.
pub async fn start() -> Res<()> {
    functions::set_log_verbosity_level(1, client()).await.map_err(err)
}

pub async fn set_parameters(cfg: Config) -> Res<()> {
    let dirs = config::dirs();
    let data = dirs.data_dir();
    functions::set_tdlib_parameters(
        false,
        data.join("db").to_string_lossy().into_owned(),
        data.join("files").to_string_lossy().into_owned(),
        String::new(),
        true,
        true,
        true,
        false,
        cfg.api_id,
        cfg.api_hash,
        "en".into(),
        "Desktop".into(),
        std::env::consts::OS.into(),
        env!("CARGO_PKG_VERSION").into(),
        client(),
    )
    .await
    .map_err(err)
}

pub async fn set_phone(phone: String) -> Res<()> {
    functions::set_authentication_phone_number(phone, None, client())
        .await
        .map_err(err)
}

pub async fn check_code(code: String) -> Res<()> {
    functions::check_authentication_code(code, client()).await.map_err(err)
}

pub async fn check_password(password: String) -> Res<()> {
    functions::check_authentication_password(password, client())
        .await
        .map_err(err)
}

pub async fn load_chats(list: ListKey, limit: i32) -> Res<()> {
    functions::load_chats(Some(list.to_tg()), limit, client())
        .await
        .map_err(err)
}

/// Download a file completely and return its local path. Already downloaded
/// files return immediately.
/// Files kept by the archive after Telegram deleted them. They get
/// negative ids, which never clash with TDLib's.
#[derive(Default)]
struct LocalFiles {
    paths: HashMap<i32, String>,
    ids: HashMap<String, i32>,
}

static LOCAL: OnceLock<Mutex<LocalFiles>> = OnceLock::new();
static NEXT_LOCAL: AtomicI32 = AtomicI32::new(-1);

fn local_files() -> std::sync::MutexGuard<'static, LocalFiles> {
    LOCAL.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// A file id for a local file (the same path always gets the same id).
pub fn local_file_id(path: &str) -> i32 {
    let mut files = local_files();
    if let Some(&id) = files.ids.get(path) {
        return id;
    }
    let id = NEXT_LOCAL.fetch_sub(1, Ordering::Relaxed);
    files.paths.insert(id, path.to_owned());
    files.ids.insert(path.to_owned(), id);
    id
}

/// A file id that refers to nothing (media the archive does not have).
pub fn missing_file_id() -> i32 {
    NEXT_LOCAL.fetch_sub(1, Ordering::Relaxed)
}

pub async fn download(file_id: i32, priority: i32) -> Res<String> {
    if file_id < 0 {
        return local_files()
            .paths
            .get(&file_id)
            .cloned()
            .ok_or_else(|| "the archive has no copy of this file".to_string());
    }
    let enums::File::File(file) = functions::download_file(file_id, priority, 0, 0, true, client())
        .await
        .map_err(err)?;
    if file.local.is_downloading_completed {
        Ok(file.local.path)
    } else {
        Err(format!("download of file {file_id} did not complete"))
    }
}

/// Where a file can be read from.
pub enum FileSource {
    /// Complete on disk: read it directly. Files TDLib didn't download
    /// itself (videos you sent) live outside its cache, and `read_file_part`
    /// refuses them.
    Local(String),
    /// Being downloaded: its size in bytes, exact or expected; 0 when unknown.
    Remote(u64),
}

pub async fn file_source(file_id: i32) -> Res<FileSource> {
    if file_id < 0 {
        return download(file_id, 0).await.map(FileSource::Local);
    }
    let enums::File::File(file) = functions::get_file(file_id, client()).await.map_err(err)?;
    if file.local.is_downloading_completed && std::path::Path::new(&file.local.path).is_file() {
        return Ok(FileSource::Local(file.local.path));
    }
    Ok(FileSource::Remote(file.size.max(file.expected_size).max(0) as u64))
}

/// Bytes already downloaded starting at `offset`.
pub async fn downloaded_prefix(file_id: i32, offset: i64) -> Res<i64> {
    let enums::FileDownloadedPrefixSize::FileDownloadedPrefixSize(prefix) =
        functions::get_file_downloaded_prefix_size(file_id, offset, client())
            .await
            .map_err(err)?;
    Ok(prefix.size)
}

/// Move the download of a file to `offset` and keep going from there.
pub async fn start_download(file_id: i32, offset: i64) -> Res<()> {
    functions::download_file(file_id, 32, offset, 0, false, client())
        .await
        .map(|_| ())
        .map_err(err)
}

/// Mark a voice or round video message as listened (for the sender too).
pub async fn open_content(chat_id: i64, message_id: i64) -> Res<()> {
    functions::open_message_content(chat_id, message_id, client()).await.map_err(err)
}

/// Delete cached files (photos, videos, voice, …) not opened for `days`
/// days; profile photos stay. Returns the bytes freed.
pub async fn clean_cache(days: i32) -> Res<i64> {
    let enums::StorageStatistics::StorageStatistics(deleted) = functions::optimize_storage(
        1 << 50, // no size limit, only age
        days.max(0) * 86_400,
        i32::MAX,
        if days == 0 { 0 } else { -1 },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        true,
        0,
        client(),
    )
    .await
    .map_err(err)?;
    Ok(deleted.size)
}

/// Download in the background at low priority, so the message's media
/// survives if it gets deleted.
pub async fn prefetch(file_id: i32) -> Res<()> {
    functions::download_file(file_id, 1, 0, 0, false, client())
        .await
        .map(|_| ())
        .map_err(err)
}

pub async fn read_part(file_id: i32, offset: i64, count: i64) -> Res<Vec<u8>> {
    use base64::Engine;
    let enums::Data::Data(data) = functions::read_file_part(file_id, offset, count, client())
        .await
        .map_err(err)?;
    base64::engine::general_purpose::STANDARD
        .decode(data.data)
        .map_err(|e| e.to_string())
}

pub async fn open_chat(chat_id: i64) -> Res<()> {
    functions::open_chat(chat_id, client()).await.map_err(err)
}

pub async fn close_chat(chat_id: i64) -> Res<()> {
    functions::close_chat(chat_id, client()).await.map_err(err)
}

pub async fn view_messages(chat_id: i64, ids: Vec<i64>) -> Res<()> {
    functions::view_messages(chat_id, ids, None, true, client())
        .await
        .map_err(err)
}

/// A page of history around `from_id` (the newest messages when it is 0),
/// returned oldest-first. A negative `offset` includes that many messages
/// newer than `from_id`.
pub async fn history(chat_id: i64, from_id: i64, offset: i32, limit: i32) -> Res<Vec<Msg>> {
    let enums::Messages::Messages(page) =
        functions::get_chat_history(chat_id, from_id, offset, limit, false, client())
            .await
            .map_err(err)?;
    let mut msgs: Vec<Msg> = page.messages.into_iter().flatten().map(Msg::from).collect();
    msgs.reverse();
    Ok(msgs)
}

/// Ids of messages matching `query`, newest first, and the total count.
pub async fn search(chat_id: i64, query: String) -> Res<(Vec<i64>, i32)> {
    let enums::FoundChatMessages::FoundChatMessages(found) =
        functions::search_chat_messages(chat_id, None, query, None, 0, 0, 100, None, client())
            .await
            .map_err(err)?;
    Ok((found.messages.iter().map(|m| m.id).collect(), found.total_count))
}

/// Messages with hashtag `tag` older than `from_id` (0: the newest ones),
/// oldest first.
pub async fn tagged(chat_id: i64, tag: String, from_id: i64, limit: i32) -> Res<Vec<Msg>> {
    let enums::FoundChatMessages::FoundChatMessages(found) =
        functions::search_chat_messages(chat_id, None, tag, None, from_id, 0, limit, None, client())
            .await
            .map_err(err)?;
    let mut msgs: Vec<Msg> = found.messages.into_iter().filter(|m| m.id != from_id).map(Msg::from).collect();
    msgs.reverse();
    Ok(msgs)
}

/// `text` has hashtag `tag` (with its `#`), in any case, not as the start
/// of a longer one.
pub fn has_hashtag(text: &str, tag: &str) -> bool {
    let (text, tag) = (text.to_lowercase(), tag.to_lowercase());
    text.match_indices(&tag)
        .any(|(i, _)| text[i + tag.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_')))
}

/// Total number of messages in a chat (an empty search counts everything).
pub async fn message_count(chat_id: i64) -> Res<i32> {
    search(chat_id, String::new()).await.map(|(_, total)| total)
}

/// Last message sent no later than `date` (unix time).
pub async fn message_at(chat_id: i64, date: i32) -> Res<i64> {
    let enums::Message::Message(msg) = functions::get_chat_message_by_date(chat_id, date, client())
        .await
        .map_err(err)?;
    Ok(msg.id)
}

pub async fn delete(chat_id: i64, ids: Vec<i64>, revoke: bool) -> Res<()> {
    functions::delete_messages(chat_id, ids, revoke, client())
        .await
        .map_err(err)
}

fn text_content(text: String) -> InputMessageContent {
    InputMessageContent::InputMessageText(types::InputMessageText {
        text: types::FormattedText {
            text,
            entities: Vec::new(),
        },
        link_preview_options: None,
        clear_draft: true,
    })
}

/// How a message is sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendOptions {
    pub reply_to: Option<i64>,
    /// Deliver without a notification sound.
    pub silent: bool,
    /// Unix time to send at (scheduled message).
    pub at: Option<i32>,
}

impl SendOptions {
    fn reply_to(self) -> Option<enums::InputMessageReplyTo> {
        self.reply_to.map(|message_id| {
            enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
                message_id,
                quote: None,
                checklist_task_id: 0,
            })
        })
    }

    fn options(self) -> Option<types::MessageSendOptions> {
        (self.silent || self.at.is_some()).then(|| types::MessageSendOptions {
            disable_notification: self.silent,
            scheduling_state: self.at.map(|send_date| {
                enums::MessageSchedulingState::SendAtDate(types::MessageSchedulingStateSendAtDate {
                    send_date,
                    repeat_period: 0,
                })
            }),
            ..Default::default()
        })
    }
}

async fn send(chat_id: i64, content: InputMessageContent, options: SendOptions) -> Res<()> {
    functions::send_message(chat_id, None, options.reply_to(), options.options(), content, client())
        .await
        .map(|_| ())
        .map_err(err)
}

pub async fn send_text(chat_id: i64, text: String, options: SendOptions) -> Res<()> {
    send(chat_id, text_content(text), options).await
}

/// What a file is sent as, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Photo,
    Video,
    Document,
}

impl FileKind {
    pub fn of(path: &std::path::Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "jpg" | "jpeg" | "png" | "webp" | "heic" => FileKind::Photo,
            "mp4" | "mov" | "mkv" | "webm" | "m4v" => FileKind::Video,
            _ => FileKind::Document,
        }
    }
}

/// A local file as a photo, video or document, with an optional caption.
fn file_content(
    path: &std::path::Path,
    caption: String,
    video: Option<crate::player::VideoInfo>,
    preview: Option<crate::player::Thumbnail>,
) -> InputMessageContent {
    let file = enums::InputFile::Local(types::InputFileLocal {
        path: path.to_string_lossy().into_owned(),
    });
    let caption = (!caption.is_empty()).then(|| types::FormattedText {
        text: caption,
        entities: Vec::new(),
    });
    match FileKind::of(path) {
        FileKind::Photo => InputMessageContent::InputMessagePhoto(types::InputMessagePhoto {
            photo: file,
            thumbnail: None,
            added_sticker_file_ids: Vec::new(),
            width: 0,
            height: 0,
            caption,
            show_caption_above_media: false,
            self_destruct_type: None,
            has_spoiler: false,
        }),
        // TDLib sends duration and size as given; they come from the file.
        FileKind::Video => InputMessageContent::InputMessageVideo(types::InputMessageVideo {
            video: file,
            thumbnail: preview.map(|t| types::InputThumbnail {
                thumbnail: enums::InputFile::Local(types::InputFileLocal {
                    path: t.path.to_string_lossy().into_owned(),
                }),
                width: t.width,
                height: t.height,
            }),
            cover: None,
            start_timestamp: 0,
            added_sticker_file_ids: Vec::new(),
            duration: video.map_or(0, |v| v.duration),
            width: video.map_or(0, |v| v.width),
            height: video.map_or(0, |v| v.height),
            supports_streaming: true,
            caption,
            show_caption_above_media: false,
            self_destruct_type: None,
            has_spoiler: false,
        }),
        FileKind::Document => InputMessageContent::InputMessageDocument(types::InputMessageDocument {
            document: file,
            thumbnail: None,
            disable_content_type_detection: false,
            caption,
        }),
    }
}

/// Send local files; the caption goes with the first one. Two to ten
/// photos and videos go as an album.
pub async fn send_files(chat_id: i64, paths: Vec<std::path::PathBuf>, caption: String, options: SendOptions) -> Res<()> {
    let album = (2..=10).contains(&paths.len()) && paths.iter().all(|p| FileKind::of(p) != FileKind::Document);
    // Videos: read length and size from the file and make a preview first.
    let probe_paths = paths.clone();
    let infos: Vec<_> = tokio::task::spawn_blocking(move || {
        probe_paths
            .iter()
            .map(|p| {
                let info = (FileKind::of(p) == FileKind::Video).then(|| crate::player::probe_video(p)).flatten();
                let preview = info.and_then(|info| crate::player::video_thumbnail(p, info).ok());
                (info, preview)
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut caption = Some(caption);
    let contents: Vec<InputMessageContent> = paths
        .iter()
        .zip(infos)
        .map(|(path, (info, preview))| file_content(path, caption.take().unwrap_or_default(), info, preview))
        .collect();
    if album {
        return functions::send_message_album(chat_id, None, options.reply_to(), options.options(), contents, client())
            .await
            .map(|_| ())
            .map_err(err);
    }
    for content in contents {
        send(chat_id, content, options).await?;
    }
    Ok(())
}

/// The pinned message of a chat, if there is one.
pub async fn pinned(chat_id: i64) -> Res<Option<Msg>> {
    match functions::get_chat_pinned_message(chat_id, client()).await {
        Ok(enums::Message::Message(msg)) => Ok(Some(Msg::from(msg))),
        Err(e) if e.code == 404 => Ok(None),
        Err(e) => Err(err(e)),
    }
}

pub async fn edit_text(chat_id: i64, message_id: i64, text: String) -> Res<()> {
    functions::edit_message_text(chat_id, message_id, text_content(text), client())
        .await
        .map(|_| ())
        .map_err(err)
}

/// Emoji the user can react with to a message, most used first. Premium-only
/// and custom emoji reactions are left out.
pub async fn available_reactions(chat_id: i64, message_id: i64) -> Res<Vec<String>> {
    let enums::AvailableReactions::AvailableReactions(available) =
        functions::get_message_available_reactions(chat_id, message_id, 8, client()).await.map_err(err)?;
    let mut emoji: Vec<String> = Vec::new();
    let all = available.top_reactions.iter().chain(&available.recent_reactions).chain(&available.popular_reactions);
    for reaction in all.filter(|r| !r.needs_premium) {
        if let enums::ReactionType::Emoji(e) = &reaction.r#type
            && !emoji.contains(&e.emoji)
        {
            emoji.push(e.emoji.clone());
        }
    }
    Ok(emoji)
}

pub async fn add_reaction(chat_id: i64, message_id: i64, emoji: String) -> Res<()> {
    let kind = enums::ReactionType::Emoji(types::ReactionTypeEmoji { emoji });
    functions::add_message_reaction(chat_id, message_id, kind, false, true, client())
        .await
        .map_err(err)
}

pub async fn remove_reaction(chat_id: i64, message_id: i64, kind: ReactionKind) -> Res<()> {
    let Some(kind) = kind.to_tg() else {
        return Err("Paid reactions can't be taken back".into());
    };
    functions::remove_message_reaction(chat_id, message_id, kind, client())
        .await
        .map_err(err)
}

/// Where a t.me link points inside Telegram: a chat, maybe a message in it.
pub async fn resolve_link(url: String) -> Res<(i64, Option<i64>)> {
    if let Ok(enums::MessageLinkInfo::MessageLinkInfo(info)) = functions::get_message_link_info(url.clone(), client()).await
        && info.chat_id != 0
    {
        return Ok((info.chat_id, info.message.map(|m| m.id)));
    }
    // Not a message link: t.me/username opens the chat.
    let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
    let username = parsed
        .path_segments()
        .and_then(|mut s| s.next())
        .filter(|u| !u.is_empty() && !u.starts_with('+') && !["c", "joinchat", "addstickers", "share"].contains(u))
        .ok_or_else(|| format!("can't open {url} here"))?;
    let enums::Chat::Chat(chat) = functions::search_public_chat(username.to_owned(), client()).await.map_err(err)?;
    Ok((chat.id, None))
}

/// Load a chat TDLib may not know yet (the origin of a forward); fails
/// when it can't be opened, e.g. a private channel you are not in.
pub async fn load_chat(chat_id: i64) -> Res<i64> {
    functions::get_chat(chat_id, client()).await.map(|_| chat_id).map_err(err)
}

/// The private chat with a user, created if needed.
pub async fn private_chat(user_id: i64) -> Res<i64> {
    let enums::Chat::Chat(chat) = functions::create_private_chat(user_id, false, client()).await.map_err(err)?;
    Ok(chat.id)
}

/// Forward `ids`; a non-empty `comment` goes first as a message of its own,
/// like Telegram's "add a comment".
pub async fn forward(to_chat: i64, from_chat: i64, ids: Vec<i64>, comment: String, options: SendOptions) -> Res<()> {
    if !comment.is_empty() {
        send_text(to_chat, comment, options).await?;
    }
    functions::forward_messages(to_chat, None, from_chat, ids, options.options(), false, false, client())
        .await
        .map(|_| ())
        .map_err(err)
}

pub async fn send_voice(chat_id: i64, recording: Recording, options: SendOptions) -> Res<()> {
    use base64::Engine;
    let content = InputMessageContent::InputMessageVoiceNote(types::InputMessageVoiceNote {
        voice_note: enums::InputFile::Local(types::InputFileLocal {
            path: recording.path.to_string_lossy().into_owned(),
        }),
        duration: recording.duration,
        // TDLib's JSON interface takes `bytes` as base64.
        waveform: base64::engine::general_purpose::STANDARD.encode(&recording.waveform),
        caption: None,
        self_destruct_type: None,
    });
    send(chat_id, content, options).await
}

/// Gracefully close TDLib so its database is flushed, but never hang on exit.
pub async fn close() {
    let _ = tokio::time::timeout(Duration::from_secs(3), functions::close(client())).await;
}

/// A chat list: the main one, the archive or a user folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKey {
    Main,
    Archive,
    Folder(i32),
}

impl ListKey {
    pub fn from_tg(list: &ChatList) -> Self {
        match list {
            ChatList::Main => ListKey::Main,
            ChatList::Archive => ListKey::Archive,
            ChatList::Folder(f) => ListKey::Folder(f.chat_folder_id),
        }
    }

    fn to_tg(self) -> ChatList {
        match self {
            ListKey::Main => ChatList::Main,
            ListKey::Archive => ChatList::Archive,
            ListKey::Folder(id) => ChatList::Folder(types::ChatListFolder { chat_folder_id: id }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub key: ListKey,
    pub name: String,
    pub icon: &'static str,
}

/// Folders in the order Telegram shows them, with the archive last.
pub fn folders(update: types::UpdateChatFolders) -> Vec<Folder> {
    let mut folders: Vec<Folder> = update
        .chat_folders
        .into_iter()
        .map(|f| Folder {
            key: ListKey::Folder(f.id),
            name: f.name.text.text,
            icon: folder_icon(&f.icon.name),
        })
        .collect();
    let main = Folder {
        key: ListKey::Main,
        name: "All chats".into(),
        icon: "💬",
    };
    let at = usize::try_from(update.main_chat_list_position).unwrap_or(0).min(folders.len());
    folders.insert(at, main);
    folders.push(Folder {
        key: ListKey::Archive,
        name: "Archive".into(),
        icon: "🗄",
    });
    folders
}

fn folder_icon(name: &str) -> &'static str {
    match name {
        "All" => "💬",
        "Unread" => "🔵",
        "Unmuted" => "🔔",
        "Bots" => "🤖",
        "Channels" => "📢",
        "Groups" => "👥",
        "Private" => "👤",
        "Cat" => "🐱",
        "Crown" => "👑",
        "Favorite" => "⭐",
        "Flower" => "🌹",
        "Game" => "🎮",
        "Home" => "🏠",
        "Love" => "❤",
        "Mask" => "🎭",
        "Party" => "🎉",
        "Sport" => "⚽",
        "Study" => "🎓",
        "Trade" => "📈",
        "Travel" | "Airplane" => "✈",
        "Work" => "💼",
        "Book" => "📖",
        "Light" => "💡",
        "Like" => "👍",
        "Money" => "💰",
        "Note" => "📝",
        "Palette" => "🎨",
        _ => "📁",
    }
}

/// What kind of chat it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    /// One-to-one chat (or secret chat) with this user.
    Private(i64),
    Group,
    Channel,
}

impl ChatKind {
    pub fn of(kind: &enums::ChatType) -> Self {
        match kind {
            enums::ChatType::Private(p) => ChatKind::Private(p.user_id),
            enums::ChatType::Secret(s) => ChatKind::Private(s.user_id),
            enums::ChatType::BasicGroup(_) => ChatKind::Group,
            enums::ChatType::Supergroup(s) if s.is_channel => ChatKind::Channel,
            enums::ChatType::Supergroup(_) => ChatKind::Group,
        }
    }

    /// The notification settings scope chats of this kind default to.
    pub fn scope(self) -> Scope {
        match self {
            ChatKind::Private(_) => Scope::Private,
            ChatKind::Group => Scope::Group,
            ChatKind::Channel => Scope::Channel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Private,
    Group,
    Channel,
}

impl Scope {
    pub fn of(scope: &enums::NotificationSettingsScope) -> Self {
        match scope {
            enums::NotificationSettingsScope::PrivateChats => Scope::Private,
            enums::NotificationSettingsScope::GroupChats => Scope::Group,
            enums::NotificationSettingsScope::ChannelChats => Scope::Channel,
        }
    }
}

/// When a user was last online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Online,
    /// Unix time.
    Offline(i32),
    Recently,
    LastWeek,
    LastMonth,
    Unknown,
}

impl Presence {
    pub fn of(status: &enums::UserStatus) -> Self {
        match status {
            enums::UserStatus::Online(_) => Presence::Online,
            enums::UserStatus::Offline(o) => Presence::Offline(o.was_online),
            enums::UserStatus::Recently(_) => Presence::Recently,
            enums::UserStatus::LastWeek(_) => Presence::LastWeek,
            enums::UserStatus::LastMonth(_) => Presence::LastMonth,
            enums::UserStatus::Empty => Presence::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
    pub presence: Presence,
    /// File id of the small profile photo.
    pub avatar: Option<i32>,
}

impl From<&types::User> for User {
    fn from(user: &types::User) -> Self {
        User {
            name: format!("{} {}", user.first_name, user.last_name).trim().to_owned(),
            presence: Presence::of(&user.status),
            avatar: user.profile_photo.as_ref().map(|p| p.small.id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sender {
    User(i64),
    Chat(i64),
}

impl From<&MessageSender> for Sender {
    fn from(sender: &MessageSender) -> Self {
        match sender {
            MessageSender::User(u) => Sender::User(u.user_id),
            MessageSender::Chat(c) => Sender::Chat(c.chat_id),
        }
    }
}

/// A remote file with the pixel size it will have once downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub id: i32,
    pub width: u32,
    pub height: u32,
    /// Bytes, exact or expected; 0 when unknown.
    #[serde(default)]
    pub size: i64,
}

impl FileRef {
    fn new(file: &types::File, width: i32, height: i32) -> Self {
        FileRef {
            id: file.id,
            width: width.max(1) as u32,
            height: height.max(1) as u32,
            size: file.size.max(file.expected_size),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StickerKind {
    /// Static WebP image.
    Webp,
    /// Lottie animation (gzipped JSON).
    Tgs,
    /// VP9 video with alpha channel.
    Webm,
}

/// Media the UI can render inline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Media {
    Photo {
        /// Size used in the message list.
        preview: FileRef,
        /// Largest size, for the full-screen viewer.
        full: FileRef,
    },
    Sticker {
        file: FileRef,
        kind: StickerKind,
        emoji: String,
    },
    /// What Telegram calls a GIF: usually a silent looping MP4.
    Animation {
        file: FileRef,
    },
    /// A video, or a round video message when `round`.
    Video {
        /// The video file; its size is the natural size of the video.
        video: FileRef,
        /// Still preview image, if Telegram has one in a decodable format.
        thumb: Option<i32>,
        /// Tiny blurred preview embedded in the message.
        #[serde(skip)]
        mini: Option<Handle>,
        duration: i32,
        round: bool,
        /// Round video messages: watched to the end (by the recipient, for
        /// outgoing ones). Always true for other videos.
        #[serde(default = "yes")]
        listened: bool,
    },
    Voice {
        file: i32,
        duration: i32,
        /// Bar heights, 0..=31.
        waveform: Vec<u8>,
        /// Played to the end (by the recipient, for outgoing ones).
        #[serde(default = "yes")]
        listened: bool,
    },
}

impl Media {
    /// The image file shown in the message list, if any.
    pub fn inline_file(&self) -> Option<i32> {
        match self {
            Media::Photo { preview, .. } => Some(preview.id),
            Media::Sticker { file, .. } | Media::Animation { file } => Some(file.id),
            Media::Video { thumb, .. } => *thumb,
            Media::Voice { .. } => None,
        }
    }

    /// A voice or round video message nobody has played yet.
    pub fn unlistened(&self) -> bool {
        matches!(self, Media::Voice { listened: false, .. } | Media::Video { listened: false, .. })
    }

    pub fn set_listened(&mut self) {
        if let Media::Voice { listened, .. } | Media::Video { listened, .. } = self {
            *listened = true;
        }
    }

    /// Every file the media consists of, with its size (0 when unknown).
    pub fn files(&self) -> Vec<(i32, i64)> {
        match self {
            Media::Photo { preview, full } => vec![(preview.id, preview.size), (full.id, full.size)],
            Media::Sticker { file, .. } | Media::Animation { file } => vec![(file.id, file.size)],
            Media::Video { video, thumb, .. } => {
                let mut files = vec![(video.id, video.size)];
                files.extend(thumb.map(|t| (t, 0)));
                files
            }
            Media::Voice { file, .. } => vec![(*file, 0)],
        }
    }

    /// Replace file ids, e.g. with the archive's local copies.
    pub fn remap_files(&mut self, mut f: impl FnMut(i32) -> i32) {
        match self {
            Media::Photo { preview, full } => {
                let same = preview.id == full.id;
                preview.id = f(preview.id);
                full.id = if same { preview.id } else { f(full.id) };
            }
            Media::Sticker { file, .. } | Media::Animation { file } => file.id = f(file.id),
            Media::Video { video, thumb, .. } => {
                video.id = f(video.id);
                *thumb = thumb.map(&mut f);
            }
            Media::Voice { file, .. } => *file = f(*file),
        }
    }

    /// Natural pixel size, for layout. Voice messages have none.
    pub fn dims(&self) -> Option<(u32, u32)> {
        let file = match self {
            Media::Photo { preview, .. } => preview,
            Media::Sticker { file, .. } | Media::Animation { file } => file,
            Media::Video { video, .. } => video,
            Media::Voice { .. } => return None,
        };
        Some((file.width, file.height))
    }

    /// Photos and videos open in the full-screen viewer.
    pub fn viewable(&self) -> bool {
        matches!(self, Media::Photo { .. } | Media::Video { .. })
    }

    fn label(&self) -> String {
        match self {
            Media::Photo { .. } => "🖼 Photo".into(),
            Media::Animation { .. } => "GIF".into(),
            Media::Sticker { emoji, .. } => format!("{emoji} Sticker"),
            Media::Video { round: false, .. } => "📹 Video".into(),
            Media::Video { round: true, .. } => "📹 Video message".into(),
            Media::Voice { duration, .. } => format!("🎤 Voice message {}", fmt_duration(*duration)),
        }
    }
}

pub fn fmt_duration(secs: i32) -> String {
    let secs = secs.max(0);
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// The parts of a TDLib message the UI cares about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub id: i64,
    pub chat_id: i64,
    pub sender: Sender,
    pub outgoing: bool,
    pub date: i32,
    pub edited: bool,
    /// Text, or the caption of rendered media.
    pub text: String,
    /// Several items when messages of an album are merged into one.
    pub media: Vec<Media>,
    /// Non-zero for messages that belong to an album.
    pub album: i64,
    /// Ids of all messages merged into this one (album parts), including `id`.
    pub members: Vec<i64>,
    /// Message in the same chat this one replies to.
    pub reply_to: Option<i64>,
    /// Not yet delivered to the server.
    pub sending: bool,
    /// Scheduled for later; not part of the chat history.
    pub scheduled: bool,
    /// Deleted, but kept from the local archive.
    pub deleted: bool,
    /// Media files already downloaded when the message arrived: (id, path).
    #[serde(skip)]
    pub local_files: Vec<(i32, String)>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub forward: Option<Forward>,
    #[serde(default)]
    pub links: Vec<Link>,
    /// Bold, italic, spoilers… in `text`.
    #[serde(default)]
    pub styles: Vec<Styled>,
    /// The photo or video is hidden under a spoiler until shown.
    #[serde(default)]
    pub spoiler: bool,
    /// What the first link in the text points to.
    #[serde(default)]
    pub preview: Option<Preview>,
}

/// A link preview: the page's site, title, description and picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub url: String,
    pub site: String,
    pub title: String,
    pub description: String,
    pub photo: Option<FileRef>,
}

/// How a byte range of `Msg::text` is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Styled {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    Bold,
    Italic,
    Underline,
    Strike,
    /// Inline code and code blocks.
    Code,
    /// Hidden until shown.
    Spoiler,
}

/// A link in a message's text: the byte range of `Msg::text` it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// Byte offset in `text` of an offset in UTF-16 code units (TDLib's).
fn byte_offset(text: &str, units: i32) -> usize {
    let mut seen = 0;
    for (i, c) in text.char_indices() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16() as i32;
    }
    text.len()
}

/// Links of formatted text: URLs, links behind words, e-mail addresses
/// and @mentions (as t.me links).
pub fn links(text: &types::FormattedText) -> Vec<Link> {
    use enums::TextEntityType as T;
    let mut links: Vec<Link> = text
        .entities
        .iter()
        .filter_map(|e| {
            let start = byte_offset(&text.text, e.offset);
            let end = byte_offset(&text.text, e.offset + e.length);
            let shown = text.text.get(start..end).filter(|s| !s.is_empty())?;
            let url = match &e.r#type {
                T::Url if shown.contains("://") => shown.to_owned(),
                T::Url => format!("https://{shown}"),
                T::TextUrl(t) => t.url.clone(),
                T::EmailAddress => format!("mailto:{shown}"),
                T::Mention => format!("https://t.me/{}", shown.trim_start_matches('@')),
                // Not a URL: opening it shows the chat's messages with it.
                T::Hashtag => shown.to_owned(),
                _ => return None,
            };
            Some(Link { start, end, url })
        })
        .collect();
    links.sort_by_key(|l| l.start);
    links
}

pub fn styles(text: &types::FormattedText) -> Vec<Styled> {
    use enums::TextEntityType as T;
    text.entities
        .iter()
        .filter_map(|e| {
            let style = match &e.r#type {
                T::Bold => Style::Bold,
                // Quotes have no frame here; slanted, they still stand out.
                T::Italic | T::BlockQuote | T::ExpandableBlockQuote => Style::Italic,
                T::Underline => Style::Underline,
                T::Strikethrough => Style::Strike,
                T::Code | T::Pre | T::PreCode(_) => Style::Code,
                T::Spoiler => Style::Spoiler,
                _ => return None,
            };
            let start = byte_offset(&text.text, e.offset);
            let end = byte_offset(&text.text, e.offset + e.length);
            (start < end).then_some(Styled { start, end, style })
        })
        .collect()
}

/// The formatted text a message shows as its text, if it is shown as is
/// (no "[photo] " label), so that entity offsets fit it.
fn shown_formatted<'a>(content: &'a MessageContent, shown: &str) -> Option<&'a types::FormattedText> {
    let formatted = match content {
        MessageContent::MessageText(t) => &t.text,
        MessageContent::MessagePhoto(p) => &p.caption,
        MessageContent::MessageAnimation(a) => &a.caption,
        MessageContent::MessageVideo(v) => &v.caption,
        MessageContent::MessageVoiceNote(v) => &v.caption,
        _ => return None,
    };
    (formatted.text == shown).then_some(formatted)
}

/// The links of the text a message shows.
pub fn content_links(content: &MessageContent, shown: &str) -> Vec<Link> {
    shown_formatted(content, shown).map(links).unwrap_or_default()
}

/// The styles of the text a message shows.
pub fn content_styles(content: &MessageContent, shown: &str) -> Vec<Styled> {
    shown_formatted(content, shown).map(styles).unwrap_or_default()
}

/// Preview pictures are small: the smallest size at least this large.
const PREVIEW_PHOTO_SIDE: i32 = 160;

pub fn link_preview(content: &MessageContent) -> Option<Preview> {
    use enums::LinkPreviewType as T;
    let MessageContent::MessageText(t) = content else {
        return None;
    };
    let p = t.link_preview.as_ref()?;
    let photo = match &p.r#type {
        T::Photo(t) => Some(&t.photo),
        T::Article(t) => t.photo.as_ref(),
        T::Video(t) => t.cover.as_ref(),
        _ => None,
    };
    let photo = photo.and_then(|photo| {
        let mut sizes: Vec<&types::PhotoSize> = photo.sizes.iter().collect();
        sizes.sort_by_key(|s| s.width.max(s.height));
        let size = sizes.iter().find(|s| s.width.max(s.height) >= PREVIEW_PHOTO_SIDE).or(sizes.last())?;
        Some(FileRef::new(&size.photo, size.width, size.height))
    });
    let preview = Preview {
        url: p.url.clone(),
        site: p.site_name.clone(),
        title: p.title.clone(),
        description: p.description.text.clone(),
        photo,
    };
    (!preview.site.is_empty() || !preview.title.is_empty() || !preview.description.is_empty() || preview.photo.is_some())
        .then_some(preview)
}

/// A photo, video or GIF sent under a spoiler.
pub fn media_spoiler(content: &MessageContent) -> bool {
    match content {
        MessageContent::MessagePhoto(p) => p.has_spoiler,
        MessageContent::MessageVideo(v) => v.has_spoiler,
        MessageContent::MessageAnimation(a) => a.has_spoiler,
        _ => false,
    }
}


/// Where a forwarded message comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Forward {
    /// The original author; `None` when they hide their account.
    pub sender: Option<Sender>,
    /// Their name when hidden, or a channel post's signature.
    pub name: String,
    /// The original message (chat, message), when Telegram says where it is:
    /// channel posts, and messages saved to Saved Messages.
    pub source: Option<(i64, i64)>,
}

fn forward_origin(info: Option<&types::MessageForwardInfo>) -> Option<Forward> {
    let info = info?;
    let saved = info.source.as_ref().map(|s| (s.chat_id, s.message_id)).filter(|&(chat, msg)| chat != 0 && msg != 0);
    Some(match &info.origin {
        enums::MessageOrigin::User(u) => Forward { sender: Some(Sender::User(u.sender_user_id)), name: String::new(), source: saved },
        enums::MessageOrigin::HiddenUser(h) => Forward { sender: None, name: h.sender_name.clone(), source: saved },
        enums::MessageOrigin::Chat(c) => Forward {
            sender: Some(Sender::Chat(c.sender_chat_id)),
            name: c.author_signature.clone(),
            source: saved,
        },
        enums::MessageOrigin::Channel(c) => Forward {
            sender: Some(Sender::Chat(c.chat_id)),
            name: c.author_signature.clone(),
            source: Some((c.chat_id, c.message_id)),
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReactionKind {
    Emoji(String),
    /// Custom emoji: shown as a generic sparkle, the sticker isn't loaded.
    Custom(i64),
    /// Telegram Stars.
    Paid,
}

impl ReactionKind {
    fn of(kind: &enums::ReactionType) -> Self {
        match kind {
            enums::ReactionType::Emoji(e) => ReactionKind::Emoji(e.emoji.clone()),
            enums::ReactionType::CustomEmoji(c) => ReactionKind::Custom(c.custom_emoji_id),
            enums::ReactionType::Paid => ReactionKind::Paid,
        }
    }

    fn to_tg(&self) -> Option<enums::ReactionType> {
        match self {
            ReactionKind::Emoji(emoji) => Some(enums::ReactionType::Emoji(types::ReactionTypeEmoji { emoji: emoji.clone() })),
            ReactionKind::Custom(id) => Some(enums::ReactionType::CustomEmoji(types::ReactionTypeCustomEmoji {
                custom_emoji_id: *id,
            })),
            ReactionKind::Paid => None,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            ReactionKind::Emoji(emoji) => emoji,
            ReactionKind::Custom(_) => "✨",
            ReactionKind::Paid => "⭐",
        }
    }
}

/// One reaction under a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub kind: ReactionKind,
    pub count: i32,
    /// Put by the user.
    pub chosen: bool,
}

pub fn reactions(info: Option<&types::MessageInteractionInfo>) -> Vec<Reaction> {
    let Some(reactions) = info.and_then(|i| i.reactions.as_ref()) else {
        return Vec::new();
    };
    reactions
        .reactions
        .iter()
        .map(|r| Reaction {
            kind: ReactionKind::of(&r.r#type),
            count: r.total_count,
            chosen: r.is_chosen,
        })
        .collect()
}

impl From<types::Message> for Msg {
    fn from(m: types::Message) -> Self {
        let media = media(&m.content);
        let text = if media.is_some() {
            caption(&m.content)
        } else {
            content_text(&m.content, m.is_outgoing)
        };
        let local_files = content_files(&m.content)
            .into_iter()
            .filter(|f| f.local.is_downloading_completed && !f.local.path.is_empty())
            .map(|f| (f.id, f.local.path.clone()))
            .collect();
        let reply_to = match &m.reply_to {
            Some(enums::MessageReplyTo::Message(r)) if r.chat_id == m.chat_id => Some(r.message_id),
            _ => None,
        };
        Msg {
            id: m.id,
            chat_id: m.chat_id,
            sender: Sender::from(&m.sender_id),
            outgoing: m.is_outgoing,
            date: m.date,
            edited: m.edit_date != 0,
            media: media.into_iter().collect(),
            album: m.media_album_id,
            members: vec![m.id],
            reply_to,
            sending: matches!(m.sending_state, Some(enums::MessageSendingState::Pending(_))),
            scheduled: m.scheduling_state.is_some(),
            deleted: false,
            local_files,
            reactions: reactions(m.interaction_info.as_ref()),
            forward: forward_origin(m.forward_info.as_ref()),
            links: content_links(&m.content, &text),
            styles: content_styles(&m.content, &text),
            spoiler: media_spoiler(&m.content),
            preview: link_preview(&m.content),
            text,
        }
    }
}

impl From<Record> for Msg {
    /// A deleted message, as remembered by the archive.
    fn from(r: Record) -> Self {
        // Newer records keep the whole message; media points at the
        // archive's copies of the files.
        if let Some(mut msg) = r.data.as_deref().and_then(|d| serde_json::from_str::<Msg>(d).ok()) {
            let saved: HashMap<i32, &str> = r.files.iter().map(|(id, path)| (*id, path.as_str())).collect();
            for media in &mut msg.media {
                media.remap_files(|id| match saved.get(&id) {
                    Some(path) => local_file_id(path),
                    None => missing_file_id(),
                });
            }
            msg.deleted = true;
            msg.sending = false;
            return msg;
        }
        Msg {
            id: r.id,
            chat_id: r.chat_id,
            sender: r.sender,
            outgoing: r.outgoing,
            date: r.date,
            edited: false,
            text: r.text,
            media: Vec::new(),
            album: 0,
            members: vec![r.id],
            reply_to: None,
            sending: false,
            scheduled: false,
            deleted: true,
            local_files: Vec::new(),
            reactions: Vec::new(),
            forward: None,
            links: Vec::new(),
            styles: Vec::new(),
            spoiler: false,
            preview: None,
        }
    }
}

impl Msg {
    /// The text with its spoilers blanked out, for previews.
    pub fn masked_text(&self) -> String {
        let mut text = self.text.clone();
        let mut spoilers: Vec<&Styled> = self.styles.iter().filter(|s| s.style == Style::Spoiler).collect();
        // From the end, so earlier offsets stay valid.
        spoilers.sort_by_key(|s| std::cmp::Reverse(s.start));
        for s in spoilers {
            if let Some(hidden) = text.get(s.start..s.end) {
                let blank: String = hidden.chars().map(|c| if c == '\n' { c } else { '▒' }).collect();
                text.replace_range(s.start..s.end, &blank);
            }
        }
        text
    }

    /// One-line description for chat previews.
    pub fn summary(&self) -> String {
        self.describe(self.masked_text())
    }

    /// Like `summary`, spoilers included: for the archive and exports.
    pub fn full_summary(&self) -> String {
        self.describe(self.text.clone())
    }

    fn describe(&self, text: String) -> String {
        let Some(first) = self.media.first() else {
            return text;
        };
        let label = if self.media.len() > 1 {
            format!("🖼 Album ({})", self.media.len())
        } else {
            first.label()
        };
        if text.is_empty() {
            label
        } else {
            format!("{label}, {text}")
        }
    }

    pub fn contains(&self, id: i64) -> bool {
        self.id == id || self.members.contains(&id)
    }

    /// Merge another part of the same album into this message.
    pub fn absorb(&mut self, other: Msg) {
        self.members.extend(other.members);
        self.media.extend(other.media);
        self.edited |= other.edited;
        // The other part's text is appended after a newline, its links too.
        let shift = if self.text.is_empty() { 0 } else { self.text.len() + 1 };
        self.links.extend(other.links.into_iter().map(|l| Link { start: l.start + shift, end: l.end + shift, ..l }));
        self.styles.extend(other.styles.into_iter().map(|s| Styled { start: s.start + shift, end: s.end + shift, ..s }));
        self.spoiler |= other.spoiler;
        // Reactions to an album are put on one of its parts.
        if self.reactions.is_empty() {
            self.reactions = other.reactions;
        }
        if self.text.is_empty() {
            self.text = other.text;
        } else if !other.text.is_empty() {
            self.text = format!("{}\n{}", self.text, other.text);
        }
    }
}

/// Smallest photo size at least this large is shown inline.
const PREVIEW_MIN_SIDE: i32 = 640;

fn caption(content: &MessageContent) -> String {
    match content {
        MessageContent::MessagePhoto(p) => p.caption.text.clone(),
        MessageContent::MessageAnimation(a) => a.caption.text.clone(),
        MessageContent::MessageVideo(v) => v.caption.text.clone(),
        MessageContent::MessageVoiceNote(v) => v.caption.text.clone(),
        _ => String::new(),
    }
}

fn decode_base64(data: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

fn minithumbnail(mini: Option<&types::Minithumbnail>) -> Option<Handle> {
    Some(Handle::from_bytes(decode_base64(&mini?.data)?))
}

/// Only still image thumbnails; animated ones (MPEG4, GIF) are skipped.
fn still_thumbnail(thumb: Option<&types::Thumbnail>) -> Option<i32> {
    let thumb = thumb?;
    matches!(
        thumb.format,
        ThumbnailFormat::Jpeg | ThumbnailFormat::Png | ThumbnailFormat::Webp
    )
    .then_some(thumb.file.id)
}

/// Unpack Telegram's little-endian 5-bit waveform.
fn unpack_waveform(data: &str) -> Vec<u8> {
    let Some(bytes) = decode_base64(data) else {
        return Vec::new();
    };
    (0..bytes.len() * 8 / 5)
        .map(|i| {
            let bit = i * 5;
            let lo = u16::from(bytes[bit / 8]);
            let hi = u16::from(bytes.get(bit / 8 + 1).copied().unwrap_or(0));
            (((hi << 8 | lo) >> (bit % 8)) & 0x1f) as u8
        })
        .collect()
}

pub fn media(content: &MessageContent) -> Option<Media> {
    match content {
        MessageContent::MessagePhoto(p) => {
            let mut sizes: Vec<&types::PhotoSize> = p.photo.sizes.iter().collect();
            sizes.sort_by_key(|s| s.width.max(s.height));
            let full = *sizes.last()?;
            let preview = sizes
                .iter()
                .find(|s| s.width.max(s.height) >= PREVIEW_MIN_SIDE)
                .unwrap_or(&full);
            Some(Media::Photo {
                preview: FileRef::new(&preview.photo, preview.width, preview.height),
                full: FileRef::new(&full.photo, full.width, full.height),
            })
        }
        MessageContent::MessageSticker(s) => {
            let s = &s.sticker;
            let kind = match s.format {
                StickerFormat::Webp => StickerKind::Webp,
                StickerFormat::Tgs => StickerKind::Tgs,
                StickerFormat::Webm => StickerKind::Webm,
            };
            Some(Media::Sticker {
                file: FileRef::new(&s.sticker, s.width, s.height),
                kind,
                emoji: s.emoji.clone(),
            })
        }
        MessageContent::MessageAnimation(a) => {
            let a = &a.animation;
            Some(Media::Animation {
                file: FileRef::new(&a.animation, a.width, a.height),
            })
        }
        MessageContent::MessageVideo(v) => {
            let v = &v.video;
            Some(Media::Video {
                video: FileRef::new(&v.video, v.width, v.height),
                thumb: still_thumbnail(v.thumbnail.as_ref()),
                mini: minithumbnail(v.minithumbnail.as_ref()),
                duration: v.duration,
                round: false,
                listened: true,
            })
        }
        MessageContent::MessageVideoNote(note) => {
            let v = &note.video_note;
            Some(Media::Video {
                video: FileRef::new(&v.video, v.length, v.length),
                thumb: still_thumbnail(v.thumbnail.as_ref()),
                mini: minithumbnail(v.minithumbnail.as_ref()),
                duration: v.duration,
                round: true,
                listened: note.is_viewed,
            })
        }
        MessageContent::MessageVoiceNote(note) => {
            let v = &note.voice_note;
            Some(Media::Voice {
                file: v.voice.id,
                duration: v.duration,
                waveform: unpack_waveform(&v.waveform),
                listened: note.is_listened,
            })
        }
        _ => None,
    }
}

/// Every file attached to a message (all photo sizes, thumbnails, …).
fn content_files(content: &MessageContent) -> Vec<&types::File> {
    match content {
        MessageContent::MessagePhoto(p) => p.photo.sizes.iter().map(|s| &s.photo).collect(),
        MessageContent::MessageSticker(s) => vec![&s.sticker.sticker],
        MessageContent::MessageAnimation(a) => vec![&a.animation.animation],
        MessageContent::MessageVideo(v) => {
            let mut files = vec![&v.video.video];
            files.extend(v.video.thumbnail.as_ref().map(|t| &t.file));
            files
        }
        MessageContent::MessageVideoNote(v) => {
            let mut files = vec![&v.video_note.video];
            files.extend(v.video_note.thumbnail.as_ref().map(|t| &t.file));
            files
        }
        MessageContent::MessageVoiceNote(v) => vec![&v.voice_note.voice],
        MessageContent::MessageDocument(d) => vec![&d.document.document],
        MessageContent::MessageAudio(a) => vec![&a.audio.audio],
        _ => Vec::new(),
    }
}

fn call_text(call: &types::MessageCall, outgoing: bool) -> String {
    use enums::CallDiscardReason as Reason;
    let icon = if call.is_video { "📹" } else { "📞" };
    let kind = if call.is_video { "video call" } else { "call" };
    let direction = if outgoing { "Outgoing" } else { "Incoming" };
    match call.discard_reason {
        Reason::Missed if outgoing => format!("{icon} Cancelled {kind}"),
        Reason::Missed => format!("{icon} Missed {kind}"),
        Reason::Declined => format!("{icon} Declined {kind}"),
        _ if call.duration > 0 => format!("{icon} {direction} {kind} ({})", fmt_duration(call.duration)),
        _ => format!("{icon} {direction} {kind}"),
    }
}

/// "MessageGiftedPremium" → "gifted premium": better than nothing for
/// service messages without their own text.
fn variant_name(content: &MessageContent) -> String {
    let debug = format!("{content:?}");
    let name = debug.split(['(', ' ', '{']).next().unwrap_or_default();
    let name = name.strip_prefix("Message").unwrap_or(name);
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    out
}

pub fn content_text(content: &MessageContent, outgoing: bool) -> String {
    let with_caption = |label: &str, caption: &types::FormattedText| {
        if caption.text.is_empty() {
            format!("[{label}]")
        } else {
            format!("[{label}] {}", caption.text)
        }
    };
    match content {
        MessageContent::MessageText(t) => t.text.text.clone(),
        MessageContent::MessagePhoto(p) => with_caption("photo", &p.caption),
        MessageContent::MessageVideo(v) => with_caption("video", &v.caption),
        MessageContent::MessageAnimation(a) => with_caption("gif", &a.caption),
        MessageContent::MessageDocument(d) => with_caption("file", &d.caption),
        MessageContent::MessageAudio(a) => with_caption("audio", &a.caption),
        MessageContent::MessageSticker(s) => format!("[sticker {}]", s.sticker.emoji),
        MessageContent::MessageVoiceNote(_) => "[voice message]".into(),
        MessageContent::MessageVideoNote(_) => "[video message]".into(),
        MessageContent::MessageLocation(_) => "[location]".into(),
        MessageContent::MessageContact(c) => {
            let name = format!("{} {}", c.contact.first_name, c.contact.last_name);
            format!("👤 {} {}", name.trim(), c.contact.phone_number)
        }
        MessageContent::MessagePoll(p) => format!("📊 Poll: {}", p.poll.question.text),
        MessageContent::MessageCall(call) => call_text(call, outgoing),
        MessageContent::MessageGroupCall(call) => {
            let kind = if call.is_video { "group video call" } else { "group call" };
            match () {
                _ if call.is_active => format!("📞 Ongoing {kind}"),
                _ if call.was_missed => format!("📞 Missed {kind}"),
                _ if call.duration > 0 => format!("📞 {} ({})", capitalize(kind), fmt_duration(call.duration)),
                _ => format!("📞 {}", capitalize(kind)),
            }
        }
        MessageContent::MessageVideoChatStarted(_) => "🎙 Video chat started".into(),
        MessageContent::MessageVideoChatScheduled(_) => "🎙 Video chat scheduled".into(),
        MessageContent::MessageVideoChatEnded(v) => format!("🎙 Video chat ended ({})", fmt_duration(v.duration)),
        MessageContent::MessageInviteVideoChatParticipants(_) => "🎙 Invited to the video chat".into(),
        MessageContent::MessageVenue(v) => format!("📍 {}", v.venue.title),
        MessageContent::MessageAnimatedEmoji(e) => e.emoji.clone(),
        MessageContent::MessageDice(d) => format!("{} {}", d.emoji, d.value),
        MessageContent::MessageStory(_) => "[story]".into(),
        MessageContent::MessageChatChangeTitle(t) => format!("changed the title to «{}»", t.title),
        MessageContent::MessageChatChangePhoto(_) => "changed the chat photo".into(),
        MessageContent::MessageChatDeletePhoto => "removed the chat photo".into(),
        MessageContent::MessageChatAddMembers(_) => "added members".into(),
        MessageContent::MessageChatJoinByLink => "joined by link".into(),
        MessageContent::MessageChatJoinByRequest => "joined the chat".into(),
        MessageContent::MessageChatDeleteMember(_) => "removed a member".into(),
        MessageContent::MessageBasicGroupChatCreate(_) | MessageContent::MessageSupergroupChatCreate(_) => {
            "created the group".into()
        }
        MessageContent::MessagePinMessage(_) => "📌 pinned a message".into(),
        MessageContent::MessageScreenshotTaken => "took a screenshot".into(),
        MessageContent::MessageContactRegistered => "joined Telegram".into(),
        MessageContent::MessageExpiredPhoto => "[photo has expired]".into(),
        MessageContent::MessageExpiredVideo => "[video has expired]".into(),
        MessageContent::MessageExpiredVideoNote => "[video message has expired]".into(),
        MessageContent::MessageExpiredVoiceNote => "[voice message has expired]".into(),
        MessageContent::MessageGift(_) => "🎁 Gift".into(),
        MessageContent::MessageUnsupported => "[not supported by TDLib yet]".into(),
        other => format!("[{}]", variant_name(other)),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(reason: enums::CallDiscardReason, duration: i32, is_video: bool) -> types::MessageCall {
        types::MessageCall { is_video, discard_reason: reason, duration }
    }

    #[test]
    fn calls_are_described() {
        use enums::CallDiscardReason as R;
        assert_eq!(call_text(&call(R::HungUp, 83, false), true), "📞 Outgoing call (1:23)");
        assert_eq!(call_text(&call(R::HungUp, 5, true), false), "📹 Incoming video call (0:05)");
        assert_eq!(call_text(&call(R::Missed, 0, false), false), "📞 Missed call");
        assert_eq!(call_text(&call(R::Missed, 0, false), true), "📞 Cancelled call");
        assert_eq!(call_text(&call(R::Declined, 0, false), false), "📞 Declined call");
        let content = MessageContent::MessageCall(call(R::Disconnected, 0, false));
        assert_eq!(content_text(&content, false), "📞 Incoming call");
    }

    #[test]
    fn unknown_service_messages_get_a_readable_name() {
        assert_eq!(content_text(&MessageContent::MessageScreenshotTaken, false), "took a screenshot");
        let other = MessageContent::MessageChatSetMessageAutoDeleteTime(Default::default());
        assert_eq!(content_text(&other, false), "[chat set message auto delete time]");
    }

    #[test]
    fn media_files_can_be_remapped() {
        let mut photo = Media::Photo {
            preview: FileRef { id: 5, width: 1, height: 1, size: 10 },
            full: FileRef { id: 5, width: 1, height: 1, size: 10 },
        };
        assert_eq!(photo.files(), vec![(5, 10), (5, 10)]);
        let mut calls = 0;
        photo.remap_files(|id| {
            calls += 1;
            -id
        });
        assert_eq!(calls, 1, "a shared file is remapped once");
        assert_eq!(photo.files(), vec![(-5, 10), (-5, 10)]);
        assert!(local_file_id("/a") < 0);
        assert_eq!(local_file_id("/a"), local_file_id("/a"));
    }

    #[test]
    fn listened_state() {
        let mut voice = Media::Voice { file: 1, duration: 2, waveform: Vec::new(), listened: false };
        assert!(voice.unlistened());
        voice.set_listened();
        assert!(!voice.unlistened());
        // Archived before the field existed: no unlistened dot.
        let old: Media = serde_json::from_str(r#"{"Voice":{"file":1,"duration":2,"waveform":[]}}"#).unwrap();
        assert!(!old.unlistened());
        let photo = Media::Photo {
            preview: FileRef { id: 1, width: 1, height: 1, size: 0 },
            full: FileRef { id: 1, width: 1, height: 1, size: 0 },
        };
        assert!(!photo.unlistened());
    }

    #[test]
    fn links_in_text() {
        let entity = |offset, length, r#type| types::TextEntity { offset, length, r#type };
        // 😁 is two UTF-16 units and four bytes.
        let text = types::FormattedText {
            text: "😁 see example.com, @durov or this".into(),
            entities: vec![
                entity(30, 4, enums::TextEntityType::TextUrl(types::TextEntityTypeTextUrl { url: "https://a.b/c".into() })),
                entity(7, 11, enums::TextEntityType::Url),
                entity(20, 6, enums::TextEntityType::Mention),
                entity(0, 2, enums::TextEntityType::Bold),
            ],
        };
        let found = links(&text);
        let found: Vec<(&str, &str)> = found.iter().map(|l| (&text.text[l.start..l.end], l.url.as_str())).collect();
        assert_eq!(
            found,
            [
                ("example.com", "https://example.com"),
                ("@durov", "https://t.me/durov"),
                ("this", "https://a.b/c"),
            ]
        );
    }

    #[test]
    fn styles_and_spoilers() {
        let entity = |offset, length, r#type| types::TextEntity { offset, length, r#type };
        let text = types::FormattedText {
            text: "жирный и тайна".into(),
            entities: vec![
                entity(0, 6, enums::TextEntityType::Bold),
                entity(9, 5, enums::TextEntityType::Spoiler),
                entity(0, 14, enums::TextEntityType::Url),
            ],
        };
        let found = styles(&text);
        let found: Vec<(&str, Style)> = found.iter().map(|s| (&text.text[s.start..s.end], s.style)).collect();
        assert_eq!(found, [("жирный", Style::Bold), ("тайна", Style::Spoiler)]);

        let mut msg = Msg::from(Record {
            chat_id: 1,
            id: 1,
            sender: Sender::User(1),
            outgoing: false,
            date: 0,
            text: text.text.clone(),
            deleted_at: None,
            data: None,
            files: Vec::new(),
        });
        msg.styles = styles(&text);
        assert_eq!(msg.masked_text(), "жирный и ▒▒▒▒▒");
        assert_eq!(msg.summary(), "жирный и ▒▒▒▒▒");
    }

    #[test]
    fn hashtags() {
        let text = types::FormattedText {
            text: "идея #вимграм на завтра".into(),
            entities: vec![types::TextEntity { offset: 5, length: 8, r#type: enums::TextEntityType::Hashtag }],
        };
        let found = links(&text);
        assert_eq!(found.len(), 1);
        assert_eq!(&text.text[found[0].start..found[0].end], "#вимграм");
        assert_eq!(found[0].url, "#вимграм");

        assert!(has_hashtag("идея #Вимграм!", "#вимграм"));
        assert!(has_hashtag("#todo", "#TODO"));
        assert!(has_hashtag("a #todo_x and #todo", "#todo"));
        assert!(!has_hashtag("#todolist", "#todo"));
        assert!(!has_hashtag("#todo_x", "#todo"));
        assert!(!has_hashtag("todo", "#todo"));
    }
}
