use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::keyboard::key::{Named, Physical};
use iced::keyboard::{self, Key, Modifiers};
use iced::mouse::ScrollDelta;
use iced::widget::scrollable::{AbsoluteOffset, RelativeOffset, Viewport};
use iced::{Size, Subscription, Task, Theme, window};
use tdlib_rs::enums::{AuthorizationState, Update};
use tdlib_rs::types::{ChatNotificationSettings, ChatPhotoInfo, ChatPosition};

use crate::archive::{Archive, Record};
use crate::config::{self, Config};
use crate::clipboard::{self, Clip};
use crate::emoji::{self, Picker, React};
use crate::{autostart, export, icon, notify, tray};
use crate::media::{self, Kind, Loaded, Shared, Store};
use crate::player::{self, Audio, Movie, SpeedKind, Speeds};
use crate::tg::{
    self, ChatKind, Folder, ListKey, Media, Membership, Msg, Presence, ReactionKind, Scope, SendOptions, Sender, User,
};
use crate::view;
use crate::vim::{Action, History, Input, Mode, NKey, Pending, TextAction, TextPending, normalize};
use crate::voice::Recorder;

const HISTORY_PAGE: i32 = 50;
/// Keep fetching older history until at least this many messages are loaded.
const MIN_HISTORY: usize = 40;
/// Start loading more when the cursor gets this close to the loaded edge.
const PREFETCH: usize = 8;
const CHATS_PAGE: i32 = 100;
/// Redraw rate while something animates, plays or records.
const TICK: Duration = Duration::from_millis(33);
/// How long the chat list cursor rests on a chat before it opens
/// (`open_on_select`), so holding j doesn't open every chat on the way.
const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// How long the chat filter rests before the server is searched for it.
const GLOBAL_DELAY: Duration = Duration::from_millis(350);
/// Shorter filters only match your chats.
const GLOBAL_MIN_CHARS: usize = 2;
const SEEK_SHORT: f64 = 5.0;
const SEEK_LONG: f64 = 30.0;
/// A voice message or music left for something else goes on from this
/// much earlier, to pick up the thread.
const RESUME_BACK: Duration = Duration::from_secs(5);
/// Touchpad pixels per list step.
const SCROLL_PIXELS: f32 = 40.0;
/// Messages kept visible above and below the cursor, like Vim's `scrolloff`.
const SCROLLOFF: usize = 2;
/// Pixels `j`/`k` scroll inside a message taller than the screen.
const TALL_STEP: f32 = 90.0;
/// Older messages arriving (e.g. after being offline) don't notify.
const NOTIFY_MAX_AGE: i64 = 120;
/// Cached media unused this long is deleted, checked once this often.
const CACHE_DAYS: i32 = 7;

/// Scrollable showing a message taller than the screen.
pub const TALL_ID: &str = "tall-message";
pub const COMPOSE_ID: &str = "compose";
/// Link of a spoiler in a message's text: this prefix and the message id.
pub const SPOILER_LINK: &str = "spoiler:";
/// `OpenChat::tag` of the list of pinned messages (`gP`), not a hashtag.
pub const PINNED_TAG: &str = "📌";

const HELP: &str = "j/k move  gg/G top/bottom  ^d/^u half page  h/l ^h/^l panes  J/K gt/gT Alt-N folders  \
                    Enter open/play  i/I/a/A insert  Esc then vim keys in the text  q record  r reply  R react  cc/e edit  f forward  v visual  dd delete  \
                    yy/Y copy  p/P paste  ge emoji  gv back to playing voice  Space play/pause  [ ] speed  = 1x  { } volume  H/L seek  m mute  U leave/join  gc comments  \
                    / search chats, Telegram or messages  :posts public posts  n/N next/prev match or pinned  ga attach  gp pinned  gP all pinned  gf original/reply  gx link  ^o back  \
                    Alt-Enter silent  :later 21:00  :silent  :attach  :export  :date 01.05.2024  \
                    :folder  :theme  :autostart  :cleancache  :q";

#[derive(Debug, Clone)]
pub enum Message {
    Tg(Vec<Update>),
    Key(keyboard::Event),
    Resized(Size),
    History {
        chat_id: i64,
        /// The hashtag the page was searched for (`OpenChat::tag`).
        tag: Option<String>,
        /// The comment thread it was loaded from (`CommentThread::root`); 0: none.
        thread: i64,
        newer: bool,
        result: Result<Vec<Msg>, String>,
    },
    /// The comments of a channel post found: where they are.
    ThreadOpened {
        channel: i64,
        post: i64,
        result: Result<tg::Thread, String>,
    },
    /// Click on "💬 N comments" under a post.
    OpenComments(i64),
    /// History around a message we jump to (search result or date).
    Window {
        chat_id: i64,
        target: i64,
        result: Result<Vec<Msg>, String>,
    },
    Count {
        chat_id: i64,
        tag: Option<String>,
        result: Result<i32, String>,
    },
    Found {
        chat_id: i64,
        query: String,
        result: Result<(Vec<i64>, i32), String>,
    },
    DateFound {
        chat_id: i64,
        result: Result<i64, String>,
    },
    ChatsLoaded(ListKey, Result<(), String>),
    /// The chat filter stayed the same for a moment: search the server.
    GlobalSearch(String),
    GlobalFound {
        query: String,
        result: Result<tg::Global, String>,
    },
    /// A further page of messages found in all chats.
    GlobalMore {
        query: String,
        result: Result<(Vec<Msg>, String), String>,
    },
    PostsFound {
        query: String,
        result: Result<tg::Posts, String>,
    },
    /// An inline file (avatar, photo, sticker, thumbnail) finished downloading.
    Downloaded {
        file_id: i32,
        result: Result<String, String>,
    },
    Decoded {
        file_id: i32,
        result: Result<Shared<Loaded>, String>,
    },
    /// A voice message to play finished downloading.
    PlayFile {
        file_id: i32,
        result: Result<String, String>,
    },
    VideoReady {
        file_id: i32,
        result: Result<Shared<Movie>, String>,
    },
    /// Deleted messages remembered by the archive.
    Archived {
        chat_id: i64,
        result: Result<Vec<Record>, String>,
    },
    Pinned {
        chat_id: i64,
        result: Result<Vec<Msg>, String>,
    },
    JumpPinned,
    /// Only the pinned messages (`gP`, a click on the bar's counter).
    ListPinned,
    Picked(Vec<PathBuf>),
    Exported(Result<(PathBuf, usize), String>),
    WindowEvent(window::Event),
    WindowOpened,
    WindowClosed(window::Id),
    CloseRequested(window::Id),
    Tray(tray::Output),
    /// A notification of this chat was clicked.
    NotificationClicked(i64),
    Autostart(Result<String, String>),
    CacheCleaned(Result<i64, String>),
    TallScroll(Viewport),
    Paste(Option<String>),
    Clip(Result<Clip, String>),
    /// The message after one that finished playing, for autoplay.
    NextMedia {
        chat_id: i64,
        after: i64,
        /// The finished one was music: only music follows it.
        music: bool,
        next: Option<Msg>,
    },
    /// Player bar of a voice message from another chat.
    JumpPlaying,
    /// The cursor rested on this chat (`open_on_select`).
    PreviewChat(i64),
    /// The reply quote or "Forwarded from" line of a message was clicked.
    ClickSource(i64),
    /// The ↓ button over a chat scrolled up.
    ToLatest,
    /// A link in a message was clicked (or picked with `gx`).
    OpenLink(String),
    /// A t.me link resolved to a chat (and maybe a message in it).
    LinkResolved {
        from: Option<(i64, i64)>,
        result: Result<(i64, Option<i64>), String>,
    },
    /// The chat of a forward's origin is loaded: open it (at `msg_id`).
    GoTo {
        from: (i64, i64),
        msg_id: Option<i64>,
        result: Result<i64, String>,
    },
    PauseVoice,
    StopVoice,
    /// Emoji panel: open/close, pick a tab, insert an emoji.
    ToggleEmoji,
    EmojiTab(usize),
    EmojiPick(String),
    /// Emoji that can be put as a reaction to the message (`R`).
    Reactions {
        chat_id: i64,
        msg_id: i64,
        result: Result<Vec<String>, String>,
    },
    /// A reaction under a message was clicked.
    ToggleReaction { msg_id: i64, kind: ReactionKind },
    ClickFolder(usize),
    ClickChat(i64),
    /// Click on the photo or video beside the chat: the keys go to it.
    FocusViewer,
    /// A message found by the chat filter was clicked.
    ClickHit(usize),
    SelectMsg(i64),
    ClickMedia { msg_id: i64, item: usize },
    /// Click on a bar of a voice message's waveform: play from there.
    SeekVoice { msg_id: i64, item: usize, fraction: f64 },
    /// Dragging a progress slider: of a music file in the chat, or of the
    /// video in the viewer (`None`). Seeks once let go.
    Scrub(Option<(i64, usize)>, f64),
    ScrubDone,
    Scroll(Pane, ScrollDelta),
    Tick(Instant),
    Done(Result<(), String>),
    Quit,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Starting,
    Phone,
    Code,
    Password { hint: String },
    Ready,
    Info(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Folders,
    Chats,
    Messages,
}

/// Mouse wheel targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Folders,
    Chats,
    Messages,
    Emoji,
}

/// What Enter in Insert mode does with the composed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compose {
    New,
    Reply(i64),
    Edit(i64),
}

/// Media item shown full screen: message id and index within an album.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewer {
    pub msg_id: i64,
    pub item: usize,
}

pub struct VoicePlayback {
    pub chat_id: i64,
    pub msg_id: i64,
    pub sender: Sender,
    pub file_id: i32,
    /// Voice message or music: each keeps its own speed.
    pub kind: SpeedKind,
    /// Where to start once downloaded; zero: where it was left, if anywhere.
    start: Duration,
    /// `None` while the file downloads.
    pub audio: Option<Audio>,
}

/// What speed a voice message or music file plays at.
fn audio_kind(media: &Media) -> SpeedKind {
    if matches!(media, Media::Audio { .. }) { SpeedKind::Music } else { SpeedKind::Voice }
}

pub struct VideoPlayback {
    file_id: i32,
    /// Chat and message of a round video, until it played to the end.
    round_msg: Option<(i64, i64)>,
    /// Video or round video message: each keeps its own speed.
    kind: SpeedKind,
    /// `None` until the stream has started.
    pub movie: Option<Movie>,
    /// A speed change failed (e.g. mid-seek) and is retried on the next tick.
    speed_pending: bool,
}

impl VideoPlayback {
    pub fn video(&self) -> Option<&iced_video_player::Video> {
        self.movie.as_ref().map(|m| &m.video)
    }

    fn movie_mut(&mut self) -> Option<&mut Movie> {
        self.movie.as_mut()
    }
}

/// Messages picked with `f`, waiting for the chat to forward them to.
pub struct Forward {
    from: i64,
    ids: Vec<i64>,
    /// "the message" or "3 messages".
    pub what: String,
    /// What the compose line held before: it is the comment's while forwarding.
    draft: Input,
}

/// Results of `/` in the message pane, or the pinned messages (`gp`).
pub struct Found {
    pub query: String,
    /// The pinned messages, not a search.
    pub pins: bool,
    /// Matching message ids, newest first.
    pub ids: Vec<i64>,
    pub total: i32,
    pub index: usize,
}

/// What the chat filter found on the server, below the chats it matches.
#[derive(Default)]
pub struct Global {
    /// The filter these are for.
    query: String,
    /// Chats beyond the filtered folder: your other chats, public ones.
    chats: Vec<i64>,
    /// Messages in all your chats, newest first.
    pub msgs: Vec<Msg>,
    /// Where the next page of `msgs` starts; empty: no more.
    offset: String,
    /// Posts of public channels, after Enter.
    pub posts: Vec<Msg>,
    pub loading: bool,
}

impl Global {
    pub fn hits(&self) -> usize {
        self.msgs.len() + self.posts.len()
    }

    pub fn hit(&self, i: usize) -> Option<&Msg> {
        self.msgs.get(i).or_else(|| self.posts.get(i.checked_sub(self.msgs.len())?))
    }
}

/// A destructive action waiting for confirmation.
#[derive(Debug, Clone)]
pub enum Confirm {
    Delete { chat_id: i64, ids: Vec<i64> },
    Leave { chat_id: i64, title: String },
    /// Esc with nothing left to cancel.
    Quit,
}

#[derive(Debug, Clone, Copy)]
enum PasteTarget {
    /// Message (or login) input; `Some(before)` pastes at the start/end.
    Input(Option<bool>),
    Cmdline,
}

/// Per-chat notification override.
#[derive(Debug, Clone, Copy, Default)]
struct Mute {
    use_default: bool,
    mute_for: i32,
}

impl Mute {
    fn of(settings: &ChatNotificationSettings) -> Self {
        Mute {
            use_default: settings.use_default_mute_for,
            mute_for: settings.mute_for,
        }
    }
}

pub struct Chat {
    pub id: i64,
    pub title: String,
    pub kind: ChatKind,
    pub last: Option<Msg>,
    pub unread: i32,
    /// Outgoing messages up to this id were read by the other side.
    pub read_outbox: i64,
    mute: Mute,
    /// All of its notification settings, to change one keeping the rest.
    notify: ChatNotificationSettings,
    /// The supergroup or channel it is, for your membership.
    supergroup: Option<i64>,
    /// Order within each chat list the chat belongs to.
    positions: HashMap<ListKey, i64>,
    /// File id of the small chat photo.
    pub avatar: Option<i32>,
}

impl Chat {
    fn set_position(&mut self, position: &ChatPosition) {
        let key = ListKey::from_tg(&position.list);
        if position.order == 0 {
            self.positions.remove(&key);
        } else {
            self.positions.insert(key, position.order);
        }
    }

    /// In one of your chat lists (not just found on the server).
    pub fn listed(&self) -> bool {
        !self.positions.is_empty()
    }

    fn order(&self, list: ListKey) -> Option<i64> {
        self.positions.get(&list).copied()
    }
}

fn avatar_of(photo: Option<&ChatPhotoInfo>) -> Option<i32> {
    photo.map(|p| p.small.id)
}

pub struct OpenChat {
    pub id: i64,
    /// Sorted by id, oldest first; album parts are merged.
    pub msgs: Vec<Msg>,
    pub cursor: usize,
    /// Last message shown at the bottom of the viewport.
    pub bottom: usize,
    pub loading: bool,
    /// No older history left.
    pub exhausted: bool,
    /// The newest message is loaded; false after jumping into the past.
    pub at_latest: bool,
    /// Total number of messages in the chat.
    pub total: Option<i32>,
    /// Pinned messages, newest first.
    pub pins: Vec<Msg>,
    /// Opened by moving through the chat list (`open_on_select`): nothing
    /// is read until the message pane gets focus.
    pub preview: bool,
    /// Only the messages with this hashtag are shown (`#tag`).
    pub tag: Option<String>,
    /// The chat is a discussion group, showing the comments of a post.
    pub thread: Option<CommentThread>,
}

/// The comments of a channel post, shown in its discussion group.
pub struct CommentThread {
    pub channel: i64,
    pub post: i64,
    /// The thread's id in the group: the post's copy there.
    pub root: i64,
    /// The post's copy, shown above the comments once they are all loaded.
    roots: Vec<Msg>,
}

/// Which messages of the chat an open chat shows.
#[derive(Clone)]
struct Shown {
    tag: Option<String>,
    thread: i64,
}

impl Shown {
    fn keeps(&self, m: &Msg) -> bool {
        // The pinned ones are all there: they come from `OpenChat::pins`.
        if self.tag.as_deref() == Some(PINNED_TAG) {
            return false;
        }
        // A comment answers the post, should its thread be missing.
        let in_thread = self.thread == 0 || m.thread == self.thread || m.reply_to == Some(self.thread);
        self.tag.as_ref().is_none_or(|tag| tg::has_hashtag(&m.text, tag)) && in_thread
    }
}

impl OpenChat {
    fn new(id: i64) -> Self {
        OpenChat {
            id,
            msgs: Vec::new(),
            cursor: 0,
            bottom: 0,
            loading: false,
            exhausted: false,
            at_latest: true,
            total: None,
            pins: Vec::new(),
            preview: false,
            tag: None,
            thread: None,
        }
    }

    fn shown(&self) -> Shown {
        Shown {
            tag: self.tag.clone(),
            thread: self.thread_root(),
        }
    }

    /// Showing the list of pinned messages (`gP`).
    pub fn pins_only(&self) -> bool {
        self.tag.as_deref() == Some(PINNED_TAG)
    }

    /// The list of pinned messages, oldest first, the cursor on `at`.
    fn fill_pins(&mut self, at: Option<i64>) {
        self.retag(Some(PINNED_TAG.to_owned()));
        let pins = self.pins.clone();
        self.edit(|msgs| msgs.extend(pins.into_iter().rev()));
        self.exhausted = true;
        self.total = Some(self.pins.len() as i32);
        self.cursor = at.and_then(|id| self.position(id)).unwrap_or(self.msgs.len().saturating_sub(1));
        self.bottom = self.msgs.len().saturating_sub(1);
    }

    pub fn thread_root(&self) -> i64 {
        self.thread.as_ref().map_or(0, |t| t.root)
    }

    /// Show the messages with `tag` only (`None`: all), loaded afresh.
    fn retag(&mut self, tag: Option<String>) {
        self.tag = tag;
        self.msgs.clear();
        self.cursor = 0;
        self.bottom = 0;
        self.loading = false;
        self.exhausted = false;
        self.at_latest = true;
        self.total = None;
    }

    fn at_end(&self) -> bool {
        self.cursor + 1 >= self.msgs.len()
    }

    /// Index of the message with this id, also when it is part of an album.
    pub fn position(&self, id: i64) -> Option<usize> {
        self.msgs.iter().rposition(|m| m.contains(id))
    }

    pub fn current(&self) -> Option<&Msg> {
        self.msgs.get(self.cursor)
    }

    /// The pinned message the bar shows: the newest one at or above the
    /// cursor (the oldest one above them all), and its index in `pins`.
    pub fn shown_pin(&self) -> Option<(usize, &Msg)> {
        let i = match self.current() {
            Some(cur) => self
                .pins
                .iter()
                .position(|p| p.id <= cur.id || cur.contains(p.id))
                .unwrap_or(self.pins.len().saturating_sub(1)),
            None => 0,
        };
        self.pins.get(i).map(|p| (i, p))
    }

    /// Apply `f` to the message list while keeping the cursor and the
    /// viewport anchored to the same messages.
    fn edit(&mut self, f: impl FnOnce(&mut Vec<Msg>)) {
        let cursor = self.msgs.get(self.cursor).map(|m| m.id);
        let bottom = self.msgs.get(self.bottom).map(|m| m.id);
        f(&mut self.msgs);
        self.msgs.sort_by_key(|m| m.id);
        self.msgs.dedup_by_key(|m| m.id);
        merge_albums(&mut self.msgs);
        // A deleted anchor falls through to the next newer message.
        let last = self.msgs.len().saturating_sub(1);
        let find = |id: Option<i64>| {
            id.map_or(last, |id| {
                self.position(id)
                    .unwrap_or_else(|| self.msgs.partition_point(|m| m.id < id).min(last))
            })
        };
        let (cursor, bottom) = (find(cursor), find(bottom));
        self.cursor = cursor;
        self.bottom = bottom;
    }

    fn media(&self, viewer: Viewer) -> Option<&Media> {
        self.msgs.get(self.position(viewer.msg_id)?)?.media.get(viewer.item)
    }

    /// Next/previous photo or video in the chat, across albums.
    fn viewable_near(&self, from: Viewer, step: isize) -> Option<Viewer> {
        let all: Vec<Viewer> = self
            .msgs
            .iter()
            .flat_map(|m| {
                m.media
                    .iter()
                    .enumerate()
                    .filter(|(_, media)| media.viewable())
                    .map(|(item, _)| Viewer { msg_id: m.id, item })
            })
            .collect();
        let at = all.iter().position(|v| *v == from)?;
        all.get(at.checked_add_signed(step)?).copied()
    }
}

/// Add archived deleted messages. Live messages win, except ones already
/// known to be deleted: their archived copy has the media files.
fn restore_archived(msgs: &mut Vec<Msg>, records: Vec<Record>, shown: &Shown) {
    let archived: Vec<Msg> = records
        .into_iter()
        .map(Msg::from)
        .filter(|m| shown.keeps(m))
        .map(|m| m.in_thread(shown.thread))
        .collect();
    // A deleted album goes as a whole; `merge_albums` rebuilds it from parts.
    msgs.retain(|m| !m.deleted || !archived.iter().any(|a| m.contains(a.id)));
    msgs.extend(archived);
}

/// Telegram sends each album item as its own message; show them as one.
fn merge_albums(msgs: &mut Vec<Msg>) {
    let mut merged: Vec<Msg> = Vec::with_capacity(msgs.len());
    for msg in msgs.drain(..) {
        if let Some(prev) = merged.last_mut()
            && msg.album != 0
            && prev.album == msg.album
        {
            if !prev.contains(msg.id) {
                prev.absorb(msg);
            }
            continue;
        }
        merged.push(msg);
    }
    *msgs = merged;
}

#[derive(Default)]
struct ListLoad {
    loading: bool,
    exhausted: bool,
}

pub struct Status {
    pub text: String,
    pub error: bool,
}

pub struct App {
    cfg: Config,
    pub auth: Auth,
    pub mode: Mode,
    pub focus: Focus,
    pub chats: HashMap<i64, Chat>,
    pub users: HashMap<i64, User>,
    /// Who is typing or recording something, per chat; TDLib cancels
    /// each action itself when it times out.
    pub actions: HashMap<i64, Vec<(Sender, &'static str)>>,
    /// Your standing in each supergroup and channel, by supergroup id.
    memberships: HashMap<i64, Membership>,
    /// Supergroups holding the comments of a channel, by supergroup id.
    discussions: HashSet<i64>,
    archive: Option<Archive>,
    /// Default mute duration per kind of chat.
    scope_mute: HashMap<Scope, i32>,
    /// The window has keyboard focus (no notifications for the open chat).
    focused: bool,
    search_history: History,
    command_history: History,
    /// Scroll position inside a message taller than the screen.
    pub tall_offset: f32,
    /// How far that message can scroll, once the scrollable reported it.
    tall_max: Option<f32>,
    pub folders: Vec<Folder>,
    pub folder: usize,
    /// Number of chats with unread messages, per folder.
    pub folder_unread: HashMap<ListKey, usize>,
    /// Remembered selection of folders that are not shown.
    folder_selection: HashMap<ListKey, i64>,
    list_load: HashMap<ListKey, ListLoad>,
    /// Chat ids shown in the sidebar: current folder, filtered, sorted.
    pub list: Vec<i64>,
    list_dirty: bool,
    pub selected: Option<i64>,
    pub chat_top: usize,
    pub open: Option<OpenChat>,
    pub viewer: Option<Viewer>,
    /// Photos and videos open beside the chat instead of full screen.
    pub viewer_side: bool,
    /// The viewer beside the chat has the keys, not the chat.
    pub viewer_focused: bool,
    pub video: Option<VideoPlayback>,
    pub voice: Option<VoicePlayback>,
    /// A progress slider being dragged: what it seeks and where to.
    pub scrub: Option<(Option<(i64, usize)>, f64)>,
    /// Where voice messages and music were left when something else
    /// started, by file: they go on from a bit earlier.
    resume: HashMap<i32, Duration>,
    /// Playback speed per kind of media, remembered across restarts.
    speeds: Speeds,
    /// Of videos and voice messages, 0.0–1.0.
    volume: f64,
    pub media: Store,
    /// Time base for animations: frame = (now - clock) / frame time.
    pub clock: Instant,
    pub now: Instant,
    /// Animated media is on screen, so redraw on every tick.
    animating: bool,
    pub recorder: Option<Recorder>,
    /// Message being composed, or the login field before authorization.
    pub input: Input,
    /// Length and cursor of `input` the compose box last scrolled to.
    compose_seen: (usize, usize),
    /// Messages whose spoilers were shown (Enter or a click).
    pub revealed: HashSet<i64>,
    pub compose: Compose,
    pub forward: Option<Forward>,
    /// Where `gf` jumped from (chat, message), newest last; `Ctrl-o` goes back.
    jumps: Vec<(i64, i64)>,
    /// Visual selection in the open chat: the message it started at.
    pub visual: Option<i64>,
    /// Chats marked with `v` to forward to, in the order marked.
    pub forward_marks: Vec<i64>,
    pub found: Option<Found>,
    /// `/` searches messages instead of filtering chats.
    pub search_messages: bool,
    pub confirm: Option<Confirm>,
    paste: PasteTarget,
    scroll_pixels: f32,
    pub cmdline: Input,
    pub filter: String,
    pub global: Global,
    /// The cursor of the chat list is on this message of `global`.
    pub hit: Option<usize>,
    pub pending: Pending,
    pub text_pending: TextPending,
    pub status: Option<Status>,
    pub theme: Theme,
    pub size: Size,
    /// The window, while it is open (closed to the tray otherwise).
    window: Option<window::Id>,
    /// The tray icon, once waybar (or another tray) shows it.
    tray: Option<tray::Handle>,
    /// Unread count last shown by the tray icon.
    tray_unread: i32,
    /// Media file id → (chat, message), to archive files once downloaded.
    file_owner: HashMap<i32, (i64, i64)>,
    /// Files to send with the next message (pasted or picked).
    pub attachments: Vec<PathBuf>,
    /// A folder was picked (by the user or `folder` in the config).
    folder_chosen: bool,
    /// The emoji panel, while open.
    pub emoji: Option<Picker>,
    pub recent_emoji: Vec<String>,
}

fn window_settings(size: Size) -> window::Settings {
    window::Settings {
        size,
        icon: icon::window_icon(),
        exit_on_close_request: false,
        // The Wayland app id; other platforms have no such field.
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            application_id: "vimgram".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn find_theme(name: &str) -> Option<Theme> {
    let wanted = normalize_name(name);
    Theme::ALL
        .iter()
        .find(|t| normalize_name(&t.to_string()) == wanted)
        .cloned()
}

/// Folder by name, ignoring case, spaces and punctuation ("личка" finds
/// "Личка").
pub fn find_folder(folders: &[Folder], name: &str) -> Option<usize> {
    let wanted = normalize_name(name);
    folders.iter().position(|f| normalize_name(&f.name) == wanted)
}

fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

impl App {
    /// `hidden`: start in the tray (autostart) instead of opening the window.
    pub fn new(cfg: Config, hidden: bool) -> (Self, Task<Message>) {
        let theme = find_theme(&cfg.theme).unwrap_or(Theme::TokyoNight);
        let viewer_side = cfg.viewer == config::ViewerMode::Side;
        config::set_clock(cfg.clock);
        clipboard::clean_old_pastes();
        let mut app = App {
            cfg,
            auth: Auth::Starting,
            mode: Mode::Normal,
            focus: Focus::Chats,
            chats: HashMap::new(),
            users: HashMap::new(),
            actions: HashMap::new(),
            memberships: HashMap::new(),
            discussions: HashSet::new(),
            archive: Archive::open(config::dirs().data_dir().join("archive.sqlite")).ok(),
            scope_mute: HashMap::new(),
            focused: true,
            search_history: load_history('/'),
            command_history: load_history(':'),
            tall_offset: 0.0,
            tall_max: None,
            folders: tg::folders(tdlib_rs::types::UpdateChatFolders::default()),
            folder: 0,
            folder_unread: HashMap::new(),
            folder_selection: HashMap::new(),
            list_load: HashMap::new(),
            list: Vec::new(),
            list_dirty: false,
            selected: None,
            chat_top: 0,
            open: None,
            viewer: None,
            viewer_side,
            viewer_focused: false,
            video: None,
            voice: None,
            scrub: None,
            resume: HashMap::new(),
            speeds: Speeds::load(),
            volume: player::load_volume(),
            media: Store::default(),
            clock: Instant::now(),
            now: Instant::now(),
            animating: false,
            recorder: None,
            input: Input::default(),
            compose_seen: (0, 0),
            revealed: HashSet::new(),
            compose: Compose::New,
            forward: None,
            jumps: Vec::new(),
            visual: None,
            forward_marks: Vec::new(),
            found: None,
            search_messages: false,
            confirm: None,
            paste: PasteTarget::Input(None),
            scroll_pixels: 0.0,
            cmdline: Input::default(),
            filter: String::new(),
            global: Global::default(),
            hit: None,
            pending: Pending::default(),
            text_pending: TextPending::default(),
            status: None,
            theme,
            size: Size::new(1200.0, 800.0),
            window: None,
            tray: None,
            tray_unread: 0,
            file_owner: HashMap::new(),
            attachments: Vec::new(),
            folder_chosen: false,
            emoji: None,
            recent_emoji: emoji::load_recent(),
        };
        let window = if hidden && app.cfg.tray { Task::none() } else { app.open_window() };
        (app, Task::batch([Task::perform(tg::start(), Message::Done), window]))
    }

    fn open_window(&mut self) -> Task<Message> {
        if let Some(id) = self.window {
            return window::gain_focus(id);
        }
        let (id, opened) = window::open(window_settings(self.size));
        self.window = Some(id);
        self.focused = true;
        opened.map(|_| Message::WindowOpened)
    }

    /// Close the window but keep running in the tray.
    fn hide_window(&mut self) -> Task<Message> {
        match self.window.take() {
            Some(id) => {
                self.focused = false;
                // Nothing plays to a closed window.
                self.stop_recording(false);
                self.stop_voice();
                self.video = None;
                self.viewer = None;
                window::close(id)
            }
            None => Task::none(),
        }
    }

    pub fn title(&self, _window: window::Id) -> String {
        match self.open.as_ref().and_then(|o| self.chats.get(&o.id)) {
            Some(chat) => format!("{} — vimgram", chat.title),
            None => "vimgram".into(),
        }
    }

    pub fn theme(&self, _window: window::Id) -> Theme {
        self.theme.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let ticking = self.animating
            || self.recorder.is_some()
            || self.voice.is_some()
            || self.video.is_some();
        let tick = if ticking {
            iced::time::every(TICK).map(Message::Tick)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            Subscription::run(tg::updates).map(Message::Tg),
            keyboard::listen().map(Message::Key),
            window::resize_events().map(|(_, size)| Message::Resized(size)),
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
            window::events().map(|(_, event)| Message::WindowEvent(event)),
            if self.cfg.tray {
                Subscription::run(tray::run).map(Message::Tray)
            } else {
                Subscription::none()
            },
            tick,
        ])
    }

    pub fn current_list(&self) -> ListKey {
        self.folders.get(self.folder).map_or(ListKey::Main, |f| f.key)
    }

    pub fn sender_name(&self, sender: Sender) -> &str {
        let name = match sender {
            Sender::User(id) => self.users.get(&id).map(|u| u.name.as_str()),
            Sender::Chat(id) => self.chats.get(&id).map(|c| c.title.as_str()),
        };
        name.unwrap_or("…")
    }

    pub fn sender_avatar(&self, sender: Sender) -> Option<i32> {
        match sender {
            Sender::User(id) => self.users.get(&id)?.avatar,
            Sender::Chat(id) => self.chats.get(&id)?.avatar,
        }
    }

    /// Online status of the other side of a private chat.
    pub fn presence(&self, chat_id: i64) -> Option<Presence> {
        match self.chats.get(&chat_id)?.kind {
            ChatKind::Private(user_id) => Some(self.users.get(&user_id)?.presence),
            _ => None,
        }
    }

    /// The message under the cursor is taller than the message pane.
    pub fn is_tall(&self) -> bool {
        let (width, height) = self.viewport();
        self.open
            .as_ref()
            .and_then(OpenChat::current)
            .is_some_and(|m| view::message_height(m, width) > height)
    }

    pub fn membership(&self, chat: &Chat) -> Option<Membership> {
        self.memberships.get(&chat.supergroup?).copied()
    }

    /// A channel you can only read: only its owner and some admins post.
    pub fn read_only(&self, chat_id: i64) -> bool {
        self.chats
            .get(&chat_id)
            .is_some_and(|c| c.kind == ChatKind::Channel && self.membership(c) != Some(Membership::Poster))
    }

    /// Writing in the open chat, or why not (shown as the error).
    fn may_post(&mut self) -> bool {
        let read_only = self.open.as_ref().is_some_and(|o| self.read_only(o.id));
        if read_only {
            self.error("Only the channel's admins post here (m mutes it, U unsubscribes)");
        }
        !read_only
    }

    /// The chat `m` and `U` act on: the one under the cursor in the list,
    /// otherwise the open one.
    fn chat_in_focus(&self) -> Option<&Chat> {
        let id = match self.focus {
            Focus::Chats | Focus::Folders => self.selected.or(self.open.as_ref().map(|o| o.id)),
            Focus::Messages => self.open.as_ref().map(|o| o.id),
        };
        self.chats.get(&id?)
    }

    fn toggle_mute(&mut self) -> Task<Message> {
        let Some(chat) = self.chat_in_focus() else {
            self.error("Select a chat first");
            return Task::none();
        };
        let (id, settings, muted) = (chat.id, chat.notify.clone(), self.muted(chat));
        self.info(format!("{} {}", if muted { "Unmuted" } else { "Muted" }, chat.title));
        Task::perform(tg::set_muted(id, settings, !muted), Message::Done)
    }

    fn leave_or_join(&mut self) -> Task<Message> {
        let Some(chat) = self.chat_in_focus() else {
            self.error("Select a chat first");
            return Task::none();
        };
        let (id, title) = (chat.id, chat.title.clone());
        if matches!(chat.kind, ChatKind::Private(_)) {
            self.error("Only groups and channels can be left");
            return Task::none();
        }
        if self.membership(chat) == Some(Membership::Left) {
            self.info(format!("Joined {title}"));
            return Task::perform(tg::join(id), Message::Done);
        }
        self.confirm = Some(Confirm::Leave { chat_id: id, title });
        Task::none()
    }

    pub fn muted(&self, chat: &Chat) -> bool {
        let mute_for = if chat.mute.use_default {
            self.scope_mute.get(&chat.kind.scope()).copied().unwrap_or(0)
        } else {
            chat.mute.mute_for
        };
        mute_for > 0
    }

    /// A desktop notification for an incoming message, unless unwanted.
    /// The comments under channel posts only notify of replies to you
    /// (`to_me`, which @mentions are too).
    fn notification(&self, msg: &Msg, to_me: bool) -> Task<Message> {
        if msg.outgoing || msg.scheduled {
            return Task::none();
        }
        let Some(chat) = self.chats.get(&msg.chat_id) else {
            return Task::none();
        };
        let looking = self.looking() && self.open.as_ref().is_some_and(|o| o.id == chat.id);
        let age = chrono::Utc::now().timestamp() - i64::from(msg.date);
        let comments = chat.supergroup.is_some_and(|id| self.discussions.contains(&id));
        if looking || self.muted(chat) || age > NOTIFY_MAX_AGE || (comments && !to_me) {
            return Task::none();
        }
        let title = match chat.kind {
            ChatKind::Group => format!("{} · {}", self.sender_name(msg.sender), chat.title),
            _ => chat.title.clone(),
        };
        let body = notify::preview(&msg.summary());
        let (chat_id, avatar, msg_id) = (chat.id, chat.avatar, msg.id);
        Task::perform(
            async move {
                // The chat photo with the app icon in its corner.
                let image = match avatar {
                    Some(file) => match tg::download(file, 16).await {
                        Ok(path) => tokio::task::spawn_blocking(move || {
                            icon::notification_image(std::path::Path::new(&path))
                        })
                        .await
                        .ok()
                        .flatten(),
                        Err(_) => None,
                    },
                    None => None,
                };
                let app_icon = icon::png_path();
                let image = image.or_else(|| app_icon.clone());
                notify::show(title, body, image, app_icon, chat_id, msg_id).await
            },
            move |clicked| {
                if clicked {
                    Message::NotificationClicked(chat_id)
                } else {
                    Message::Done(Ok(()))
                }
            },
        )
    }

    /// Archive messages, and their media files as they get downloaded.
    fn archive_save(&mut self, msgs: &[Msg]) {
        let Some(archive) = &self.archive else { return };
        archive.save(msgs);
        for msg in msgs.iter().filter(|m| !m.deleted && !m.scheduled) {
            for (file_id, path) in &msg.local_files {
                archive.link(msg.chat_id, msg.id, *file_id, path.clone());
            }
            for media in &msg.media {
                for (file_id, _) in media.files() {
                    self.file_owner.insert(file_id, (msg.chat_id, msg.id));
                }
            }
        }
    }

    /// Download the media of an incoming message right away (in private
    /// chats and groups), so it is archived even if deleted unseen.
    fn prefetch(&self, msg: &Msg) -> Task<Message> {
        let channel = self.chats.get(&msg.chat_id).is_none_or(|c| c.kind == ChatKind::Channel);
        if msg.outgoing || channel || self.archive.is_none() {
            return Task::none();
        }
        let limit = i64::from(self.cfg.prefetch_mb) * 1024 * 1024;
        Task::batch(
            msg.media
                .iter()
                .flat_map(Media::files)
                .filter(|&(_, size)| size <= limit)
                .map(|(file, _)| Task::perform(tg::prefetch(file), |_| Message::Done(Ok(())))),
        )
    }

    /// Bring back messages deleted in `lo..=hi` that the archive remembers.
    fn load_archived(&self, chat_id: i64, lo: i64, hi: i64) -> Task<Message> {
        let Some(archive) = self.archive.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || archive.deleted_between(chat_id, lo, hi))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |result| Message::Archived { chat_id, result },
        )
    }

    fn load_pinned(chat_id: i64) -> Task<Message> {
        Task::perform(tg::pins(chat_id), move |result| Message::Pinned { chat_id, result })
    }

    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.list.iter().position(|&c| c == id)
    }

    /// Row of the chat list cursor: a chat, or a found message below them.
    pub fn cursor_row(&self) -> Option<usize> {
        match self.hit {
            Some(i) => Some(self.list.len() + i),
            None => self.selected_index(),
        }
    }

    /// Rows of the chat list: chats, then the messages found.
    pub fn rows(&self) -> usize {
        self.list.len() + self.global.hits()
    }

    fn current_hit(&self) -> Option<&Msg> {
        self.global.hit(self.hit?)
    }

    /// Size of the message list, which shrinks with the compose box.
    pub fn viewport(&self) -> (f32, f32) {
        let pinned = self.open.as_ref().is_some_and(|o| !o.pins.is_empty() && !o.pins_only());
        // A channel you can't post in has a one-line bar instead.
        let read_only = self.open.as_ref().is_some_and(|o| self.read_only(o.id));
        let empty = Input::default();
        view::messages_viewport(
            self.size,
            if read_only { &empty } else { &self.input },
            !read_only && view::has_banner(self),
            pinned,
            self.emoji.is_some(),
            self.playing_elsewhere().is_some(),
            self.side_viewer(),
        )
    }

    /// A photo or video is open beside the chat.
    pub fn side_viewer(&self) -> bool {
        self.viewer_side && self.viewer_media().is_some()
    }

    /// Keys go to the photo or video viewer (always, when it is full screen).
    pub fn viewer_keys(&self) -> bool {
        self.viewer.is_some() && (self.viewer_focused || !self.viewer_side)
    }

    pub fn viewer_media(&self) -> Option<&Media> {
        self.open.as_ref()?.media(self.viewer?)
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: true });
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        let read = self.end_preview();
        let follow = self.follow_compose_cursor();
        // Whatever changed, make sure what is on screen gets downloaded.
        Task::batch([task, read, follow, self.fetch_visible(), self.sync_tray()])
    }

    /// Keep the cursor line in view when the compose text is longer than
    /// its box.
    fn follow_compose_cursor(&mut self) -> Task<Message> {
        let text = self.input.as_str();
        let (before, _) = self.input.split();
        let seen = (text.len(), before.len());
        if seen == self.compose_seen {
            return Task::none();
        }
        self.compose_seen = seen;
        let width = view::compose_text_width(self.size, self.emoji.is_some(), self.side_viewer());
        let total = view::compose_lines(text, width);
        let shown = total.min(view::MAX_COMPOSE_LINES);
        if total <= shown {
            return Task::none();
        }
        // The cursor's line at the bottom of the box, or as low as it gets.
        let line = view::compose_lines(before, width);
        let top = line.saturating_sub(shown).min(total - shown);
        iced::widget::operation::snap_to(COMPOSE_ID, RelativeOffset { x: 0.0, y: top as f32 / (total - shown) as f32 })
    }

    /// A previewed chat whose messages got focus is read like an opened one.
    fn end_preview(&mut self) -> Task<Message> {
        match self.open.as_mut() {
            Some(open) if open.preview && self.focus == Focus::Messages => {
                open.preview = false;
                self.view_visible()
            }
            _ => Task::none(),
        }
    }

    /// With `open_on_select`, open the chat the cursor stopped on.
    fn schedule_preview(&self) -> Task<Message> {
        match self.selected {
            Some(id) if self.cfg.open_on_select && self.hit.is_none() && self.open.as_ref().is_none_or(|o| o.id != id) => {
                Task::perform(tokio::time::sleep(PREVIEW_DELAY), move |()| Message::PreviewChat(id))
            }
            _ => Task::none(),
        }
    }

    /// Unread chats (not muted, not archived) for the tray icon.
    fn sync_tray(&mut self) -> Task<Message> {
        let Some(tray) = &self.tray else {
            return Task::none();
        };
        let unread = self
            .chats
            .values()
            .filter(|c| c.unread > 0 && c.order(ListKey::Main).is_some() && !self.muted(c))
            .count() as i32;
        if unread == self.tray_unread {
            return Task::none();
        }
        self.tray_unread = unread;
        Task::perform(tray.clone().set_unread(unread), |()| Message::Done(Ok(())))
    }

    /// Select a chat in the sidebar, switching to the main list if the
    /// current folder doesn't have it.
    fn select_chat(&mut self, chat_id: i64) {
        if !self.list.contains(&chat_id) {
            self.folder = 0;
            self.clear_filter();
        }
        self.hit = None;
        self.selected = Some(chat_id);
        self.scroll_chats();
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tg(updates) => {
                let tasks: Vec<_> = updates.into_iter().map(|u| self.on_update(u)).collect();
                self.refresh_list();
                Task::batch(tasks)
            }
            Message::Key(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                text,
                ..
            }) => self.on_key(&key, physical_key, modifiers, text.as_deref()),
            Message::Key(_) => Task::none(),
            Message::Resized(size) => {
                self.size = size;
                self.scroll_chats();
                self.scroll_messages();
                Task::none()
            }
            Message::History { chat_id, tag, thread, newer, result } => {
                if self.open.as_ref().is_some_and(|o| o.tag != tag || o.thread_root() != thread) {
                    return Task::none();
                }
                self.on_history(chat_id, newer, result)
            }
            Message::ThreadOpened { channel, post, result } => match result {
                Ok(thread) if self.open.as_ref().is_some_and(|o| o.id == channel) => {
                    self.open_thread(channel, post, thread)
                }
                Ok(_) => Task::none(),
                Err(e) => {
                    self.error(format!("Can't open the comments: {e}"));
                    Task::none()
                }
            },
            Message::OpenComments(msg_id) => {
                self.select_message(msg_id);
                self.open_comments()
            }
            Message::Window { chat_id, target, result } => self.on_window(chat_id, target, result),
            Message::Count { chat_id, tag, result } => {
                if let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id && o.tag == tag) {
                    open.total = result.ok();
                }
                Task::none()
            }
            Message::Found { chat_id, query, result } => {
                if self.open.as_ref().is_none_or(|o| o.id != chat_id) {
                    return Task::none();
                }
                match result {
                    Ok((ids, _)) if ids.is_empty() => {
                        self.found = None;
                        self.error(format!("Pattern not found: {query}"));
                        Task::none()
                    }
                    Ok((ids, total)) => {
                        let first = ids[0];
                        self.found = Some(Found { query, pins: false, ids, total, index: 0 });
                        self.show_match();
                        self.jump_to_message(first)
                    }
                    Err(e) => {
                        self.error(e);
                        Task::none()
                    }
                }
            }
            Message::DateFound { chat_id, result } => match result {
                Ok(id) if self.open.as_ref().is_some_and(|o| o.id == chat_id) => {
                    self.jump_to_message(id)
                }
                Ok(_) => Task::none(),
                Err(e) => {
                    self.error(e);
                    Task::none()
                }
            },
            Message::ChatsLoaded(list, result) => {
                let state = self.list_load.entry(list).or_default();
                state.loading = false;
                match result {
                    // TDLib answers 404 once the whole list is loaded.
                    Err(e) if e.ends_with("(404)") => state.exhausted = true,
                    Err(e) => self.error(e),
                    Ok(()) => {}
                }
                Task::none()
            }
            Message::Downloaded { file_id, result } => {
                // Failed files stay "requested" so they are not retried in a loop.
                let job = result.ok().and_then(|path| self.media.downloaded(file_id, path));
                match job {
                    Some(job) => Task::perform(media::decode(job), move |result| {
                        Message::Decoded { file_id, result }
                    }),
                    None => Task::none(),
                }
            }
            Message::Decoded { file_id, result } => {
                match result {
                    Ok(shared) => self.media.decoded(file_id, &shared),
                    Err(e) => self.error(e),
                }
                Task::none()
            }
            Message::PlayFile { file_id, result } => self.on_play_file(file_id, result),
            Message::VideoReady { file_id, result } => {
                if let Some(playback) = self.video.as_mut().filter(|p| p.file_id == file_id) {
                    match result {
                        Ok(shared) => playback.movie = shared.take(),
                        Err(e) => self.error(format!("Cannot play video: {e}")),
                    }
                }
                Task::none()
            }
            Message::Archived { chat_id, result } => {
                match result {
                    Ok(records) if !records.is_empty() => {
                        if let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id) {
                            let shown = open.shown();
                            open.edit(|msgs| restore_archived(msgs, records, &shown));
                            self.scroll_messages();
                        }
                    }
                    Ok(_) => {}
                    Err(e) => self.error(format!("Archive: {e}")),
                }
                Task::none()
            }
            Message::Pinned { chat_id, result } => {
                if let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id) {
                    open.pins = result.unwrap_or_default();
                    if open.pins_only() {
                        let at = open.current().map(|m| m.id);
                        open.fill_pins(at);
                    }
                    let ids: Vec<i64> = open.pins.iter().map(|p| p.id).collect();
                    // Going through them with n/N: through the new ones.
                    if let Some(found) = self.found.as_mut().filter(|f| f.pins) {
                        let at = found.ids.get(found.index).copied();
                        found.index = ids.iter().position(|&id| Some(id) == at).unwrap_or(0);
                        found.total = ids.len() as i32;
                        found.ids = ids;
                        if found.ids.is_empty() {
                            self.found = None;
                        }
                    }
                }
                self.scroll_messages();
                Task::none()
            }
            Message::JumpPinned => self.jump_pinned(),
            Message::ListPinned => self.list_pinned(),
            Message::Picked(paths) => {
                self.attach(paths);
                Task::none()
            }
            Message::Exported(result) => {
                match result {
                    Ok((path, count)) => {
                        self.info(format!("Exported {} messages to {}", tg::fmt_count(count), path.display()))
                    }
                    Err(e) => self.error(format!("Export failed: {e}")),
                }
                Task::none()
            }
            Message::WindowEvent(event) => {
                match event {
                    window::Event::Focused => {
                        self.focused = true;
                        return self.view_visible();
                    }
                    window::Event::Unfocused => self.focused = false,
                    _ => {}
                }
                Task::none()
            }
            Message::WindowOpened => {
                self.scroll_messages();
                Task::none()
            }
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                    self.focused = false;
                }
                // Without a tray there is no way back: quit.
                if self.window.is_none() && self.tray.is_none() {
                    return self.handle(Message::Quit);
                }
                Task::none()
            }
            Message::CloseRequested(id) if self.window != Some(id) => window::close(id),
            Message::CloseRequested(_) => {
                if self.tray.is_some() && self.cfg.close_to_tray {
                    self.hide_window()
                } else {
                    self.handle(Message::Quit)
                }
            }
            Message::Tray(tray::Output::Ready(handle)) => {
                self.tray = Some(handle.clone());
                self.tray_unread = 0;
                self.list_dirty = true;
                self.refresh_list();
                Task::none()
            }
            Message::Tray(tray::Output::Failed(e)) => {
                eprintln!("vimgram: no tray icon: {e}");
                // Started hidden but nothing can show us: open the window.
                if self.window.is_none() { self.open_window() } else { Task::none() }
            }
            Message::Tray(tray::Output::Event(event)) => match event {
                tray::Event::Toggle if self.window.is_some() && self.focused => self.hide_window(),
                tray::Event::Toggle | tray::Event::Show => self.open_window(),
                tray::Event::Quit => self.handle(Message::Quit),
            },
            Message::NotificationClicked(chat_id) => {
                let window = self.open_window();
                if self.auth != Auth::Ready {
                    return window;
                }
                self.select_chat(chat_id);
                Task::batch([window, self.open_chat(chat_id)])
            }
            Message::CacheCleaned(result) => {
                match result {
                    Ok(freed) if freed > 0 => self.info(format!("Cache: freed {:.1} MB", freed as f64 / 1e6)),
                    Ok(_) => {}
                    Err(e) => self.error(format!("Cache cleanup: {e}")),
                }
                Task::none()
            }
            Message::Autostart(result) => {
                match result {
                    Ok(text) => self.info(text),
                    Err(e) => self.error(format!("Autostart: {e}")),
                }
                Task::none()
            }
            Message::Clip(Ok(Clip::Text(text))) => self.on_paste(Some(text)),
            Message::Clip(Ok(Clip::Files(paths))) => {
                match self.paste {
                    PasteTarget::Cmdline => {
                        let text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                        return self.on_paste(Some(text.join(" ")));
                    }
                    PasteTarget::Input(_) => self.attach(paths),
                }
                Task::none()
            }
            Message::Clip(Ok(Clip::Empty)) => {
                self.error("Clipboard is empty");
                Task::none()
            }
            // No wl-paste (or not Wayland): plain text through iced.
            Message::Clip(Err(_)) => iced::clipboard::read().map(Message::Paste),
            Message::JumpPlaying => self.act(Action::JumpPlaying),
            Message::NextMedia { chat_id, after, music, next } => self.play_next(chat_id, after, music, next),
            Message::PauseVoice => {
                if let Some(audio) = self.voice.as_mut().and_then(|v| v.audio.as_mut()) {
                    audio.toggle_pause();
                }
                Task::none()
            }
            Message::StopVoice => {
                self.stop_voice();
                self.scroll_messages();
                Task::none()
            }
            Message::ToggleEmoji => {
                if self.emoji.is_some() {
                    self.close_emoji();
                    Task::none()
                } else {
                    self.act(Action::Emoji)
                }
            }
            Message::EmojiTab(tab) => {
                if let Some(picker) = self.emoji.as_mut() {
                    picker.set_tab(tab);
                }
                Task::none()
            }
            Message::EmojiPick(e) => self.pick_emoji(e),
            Message::Reactions { chat_id, msg_id, result } => {
                match result {
                    _ if self.open.as_ref().is_none_or(|o| o.id != chat_id) => {}
                    Ok(mut choices) => {
                        // Reactions already put stay removable even when no longer offered.
                        let msg = self.open.as_ref().and_then(|o| o.position(msg_id).map(|i| &o.msgs[i]));
                        for r in msg.into_iter().flat_map(|m| &m.reactions).filter(|r| r.chosen) {
                            if let ReactionKind::Emoji(e) = &r.kind
                                && !choices.contains(e)
                            {
                                choices.insert(0, e.clone());
                            }
                        }
                        if choices.is_empty() {
                            self.error("No reactions allowed here");
                        } else {
                            self.emoji = Some(Picker::reactions(React { chat_id, msg_id, choices }));
                            self.scroll_messages();
                        }
                    }
                    Err(e) => self.error(format!("Reactions: {e}")),
                }
                Task::none()
            }
            Message::ToggleReaction { msg_id, kind } => match kind {
                ReactionKind::Emoji(e) => self.toggle_reaction(msg_id, e),
                kind => {
                    let Some(chat_id) = self.open.as_ref().map(|o| o.id) else {
                        return Task::none();
                    };
                    let chosen = self.reaction_chosen(msg_id, &kind);
                    if chosen {
                        Task::perform(tg::remove_reaction(chat_id, msg_id, kind), Message::Done)
                    } else {
                        self.error("Only emoji reactions can be put from here");
                        Task::none()
                    }
                }
            },
            Message::TallScroll(viewport) => {
                self.tall_offset = viewport.absolute_offset().y;
                self.tall_max = Some((viewport.content_bounds().height - viewport.bounds().height).max(0.0));
                Task::none()
            }
            Message::Paste(text) => self.on_paste(text),
            Message::ClickFolder(index) => {
                self.viewer_focused = false;
                self.switch_folder(index)
            }
            Message::FocusViewer => {
                self.viewer_focused = true;
                Task::none()
            }
            Message::ClickChat(id) => {
                self.viewer_focused = false;
                self.selected = Some(id);
                self.hit = None;
                self.focus = Focus::Chats;
                self.act(Action::Open)
            }
            Message::ClickHit(i) => {
                self.viewer_focused = false;
                self.hit = Some(i);
                self.focus = Focus::Chats;
                self.act(Action::Open)
            }
            Message::GlobalSearch(query) => self.search_global(query),
            Message::GlobalFound { query, result } => self.on_global(query, result),
            Message::GlobalMore { query, result } => {
                self.on_more_hits(query, result);
                Task::none()
            }
            Message::PostsFound { query, result } => {
                self.on_posts(query, result);
                Task::none()
            }
            Message::SelectMsg(id) => {
                self.select_message(id);
                Task::none()
            }
            Message::SeekVoice { msg_id, item, fraction } => self.seek_media(Some((msg_id, item)), fraction),
            Message::Scrub(target, fraction) => {
                self.scrub = Some((target, fraction.clamp(0.0, 1.0)));
                Task::none()
            }
            Message::ScrubDone => match self.scrub.take() {
                Some((target, fraction)) => self.seek_media(target, fraction),
                None => Task::none(),
            },
            Message::ClickMedia { msg_id, item } => {
                self.select_message(msg_id);
                if self.reveal(msg_id) {
                    return Task::none();
                }
                self.activate_media(msg_id, item)
            }
            Message::Scroll(pane, delta) => self.on_scroll(pane, delta),
            Message::OpenLink(url) => self.open_link(url),
            Message::ToLatest => self.to_latest(),
            Message::LinkResolved { from, result } => match result {
                Ok((chat_id, msg_id)) => {
                    self.jumps.extend(from);
                    self.open_chat_at(chat_id, msg_id)
                }
                Err(e) => {
                    self.error(format!("Can't open the link: {e}"));
                    Task::none()
                }
            },
            Message::ClickSource(msg_id) => {
                self.select_message(msg_id);
                self.go_to_source()
            }
            Message::GoTo { from, msg_id, result } => match result {
                Ok(chat_id) => {
                    self.jumps.push(from);
                    self.open_chat_at(chat_id, msg_id)
                }
                Err(e) => {
                    self.error(format!("Can't open the original: {e}"));
                    Task::none()
                }
            },
            Message::PreviewChat(id) => {
                let still_here = self.selected == Some(id) && self.hit.is_none() && self.focus == Focus::Chats;
                if !still_here || self.forward.is_some() || self.auth != Auth::Ready || self.open.as_ref().is_some_and(|o| o.id == id) {
                    return Task::none();
                }
                let task = self.open_chat(id);
                self.focus = Focus::Chats;
                if let Some(open) = self.open.as_mut() {
                    open.preview = true;
                }
                task
            }
            Message::Tick(now) => {
                self.now = now;
                self.poll_players()
            }
            Message::Done(Ok(())) => Task::none(),
            Message::Done(Err(e)) => {
                self.error(e);
                Task::none()
            }
            Message::Quit => {
                // Commit what the archive has queued before TDLib closes.
                if let Some(archive) = &self.archive {
                    archive.flush();
                }
                self.stop_recording(false);
                self.voice = None;
                self.video = None;
                self.info("Closing…");
                let window = self.window.take().map_or_else(Task::none, window::close);
                Task::batch([window, Task::perform(tg::close(), |()| Message::Exit)])
            }
            Message::Exit => iced::exit(),
        }
    }

    // ---- TDLib updates -------------------------------------------------

    fn on_update(&mut self, update: Update) -> Task<Message> {
        match update {
            Update::AuthorizationState(u) => return self.on_auth(u.authorization_state),
            Update::NewChat(u) => {
                let c = u.chat;
                let last = c.last_message.map(Msg::from);
                if let Some(last) = &last {
                    self.archive_save(std::slice::from_ref(last));
                }
                let mut chat = Chat {
                    id: c.id,
                    title: c.title,
                    kind: ChatKind::of(&c.r#type),
                    last,
                    unread: c.unread_count,
                    read_outbox: c.last_read_outbox_message_id,
                    mute: Mute::of(&c.notification_settings),
                    notify: c.notification_settings.clone(),
                    supergroup: ChatKind::supergroup(&c.r#type),
                    positions: HashMap::new(),
                    avatar: avatar_of(c.photo.as_ref()),
                };
                for p in &c.positions {
                    chat.set_position(p);
                }
                self.chats.insert(c.id, chat);
                self.list_dirty = true;
            }
            Update::ChatTitle(u) => {
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.title = u.title;
                    self.list_dirty = true;
                }
            }
            Update::ChatPhoto(u) => {
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.avatar = avatar_of(u.photo.as_ref());
                }
            }
            Update::ChatPosition(u) => {
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.set_position(&u.position);
                    self.list_dirty = true;
                }
            }
            Update::ChatLastMessage(u) => {
                let last = u.last_message.map(Msg::from);
                if let Some(last) = &last {
                    self.archive_save(std::slice::from_ref(last));
                }
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.last = last;
                    for p in &u.positions {
                        chat.set_position(p);
                    }
                    self.list_dirty = true;
                }
            }
            Update::ChatReadOutbox(u) => {
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.read_outbox = u.last_read_outbox_message_id;
                }
            }
            Update::ChatNotificationSettings(u) => {
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.mute = Mute::of(&u.notification_settings);
                    chat.notify = u.notification_settings;
                }
            }
            Update::ScopeNotificationSettings(u) => {
                self.scope_mute.insert(Scope::of(&u.scope), u.notification_settings.mute_for);
            }
            Update::UserStatus(u) => {
                if let Some(user) = self.users.get_mut(&u.user_id) {
                    user.presence = Presence::of(&u.status);
                }
            }
            Update::ChatAction(u) => {
                let sender = Sender::from(&u.sender_id);
                let acting = self.actions.entry(u.chat_id).or_default();
                acting.retain(|(s, _)| *s != sender);
                match tg::action_label(&u.action) {
                    Some(label) => acting.push((sender, label)),
                    None if acting.is_empty() => {
                        self.actions.remove(&u.chat_id);
                    }
                    None => {}
                }
            }
            Update::MessageIsPinned(u) => {
                if self.open.as_ref().is_some_and(|o| o.id == u.chat_id) {
                    return Self::load_pinned(u.chat_id);
                }
            }
            Update::ChatReadInbox(u) => {
                if let Some(chat) = self.chats.get_mut(&u.chat_id) {
                    chat.unread = u.unread_count;
                    self.list_dirty = true;
                }
                // Read here or on another device: its notifications go away.
                let up_to = u.last_read_inbox_message_id;
                return Task::perform(notify::withdraw(u.chat_id, up_to), |()| Message::Done(Ok(())));
            }
            Update::ChatFolders(u) => {
                let current = self.current_list();
                self.folders = tg::folders(u);
                self.folder = self
                    .folders
                    .iter()
                    .position(|f| f.key == current)
                    .unwrap_or(0);
                self.list_dirty = true;
                // Start in the configured folder (`folder = "…"`).
                if !self.folder_chosen
                    && let Some(i) = self.cfg.folder.as_deref().and_then(|name| find_folder(&self.folders, name))
                {
                    self.folder_chosen = true;
                    self.folder = i;
                    return self.load_more_chats();
                }
            }
            Update::User(u) => {
                self.users.insert(u.user.id, User::from(&u.user));
            }
            Update::Supergroup(u) => {
                self.memberships.insert(u.supergroup.id, Membership::of(&u.supergroup.status));
                if u.supergroup.has_linked_chat && !u.supergroup.is_channel {
                    self.discussions.insert(u.supergroup.id);
                } else {
                    self.discussions.remove(&u.supergroup.id);
                }
            }
            Update::NewMessage(u) => {
                // Replies to your messages count as mentions too.
                let to_me = u.message.contains_unread_mention;
                let msg = Msg::from(u.message);
                self.archive_save(std::slice::from_ref(&msg));
                let notify = self.notification(&msg, to_me);
                let prefetch = self.prefetch(&msg);
                return Task::batch([notify, prefetch, self.on_new_message(msg)]);
            }
            Update::MessageSendSucceeded(u) => {
                let msg = Msg::from(u.message);
                self.archive_save(std::slice::from_ref(&msg));
                if let Some(open) = self.open.as_mut().filter(|o| o.id == msg.chat_id)
                    && (open.shown().keeps(&msg) || open.position(u.old_message_id).is_some())
                {
                    let msg = msg.in_thread(open.thread_root());
                    let follow = open.at_end();
                    open.edit(|msgs| {
                        msgs.retain(|m| m.id != u.old_message_id);
                        msgs.push(msg);
                    });
                    if follow {
                        open.cursor = open.msgs.len() - 1;
                        open.bottom = open.cursor;
                    }
                }
            }
            Update::MessageContent(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.id == u.chat_id)
                    && let Some(i) = open.position(u.message_id)
                    && open.msgs[i].members.len() == 1
                {
                    let msg = &mut open.msgs[i];
                    msg.media = tg::media(&u.new_content).into_iter().collect();
                    if msg.media.is_empty() {
                        msg.text = tg::content_text(&u.new_content, msg.outgoing);
                    }
                    msg.links = tg::content_links(&u.new_content, &msg.text);
                    msg.styles = tg::content_styles(&u.new_content, &msg.text);
                    msg.spoiler = tg::media_spoiler(&u.new_content);
                    msg.preview = tg::link_preview(&u.new_content);
                    let msg = msg.clone();
                    self.archive_save(&[msg]);
                }
            }
            // Listened on another device, or by the recipient.
            Update::MessageContentOpened(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.id == u.chat_id)
                    && let Some(i) = open.position(u.message_id)
                {
                    open.msgs[i].media.iter_mut().for_each(Media::set_listened);
                }
            }
            Update::MessageInteractionInfo(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.id == u.chat_id)
                    && let Some(i) = open.position(u.message_id)
                {
                    let reactions = tg::reactions(u.interaction_info.as_ref());
                    let msg = &mut open.msgs[i];
                    // Another part of an album without reactions doesn't clear them.
                    if msg.id == u.message_id || !reactions.is_empty() {
                        msg.reactions = reactions;
                    }
                    let comments = tg::comments(u.interaction_info.as_ref());
                    if msg.id == u.message_id || comments.is_some() {
                        msg.comments = comments;
                    }
                }
            }
            Update::MessageEdited(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.id == u.chat_id)
                    && let Some(i) = open.position(u.message_id)
                {
                    open.msgs[i].edited = true;
                }
            }
            // `from_cache` only drops TDLib's local copy: not a deletion.
            Update::DeleteMessages(u) if u.is_permanent && !u.from_cache => {
                if let Some(archive) = &self.archive {
                    archive.mark_deleted(u.chat_id, u.message_ids.clone());
                }
                // Keep showing deleted messages, marked as such…
                if let Some(open) = self.open.as_mut().filter(|o| o.id == u.chat_id) {
                    let mut shown = false;
                    // Albums count as deleted once all their parts are.
                    for msg in open.msgs.iter_mut().filter(|m| m.members.iter().all(|id| u.message_ids.contains(id))) {
                        msg.deleted = true;
                        shown = true;
                    }
                    // …with their media from the archive: TDLib deletes the files.
                    if shown && let Some(archive) = self.archive.clone() {
                        let lo = u.message_ids.iter().copied().min().unwrap_or(0);
                        let hi = u.message_ids.iter().copied().max().unwrap_or(0);
                        let chat_id = u.chat_id;
                        return Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    archive.flush();
                                    archive.deleted_between(chat_id, lo, hi)
                                })
                                .await
                                .map_err(|e| e.to_string())?
                            },
                            move |result| Message::Archived { chat_id, result },
                        );
                    }
                }
            }
            Update::File(u) => {
                let file = u.file;
                if file.local.is_downloading_completed
                    && let Some((chat_id, msg_id)) = self.file_owner.get(&file.id).copied()
                    && let Some(archive) = &self.archive
                {
                    archive.link(chat_id, msg_id, file.id, file.local.path);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn on_auth(&mut self, state: AuthorizationState) -> Task<Message> {
        use AuthorizationState as S;
        let prompt = |app: &mut Self, auth: Auth| {
            app.auth = auth;
            app.mode = Mode::Insert;
            app.input.clear();
        };
        match state {
            S::WaitTdlibParameters => {
                return Task::perform(tg::set_parameters(self.cfg.clone()), Message::Done);
            }
            S::WaitPhoneNumber => prompt(self, Auth::Phone),
            S::WaitCode(_) => prompt(self, Auth::Code),
            S::WaitPassword(p) => prompt(self, Auth::Password { hint: p.password_hint }),
            S::WaitOtherDeviceConfirmation(x) => {
                self.auth = Auth::Info(format!("Confirm this login on another device: {}", x.link));
            }
            S::Ready => {
                self.auth = Auth::Ready;
                self.mode = Mode::Normal;
                self.input.clear();
                self.status = None;
                return Task::batch([self.load_more_chats(), weekly_cache_clean()]);
            }
            S::LoggingOut | S::Closing => self.auth = Auth::Info("Closing…".into()),
            S::Closed => self.auth = Auth::Info("Session closed".into()),
            _ => {
                self.auth = Auth::Info(
                    "This login step (email/registration) is not supported yet — \
                     finish it in another client first"
                        .into(),
                );
            }
        }
        Task::none()
    }

    fn on_new_message(&mut self, msg: Msg) -> Task<Message> {
        // Scheduled messages live in their own list, not in the history.
        if msg.scheduled {
            return Task::none();
        }
        let (width, height) = self.viewport();
        let Some(open) = self.open.as_mut().filter(|o| o.id == msg.chat_id) else {
            return Task::none();
        };
        if !open.shown().keeps(&msg) {
            return Task::none();
        }
        let msg = msg.in_thread(open.thread_root());
        let (chat_id, id, incoming) = (msg.chat_id, msg.id, !msg.outgoing);
        // Still pending: sent from here, not from another device.
        let sent_here = msg.sending;
        if !open.at_latest {
            // Sending from old history goes back to the newest messages, like
            // other clients; other messages load when scrolling down.
            return if sent_here { self.to_latest() } else { Task::none() };
        }
        if let Some(total) = open.total.as_mut() {
            *total += 1;
        }
        let cursor_at_end = open.msgs.is_empty() || open.at_end();
        let bottom_shown = open.bottom + 1 >= open.msgs.len();
        open.edit(|msgs| msgs.push(msg));
        let last = open.msgs.len() - 1;
        if cursor_at_end || sent_here {
            open.cursor = last;
            open.bottom = last;
            self.reset_tall();
        } else if bottom_shown {
            // The newest message was on screen: keep it there, and the cursor
            // with it if it would scroll away.
            open.bottom = last;
            open.cursor = open.cursor.max(view::top_for_bottom(&open.msgs, last, width, height));
        }
        if incoming { self.view_messages(chat_id, vec![id]) } else { Task::none() }
    }

    /// Back to the newest messages: from old history by reloading them.
    fn to_latest(&mut self) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            return Task::none();
        };
        self.focus = Focus::Messages;
        if !open.at_latest {
            open.msgs.clear();
            open.at_latest = true;
            open.exhausted = false;
            open.loading = false;
            return self.load_history(false);
        }
        self.jump(usize::MAX)
    }

    /// The user can see the window (focused; not minimized, on another
    /// workspace or closed to the tray).
    fn looking(&self) -> bool {
        self.focused && self.window.is_some()
    }

    /// Mark messages read, but only while the user is looking: a chat left
    /// open in a window in the background stays unread (and notifies).
    fn view_messages(&self, chat_id: i64, ids: Vec<i64>) -> Task<Message> {
        let preview = self.open.as_ref().is_some_and(|o| o.id == chat_id && o.preview);
        if self.looking() && !preview && !ids.is_empty() {
            Task::perform(tg::view_messages(chat_id, ids), Message::Done)
        } else {
            Task::none()
        }
    }

    /// Back at the window: read what is on screen in the open chat.
    fn view_visible(&self) -> Task<Message> {
        let Some(open) = self.open.as_ref().filter(|o| !o.msgs.is_empty()) else {
            return Task::none();
        };
        let (width, height) = self.viewport();
        let bottom = open.bottom.min(open.msgs.len() - 1);
        let top = view::top_for_bottom(&open.msgs, bottom, width, height);
        let ids: Vec<i64> = open.msgs[top..=bottom]
            .iter()
            .filter(|m| !m.outgoing && !m.deleted)
            .flat_map(|m| m.members.iter().copied())
            .collect();
        self.view_messages(open.id, ids)
    }

    fn on_history(&mut self, chat_id: i64, newer: bool, result: Result<Vec<Msg>, String>) -> Task<Message> {
        let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id) else {
            return Task::none();
        };
        open.loading = false;
        let batch = match result {
            Ok(batch) => batch,
            Err(e) => {
                self.error(e);
                return Task::none();
            }
        };
        self.archive_save(&batch);
        let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id) else {
            return Task::none();
        };
        if newer {
            // The page includes the message it starts from.
            if batch.len() <= 1 {
                open.at_latest = true;
            }
            let ids: Vec<i64> = batch.iter().map(|m| m.id).collect();
            let lo = open.msgs.last().map_or(0, |m| m.id);
            let hi = if open.at_latest { i64::MAX } else { ids.iter().copied().max().unwrap_or(lo) };
            open.edit(|msgs| msgs.extend(batch));
            self.scroll_messages();
            return Task::batch([
                self.view_messages(chat_id, ids),
                self.load_archived(chat_id, lo, hi),
            ]);
        }
        if batch.is_empty() {
            open.exhausted = true;
            // All comments are here: the post goes on top.
            if let Some(roots) = open.thread.as_mut().map(|t| std::mem::take(&mut t.roots))
                && !roots.is_empty()
            {
                open.edit(|msgs| msgs.extend(roots));
                self.scroll_messages();
            }
            return Task::none();
        }

        let ids: Vec<i64> = batch.iter().map(|m| m.id).collect();
        let first_load = open.msgs.is_empty();
        let lo = ids.iter().copied().min().unwrap_or(0);
        let hi = match open.msgs.first() {
            Some(first) => first.id,
            None if open.at_latest => i64::MAX,
            None => ids.iter().copied().max().unwrap_or(lo),
        };
        open.edit(|msgs| msgs.extend(batch));
        if first_load {
            open.cursor = open.msgs.len() - 1;
            open.bottom = open.cursor;
        }
        let want_more = open.msgs.len() < MIN_HISTORY || open.cursor < PREFETCH;
        self.scroll_messages();

        let mut tasks = vec![
            self.view_messages(chat_id, ids),
            self.load_archived(chat_id, lo, hi),
        ];
        if want_more {
            tasks.push(self.load_history(true));
        }
        Task::batch(tasks)
    }

    fn refresh_list(&mut self) {
        if !self.list_dirty {
            return;
        }
        self.list_dirty = false;

        self.folder_unread.clear();
        for chat in self.chats.values().filter(|c| c.unread > 0) {
            for key in chat.positions.keys() {
                *self.folder_unread.entry(*key).or_default() += 1;
            }
        }

        let list = self.current_list();
        let filter = self.filter.to_lowercase();
        let mut chats: Vec<(i64, i64)> = self
            .chats
            .values()
            .filter(|c| filter.is_empty() || c.title.to_lowercase().contains(&filter))
            .filter_map(|c| Some((c.order(list)?, c.id)))
            .collect();
        chats.sort_unstable_by_key(|&(order, _)| std::cmp::Reverse(order));
        self.list = chats.into_iter().map(|(_, id)| id).collect();
        if !self.filter.is_empty() {
            for &id in &self.global.chats {
                if self.chats.contains_key(&id) && !self.list.contains(&id) {
                    self.list.push(id);
                }
            }
        }
        if self.hit.is_none() && self.selected_index().is_none() {
            self.selected = self.list.first().copied();
        }
        self.scroll_chats();
    }

    /// Filter the chat list; a while later the server is searched too.
    fn set_filter(&mut self, filter: String) -> Task<Message> {
        self.filter = filter;
        self.list_dirty = true;
        self.selected = None;
        self.hit = None;
        let query = self.filter.trim().to_owned();
        if self.global.query != query {
            self.global = Global::default();
        }
        self.refresh_list();
        if query.chars().count() < GLOBAL_MIN_CHARS || self.forward.is_some() || self.auth != Auth::Ready {
            return Task::none();
        }
        Task::perform(tokio::time::sleep(GLOBAL_DELAY), move |()| Message::GlobalSearch(query.clone()))
    }

    fn clear_filter(&mut self) {
        let _ = self.set_filter(String::new());
    }

    fn search_global(&mut self, query: String) -> Task<Message> {
        if self.filter.trim() != query || self.global.query == query {
            return Task::none();
        }
        self.global = Global {
            query: query.clone(),
            loading: true,
            ..Default::default()
        };
        Task::perform(tg::search_global(query.clone()), move |result| Message::GlobalFound {
            query: query.clone(),
            result,
        })
    }

    /// `:posts [query]`: posts of public channels too, below the other
    /// results. Only on request: Telegram allows a few free a day.
    fn search_posts(&mut self, arg: Option<&str>) -> Task<Message> {
        if let Some(query) = arg.filter(|q| q.trim() != self.filter.trim()) {
            let _ = self.set_filter(query.to_owned());
        }
        let query = self.filter.trim().to_owned();
        if query.chars().count() < GLOBAL_MIN_CHARS || self.forward.is_some() {
            self.error("Search public posts for what? /query, then :posts (or :posts query)");
            return Task::none();
        }
        self.focus = Focus::Chats;
        let global = self.search_global(query.clone());
        self.info(format!("Searching public posts for {query}…"));
        let posts = Task::perform(tg::search_posts(query.clone()), move |result| Message::PostsFound {
            query: query.clone(),
            result,
        });
        Task::batch([global, posts])
    }

    /// Found messages not shown yet.
    fn new_hits(&self, msgs: Vec<Msg>) -> Vec<Msg> {
        let shown = |m: &Msg| {
            let same = |x: &Msg| x.chat_id == m.chat_id && x.id == m.id;
            self.global.msgs.iter().any(same) || self.global.posts.iter().any(same)
        };
        msgs.into_iter().filter(|m| !m.scheduled && !shown(m)).collect()
    }

    fn on_global(&mut self, query: String, result: Result<tg::Global, String>) -> Task<Message> {
        if self.global.query != query {
            return Task::none();
        }
        self.global.loading = false;
        match result {
            Ok(found) => {
                self.global.chats = found.chats;
                self.global.msgs = self.new_hits(found.msgs);
                self.global.offset = found.offset;
                // The cursor on a post stays on it.
                if let Some(hit) = self.hit.as_mut() {
                    *hit += self.global.msgs.len();
                }
                self.list_dirty = true;
                self.refresh_list();
            }
            Err(e) => self.error(format!("Search failed: {e}")),
        }
        Task::none()
    }

    /// The cursor came near the last message found: the next page.
    fn more_hits(&mut self) -> Task<Message> {
        let global = &mut self.global;
        if global.loading || global.offset.is_empty() {
            return Task::none();
        }
        global.loading = true;
        let (query, offset) = (global.query.clone(), global.offset.clone());
        Task::perform(tg::search_messages(query.clone(), offset), move |result| Message::GlobalMore {
            query: query.clone(),
            result,
        })
    }

    fn on_more_hits(&mut self, query: String, result: Result<(Vec<Msg>, String), String>) {
        if self.global.query != query {
            return;
        }
        self.global.loading = false;
        match result {
            Ok((msgs, offset)) => {
                let msgs = self.new_hits(msgs);
                if let Some(hit) = self.hit.as_mut().filter(|h| **h >= self.global.msgs.len()) {
                    *hit += msgs.len();
                }
                self.global.msgs.extend(msgs);
                self.global.offset = offset;
            }
            Err(e) => {
                self.global.offset.clear();
                self.error(format!("Search failed: {e}"));
            }
        }
    }

    fn on_posts(&mut self, query: String, result: Result<tg::Posts, String>) {
        if self.filter.trim() != query {
            return;
        }
        match result {
            Ok(posts) if posts.exceeded => {
                let hours = (posts.next_in + 3599) / 3600;
                self.error(format!("No free searches of public posts left today (next in {hours} h)"));
            }
            Ok(posts) => {
                let found = self.new_hits(posts.msgs);
                self.info(match posts.left {
                    Some(left) => format!("{} public posts found; {left} free searches left today", tg::fmt_count(found.len())),
                    None => format!("{} public posts found", tg::fmt_count(found.len())),
                });
                self.global.posts.extend(found);
            }
            Err(e) => self.error(format!("Public post search failed: {e}")),
        }
    }

    fn switch_folder(&mut self, index: usize) -> Task<Message> {
        if index >= self.folders.len() || index == self.folder {
            return Task::none();
        }
        if let Some(id) = self.selected {
            self.folder_selection.insert(self.current_list(), id);
        }
        self.folder = index;
        self.folder_chosen = true;
        self.selected = self.folder_selection.get(&self.current_list()).copied();
        self.chat_top = 0;
        // Moving through the folder pane keeps it focused.
        if self.focus != Focus::Folders {
            self.focus = Focus::Chats;
        }
        self.list_dirty = true;
        self.refresh_list();
        self.load_more_chats()
    }

    fn cycle_folder(&mut self, step: isize) -> Task<Message> {
        let n = self.folders.len() as isize;
        let index = (self.folder as isize + step).rem_euclid(n) as usize;
        self.switch_folder(index)
    }

    // ---- requests ------------------------------------------------------

    fn load_more_chats(&mut self) -> Task<Message> {
        if self.auth != Auth::Ready {
            return Task::none();
        }
        let list = self.current_list();
        let state = self.list_load.entry(list).or_default();
        if state.loading || state.exhausted {
            return Task::none();
        }
        state.loading = true;
        Task::perform(tg::load_chats(list, CHATS_PAGE), move |result| {
            Message::ChatsLoaded(list, result)
        })
    }

    fn load_history(&mut self, older: bool) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            return Task::none();
        };
        if open.loading || (older && open.exhausted) || open.pins_only() {
            return Task::none();
        }
        open.loading = true;
        let chat_id = open.id;
        let from = if older { open.msgs.first().map_or(0, |m| m.id) } else { 0 };
        if let Some(tag) = open.tag.clone() {
            return Task::perform(tg::tagged(chat_id, tag.clone(), from, HISTORY_PAGE), move |result| {
                Message::History { chat_id, tag: Some(tag.clone()), thread: 0, newer: false, result }
            });
        }
        if let Some(thread) = open.thread.as_ref().map(|t| t.root) {
            return Task::perform(tg::thread_history(chat_id, thread, from, HISTORY_PAGE), move |result| {
                Message::History { chat_id, tag: None, thread, newer: false, result }
            });
        }
        Task::perform(tg::history(chat_id, from, 0, HISTORY_PAGE), move |result| {
            Message::History { chat_id, tag: None, thread: 0, newer: false, result }
        })
    }

    /// After jumping into the past, load the messages after the newest one.
    fn load_newer(&mut self) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            return Task::none();
        };
        if open.loading || open.at_latest {
            return Task::none();
        }
        let Some(from) = open.msgs.last().map(|m| m.id) else {
            return Task::none();
        };
        open.loading = true;
        let chat_id = open.id;
        Task::perform(tg::history(chat_id, from, -(HISTORY_PAGE - 1), HISTORY_PAGE), move |result| {
            Message::History { chat_id, tag: None, thread: 0, newer: true, result }
        })
    }

    /// Put the cursor on a message, loading the history around it if needed.
    fn jump_to_message(&mut self, id: i64) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            return Task::none();
        };
        self.focus = Focus::Messages;
        if let Some(i) = open.position(id) {
            open.cursor = i;
            self.scroll_messages();
            return Task::none();
        }
        // Comments come in order only: not loaded yet means not there.
        if open.thread.is_some() {
            self.error("Not among the loaded comments (k loads older ones)");
            return Task::none();
        }
        // Not among the tagged messages: show it among all of them.
        let count = open.tag.is_some().then(|| self.untag()).unwrap_or_else(Task::none);
        let Some(open) = self.open.as_mut() else {
            return count;
        };
        let chat_id = open.id;
        open.loading = true;
        let half = HISTORY_PAGE / 2;
        let window = Task::perform(tg::history(chat_id, id, -half, HISTORY_PAGE), move |result| {
            Message::Window { chat_id, target: id, result }
        });
        Task::batch([count, window])
    }

    /// `#tag` clicked (or `gx`, `/#tag`): the open chat shows only the
    /// messages with it, newest at the bottom; Esc brings back the rest.
    fn show_tag(&mut self, tag: String) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            self.error("Open a chat first (Enter)");
            return Task::none();
        };
        let chat_id = open.id;
        open.retag(Some(tag.clone()));
        self.focus = Focus::Messages;
        self.visual = None;
        self.found = None;
        self.reset_tall();
        self.info(format!("Only messages with {tag} (Esc shows all)"));
        let count = Task::perform(tg::search(chat_id, tag.clone()), move |result| Message::Count {
            chat_id,
            tag: Some(tag.clone()),
            result: result.map(|(_, total)| total),
        });
        Task::batch([count, self.load_history(false)])
    }

    /// `gP`: the open chat shows only its pinned messages, the cursor on
    /// the one the bar shows; Esc (or Enter) shows all, at the one under it.
    fn list_pinned(&mut self) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            self.error("Open a chat first (Enter)");
            return Task::none();
        };
        if open.pins.is_empty() {
            self.error("No pinned messages");
            return Task::none();
        }
        if open.thread.is_some() {
            self.error("Not in the comments (Esc back to the post first)");
            return Task::none();
        }
        let at = open.shown_pin().map(|(_, p)| p.id);
        open.fill_pins(at);
        self.focus = Focus::Messages;
        self.visual = None;
        self.found = None;
        self.reset_tall();
        self.scroll_messages();
        self.info("Pinned messages: Enter or Esc goes to the one under the cursor in the chat");
        self.scroll_tall()
    }

    /// Back to all messages; returns the task counting them.
    fn untag(&mut self) -> Task<Message> {
        let Some(open) = self.open.as_mut() else {
            return Task::none();
        };
        let chat_id = open.id;
        open.retag(None);
        self.reset_tall();
        Task::perform(tg::message_count(chat_id), move |result| Message::Count {
            chat_id,
            tag: None,
            result,
        })
    }

    /// The message has a spoiler not shown yet.
    pub fn hides(&self, m: &Msg) -> bool {
        (m.spoiler || m.styles.iter().any(|s| s.style == tg::Style::Spoiler)) && !self.revealed.contains(&m.id)
    }

    /// Show the spoilers of a message; false if it had none hidden.
    fn reveal(&mut self, msg_id: i64) -> bool {
        let hidden = self
            .open
            .as_ref()
            .and_then(|o| o.position(msg_id).map(|i| &o.msgs[i]))
            .is_some_and(|m| self.hides(m));
        if hidden {
            self.revealed.insert(msg_id);
        }
        hidden
    }

    /// Esc in a `#tag` view: all messages, at the one under the cursor.
    fn leave_tag(&mut self) -> Task<Message> {
        let at = self.open.as_ref().and_then(OpenChat::current).map(|m| m.id);
        let count = self.untag();
        let load = match at {
            Some(id) => self.jump_to_message(id),
            None => self.load_history(false),
        };
        self.info("All messages");
        Task::batch([count, load])
    }

    fn on_window(&mut self, chat_id: i64, target: i64, result: Result<Vec<Msg>, String>) -> Task<Message> {
        let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id && o.tag.is_none()) else {
            return Task::none();
        };
        open.loading = false;
        let batch = match result {
            Ok(batch) if !batch.is_empty() => batch,
            Ok(_) => return Task::none(),
            Err(e) => {
                self.error(e);
                return Task::none();
            }
        };
        self.archive_save(&batch);
        let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id) else {
            return Task::none();
        };
        let ids: Vec<i64> = batch.iter().map(|m| m.id).collect();
        let lo = ids.iter().copied().min().unwrap_or(0);
        let hi = ids.iter().copied().max().unwrap_or(0);
        // Replace what was loaded: the window is not contiguous with it.
        open.msgs.clear();
        open.exhausted = false;
        open.at_latest = false;
        open.edit(|msgs| msgs.extend(batch));
        open.cursor = open.position(target).unwrap_or(0);
        open.bottom = open.cursor;
        self.reset_tall();
        self.scroll_messages();
        Task::batch([
            self.view_messages(chat_id, ids),
            self.load_archived(chat_id, lo, hi),
            self.scroll_tall(),
        ])
    }

    fn show_match(&mut self) {
        if let Some(found) = self.found.as_ref().filter(|f| f.pins) {
            let n = found.ids.len();
            self.info(format!("📌 Pinned message {} of {n}   n/N older/newer", n - found.index));
        } else if let Some(found) = &self.found {
            let text = format!(
                "/{}  match {} of {}",
                found.query,
                found.index + 1,
                found.total.max(found.ids.len() as i32)
            );
            self.info(text);
        }
    }

    /// `n` goes to older matches, `N` to newer ones.
    fn next_match(&mut self, step: isize) -> Task<Message> {
        let Some(found) = self.found.as_mut() else {
            self.error("No previous search (/ in the message pane)");
            return Task::none();
        };
        let Some(index) = found.index.checked_add_signed(step).filter(|&i| i < found.ids.len()) else {
            match (found.pins, step > 0) {
                (true, true) => self.error("No older pinned messages (gp goes round to the newest)"),
                (true, false) => self.error("No newer pinned messages"),
                _ => self.error("No more matches"),
            }
            return Task::none();
        };
        found.index = index;
        let id = found.ids[index];
        self.show_match();
        self.jump_to_message(id)
    }

    /// `gp`, a click on the pinned bar: to the pinned message it shows, or
    /// when already there to the one before it (round to the newest), like
    /// Telegram; then n/N go through the pinned messages.
    fn jump_pinned(&mut self) -> Task<Message> {
        let Some(open) = self.open.as_ref() else {
            self.error("Open a chat first (Enter)");
            return Task::none();
        };
        let Some((mut index, pin)) = open.shown_pin() else {
            self.error("No pinned messages");
            return Task::none();
        };
        if open.current().is_some_and(|m| m.contains(pin.id)) {
            index = (index + 1) % open.pins.len();
        }
        let ids: Vec<i64> = open.pins.iter().map(|p| p.id).collect();
        let id = ids[index];
        let total = ids.len() as i32;
        self.found = Some(Found { query: String::new(), pins: true, ids, total, index });
        self.show_match();
        self.jump_to_message(id)
    }

    fn download(&mut self, file_id: i32, kind: Kind, size: Size, priority: i32) -> Option<Task<Message>> {
        self.media.request(file_id, kind, size).then(|| {
            Task::perform(tg::download(file_id, priority), move |result| {
                Message::Downloaded { file_id, result }
            })
        })
    }

    /// Download avatars and media that are currently on screen.
    fn fetch_visible(&mut self) -> Task<Message> {
        if self.auth != Auth::Ready {
            return Task::none();
        }
        let hits = (0..self.global.hits()).filter_map(|i| self.global.hit(i)).map(|m| &m.chat_id);
        let mut wanted: Vec<(i32, Kind, Size, i32)> = self
            .list
            .iter()
            .chain(hits)
            .skip(self.chat_top)
            .take(view::chat_rows(self.size))
            .filter_map(|id| self.chats.get(id)?.avatar)
            .map(|file| (file, Kind::Image, Size::ZERO, media::PRIORITY_AVATAR))
            .collect();

        if let Some(open) = &self.open
            && !open.msgs.is_empty()
        {
            let (width, height) = self.viewport();
            let top = view::top_for_bottom(&open.msgs, open.bottom, width, height);
            if let Some(file) = self.chats.get(&open.id).and_then(|c| c.avatar) {
                wanted.push((file, Kind::Image, Size::ZERO, media::PRIORITY_AVATAR));
            }
            let first = top.saturating_sub(1);
            for msg in &open.msgs[first..=open.bottom] {
                if let Some(file) = self.sender_avatar(msg.sender) {
                    wanted.push((file, Kind::Image, Size::ZERO, media::PRIORITY_AVATAR));
                }
                let sizes = view::media_sizes(&msg.media, width);
                for (media, size) in msg.media.iter().zip(sizes) {
                    if let Some(file) = media.inline_file() {
                        wanted.push((file, Kind::of(media), size, media::PRIORITY_INLINE));
                    }
                }
                if let Some(photo) = msg.preview.as_ref().and_then(|p| p.photo) {
                    wanted.push((photo.id, Kind::Image, Size::ZERO, media::PRIORITY_INLINE));
                }
            }
        }
        if let Some(Media::Photo { full, .. }) = self.viewer_media() {
            wanted.push((full.id, Kind::Image, Size::ZERO, media::PRIORITY_VIEWER));
        }

        let visible: HashSet<i32> = wanted.iter().map(|w| w.0).collect();
        self.media.retain_visible(&visible);
        self.animating = wanted.iter().any(|w| matches!(w.1, Kind::Tgs | Kind::Webm));

        Task::batch(
            wanted
                .into_iter()
                .filter_map(|(file, kind, size, priority)| self.download(file, kind, size, priority)),
        )
    }

    fn open_chat(&mut self, id: i64) -> Task<Message> {
        self.open_chat_at(id, None)
    }

    /// Open a chat at its latest messages, or around message `at`.
    fn open_chat_at(&mut self, id: i64, at: Option<i64>) -> Task<Message> {
        self.focus = Focus::Messages;
        let mut tasks = Vec::new();
        if let Some(prev) = self.open.take() {
            if prev.id == id && prev.thread.is_none() {
                self.open = Some(prev);
                return at.map_or_else(Task::none, |msg| self.jump_to_message(msg));
            }
            tasks.push(Task::perform(tg::close_chat(prev.id), Message::Done));
        }
        tasks.push(self.switch_chat(OpenChat::new(id)));
        tasks.push(Task::perform(tg::message_count(id), move |result| Message::Count {
            chat_id: id,
            tag: None,
            result,
        }));
        tasks.push(Self::load_pinned(id));
        self.reset_tall();
        tasks.push(match at {
            Some(msg) => self.jump_to_message(msg),
            None => self.load_history(false),
        });
        Task::batch(tasks)
    }

    /// Show `open` in place of the open chat (closed by the caller).
    fn switch_chat(&mut self, open: OpenChat) -> Task<Message> {
        let id = open.id;
        self.open = Some(open);
        self.visual = None;
        self.close_viewer();
        self.compose = Compose::New;
        self.input.clear();
        self.found = None;
        Task::perform(tg::open_chat(id), Message::Done)
    }

    /// `gc` on a channel post: find its comments.
    fn open_comments(&mut self) -> Task<Message> {
        let Some(open) = self.open.as_ref().filter(|o| o.thread.is_none()) else {
            return Task::none();
        };
        let Some(parts) = open.current().filter(|m| m.comments.is_some()).map(|m| m.members.clone()) else {
            self.error("No comments under this message");
            return Task::none();
        };
        let (channel, post) = (open.id, parts[0]);
        self.info("Loading comments…");
        Task::perform(tg::thread(channel, parts), move |result| Message::ThreadOpened { channel, post, result })
    }

    /// The comments of `post`, in the discussion group; Esc comes back.
    fn open_thread(&mut self, channel: i64, post: i64, thread: tg::Thread) -> Task<Message> {
        let mut tasks = Vec::new();
        if let Some(prev) = self.open.take() {
            tasks.push(Task::perform(tg::close_chat(prev.id), Message::Done));
        }
        self.jumps.push((channel, post));
        let mut open = OpenChat::new(thread.chat_id);
        open.total = Some(thread.count);
        open.thread = Some(CommentThread {
            channel,
            post,
            root: thread.root,
            roots: thread.roots,
        });
        self.focus = Focus::Messages;
        tasks.push(self.switch_chat(open));
        self.reset_tall();
        tasks.push(self.load_history(false));
        self.info("Comments: i writes one, r replies, Esc back to the post");
        Task::batch(tasks)
    }

    /// Esc in the comments: back to the post in the channel.
    fn leave_thread(&mut self) -> Task<Message> {
        let Some((channel, post)) = self.open.as_ref().and_then(|o| o.thread.as_ref()).map(|t| (t.channel, t.post)) else {
            return Task::none();
        };
        if self.jumps.last() == Some(&(channel, post)) {
            self.jumps.pop();
        }
        self.open_chat_at(channel, Some(post))
    }

    /// A voice message playing in a chat other than the open one.
    pub fn playing_elsewhere(&self) -> Option<&VoicePlayback> {
        self.voice.as_ref().filter(|v| self.open.as_ref().is_none_or(|o| o.id != v.chat_id))
    }

    fn send(&mut self) -> Task<Message> {
        self.send_with(SendOptions::default())
    }

    /// Send (or save the edit of) the composed message.
    fn send_with(&mut self, mut options: SendOptions) -> Task<Message> {
        if self.forward.is_some() {
            return match self.selected.or(self.forward_marks.first().copied()) {
                Some(chat) => self.forward_to(chat, options),
                None => {
                    self.error("Pick a chat to forward to (j/k)");
                    Task::none()
                }
            };
        }
        let Some(open) = &self.open else {
            return Task::none();
        };
        if !self.attachments.is_empty() && !matches!(self.compose, Compose::Edit(_)) {
            let paths = std::mem::take(&mut self.attachments);
            options.thread = open.thread_root();
            return self.send_files(paths, options);
        }
        let text = self.input.as_str().trim();
        if text.is_empty() {
            self.error("Nothing to send: write a message first (i)");
            return Task::none();
        }
        let (chat_id, text) = (open.id, text.to_owned());
        options.thread = open.thread_root();
        self.input.clear();
        let compose = std::mem::replace(&mut self.compose, Compose::New);
        match compose {
            Compose::Edit(id) => {
                self.mode = Mode::Normal;
                return Task::perform(tg::edit_text(chat_id, id, text), Message::Done);
            }
            Compose::Reply(id) => options.reply_to = Some(id),
            Compose::New => {}
        }
        if let Some(at) = options.at {
            self.info(format!("Scheduled for {}", view::fmt_datetime(at)));
        } else if options.silent {
            self.info("Sent without sound");
        }
        Task::perform(tg::send_text(chat_id, text, options), Message::Done)
    }

    fn pick_files(&mut self) -> Task<Message> {
        if self.open.is_none() {
            self.error("Open a chat first (Enter)");
            return Task::none();
        }
        Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Attach files")
                    .pick_files()
                    .await
                    .map(|files| files.iter().map(|f| f.path().to_path_buf()).collect())
                    .unwrap_or_default()
            },
            Message::Picked,
        )
    }

    /// Stage files for the next message; Enter sends them, the composed
    /// text becomes their caption.
    fn attach(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.open.is_none() {
            self.error("Open a chat first (Enter)");
            return;
        }
        if let Some(missing) = paths.iter().find(|p| !p.is_file()) {
            self.error(format!("No such file: {}", missing.display()));
            return;
        }
        if matches!(self.compose, Compose::Edit(_)) {
            self.compose = Compose::New;
            self.input.clear();
        }
        self.attachments.extend(paths);
        self.focus = Focus::Messages;
        self.mode = Mode::Insert;
        self.info("Attached: Enter sends it, the text becomes its caption");
        self.scroll_messages();
    }

    /// Send files; the composed text becomes the caption of the first one.
    fn send_files(&mut self, paths: Vec<PathBuf>, mut options: SendOptions) -> Task<Message> {
        let Some(chat_id) = self.open.as_ref().map(|o| o.id) else {
            return Task::none();
        };
        if paths.is_empty() {
            return Task::none();
        }
        if let Some(missing) = paths.iter().find(|p| !p.is_file()) {
            self.error(format!("No such file: {}", missing.display()));
            return Task::none();
        }
        if let Compose::Reply(id) = self.compose {
            options.reply_to = Some(id);
        }
        let caption = match self.compose {
            Compose::Edit(_) => String::new(),
            _ => self.input.take().trim().to_owned(),
        };
        self.compose = Compose::New;
        self.mode = Mode::Normal;
        self.info(match paths.len() {
            1 => "Sending the file…".to_owned(),
            n => format!("Sending {} files…", tg::fmt_count(n)),
        });
        Task::perform(tg::send_files(chat_id, paths, caption, options), Message::Done)
    }

    /// `:autostart` shows the state, `:autostart on|off` changes it.
    fn autostart(&mut self, arg: Option<&str>) -> Task<Message> {
        let on = match arg.map(str::trim) {
            None | Some("") => None,
            Some("on" | "1" | "true" | "yes") => Some(true),
            Some("off" | "0" | "false" | "no") => Some(false),
            Some(other) => {
                self.error(format!("Usage: :autostart [on|off], not {other}"));
                return Task::none();
            }
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || match on {
                    None => Ok(autostart::current().map_or("Autostart is off — :autostart on", autostart::Method::status).to_owned()),
                    Some(true) => autostart::enable().map(|method| method.enabled().to_owned()),
                    Some(false) => autostart::disable().map(|()| "Autostart off".to_owned()),
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::Autostart,
        )
    }

    fn export(&mut self, arg: Option<&str>) -> Task<Message> {
        let Some(open) = &self.open else {
            self.error("Open a chat first (Enter)");
            return Task::none();
        };
        let chat_id = open.id;
        let title = self.chats.get(&chat_id).map_or("chat".to_owned(), |c| c.title.clone());
        let path = arg.map_or_else(|| export::default_path(&title), expand_home);
        let mut names: HashMap<Sender, String> =
            self.users.iter().map(|(id, u)| (Sender::User(*id), u.name.clone())).collect();
        names.extend(self.chats.iter().map(|(id, c)| (Sender::Chat(*id), c.title.clone())));
        let archive = self.archive.clone();
        self.info(format!("Exporting {title}… (big chats take a while)"));
        Task::perform(
            async move {
                let msgs = export::collect(chat_id, archive).await?;
                let count = msgs.len();
                let html = export::html(&title, &msgs, &names);
                tokio::task::spawn_blocking(move || export::write_pdf(&html, &path).map(|()| (path, count)))
                    .await
                    .map_err(|e| e.to_string())?
            },
            Message::Exported,
        )
    }

    fn submit_auth(&mut self) -> Task<Message> {
        let value = self.input.as_str().trim().to_owned();
        if value.is_empty() {
            return Task::none();
        }
        self.status = None;
        match self.auth {
            Auth::Phone => Task::perform(tg::set_phone(value), Message::Done),
            Auth::Code => Task::perform(tg::check_code(value), Message::Done),
            Auth::Password { .. } => Task::perform(tg::check_password(value), Message::Done),
            _ => Task::none(),
        }
    }

    // ---- media playback ------------------------------------------------

    fn play_download(file_id: i32) -> Task<Message> {
        Task::perform(tg::download(file_id, media::PRIORITY_VIEWER), move |result| {
            Message::PlayFile { file_id, result }
        })
    }

    fn on_play_file(&mut self, file_id: i32, result: Result<String, String>) -> Task<Message> {
        let path = match result {
            Ok(path) => path,
            Err(e) => {
                self.error(e);
                return Task::none();
            }
        };
        if self.voice.as_ref().is_none_or(|v| v.file_id != file_id) {
            return Task::none();
        }
        let resumed = self.resume.remove(&file_id).map_or(Duration::ZERO, |at| at.saturating_sub(RESUME_BACK));
        let chosen = self.voice.as_ref().map_or(Duration::ZERO, |v| v.start);
        let start = if chosen > Duration::ZERO { chosen } else { resumed };
        if chosen == Duration::ZERO && start > Duration::ZERO {
            self.info(format!("Going on from {}", tg::fmt_duration(start.as_secs() as i32)));
        }
        if let Some(voice) = self.voice.as_mut() {
            match Audio::open(&path, self.speeds.get(voice.kind), self.volume, start) {
                Ok(audio) => voice.audio = Some(audio),
                Err(e) => {
                    self.voice = None;
                    self.error(e);
                }
            }
        }
        Task::none()
    }

    /// Enter / click on a media item: view photos and videos, play voice
    /// and music.
    fn activate_media(&mut self, msg_id: i64, item: usize) -> Task<Message> {
        let Some(media) = self.open.as_ref().and_then(|o| o.media(Viewer { msg_id, item })) else {
            return Task::none();
        };
        match media.audible() {
            Some(file) => {
                let kind = audio_kind(media);
                self.toggle_voice(msg_id, file, kind)
            }
            None if media.viewable() => self.open_viewer(Viewer { msg_id, item }),
            None => Task::none(),
        }
    }

    fn toggle_voice(&mut self, msg_id: i64, file_id: i32, kind: SpeedKind) -> Task<Message> {
        // By file: the tracks of a music album are one message here.
        if let Some(voice) = self.voice.as_mut().filter(|v| v.file_id == file_id) {
            if let Some(audio) = voice.audio.as_mut() {
                audio.toggle_pause();
            }
            return Task::none();
        }
        let chat_id = self.open.as_ref().map_or(0, |o| o.id);
        let sender = self
            .open
            .as_ref()
            .and_then(|o| o.position(msg_id).map(|i| o.msgs[i].sender))
            .unwrap_or(Sender::Chat(chat_id));
        self.play_audio(chat_id, msg_id, sender, file_id, kind)
    }

    fn play_audio(&mut self, chat_id: i64, msg_id: i64, sender: Sender, file_id: i32, kind: SpeedKind) -> Task<Message> {
        self.stop_voice();
        self.voice = Some(VoicePlayback {
            chat_id,
            msg_id,
            sender,
            file_id,
            kind,
            start: Duration::ZERO,
            audio: None,
        });
        Self::play_download(file_id)
    }

    /// Go to `fraction` of a voice message or music file (starting it if
    /// something else plays), or of the video in the viewer (`None`).
    fn seek_media(&mut self, target: Option<(i64, usize)>, fraction: f64) -> Task<Message> {
        let fraction = fraction.clamp(0.0, 1.0);
        let Some((msg_id, item)) = target else {
            if let Some(movie) = self.video.as_mut().and_then(VideoPlayback::movie_mut) {
                let at = movie.video.duration().mul_f64(fraction);
                let _ = movie.seek(at);
            }
            return Task::none();
        };
        let Some(media) = self.open.as_ref().and_then(|o| o.media(Viewer { msg_id, item })) else {
            return Task::none();
        };
        let Some(file) = media.audible() else {
            return Task::none();
        };
        let known = match media {
            Media::Voice { duration, .. } | Media::Audio { duration, .. } => Duration::from_secs(*duration as u64),
            _ => Duration::ZERO,
        };
        let kind = audio_kind(media);
        if let Some(voice) = self.voice.as_mut().filter(|v| v.file_id == file) {
            match voice.audio.as_mut() {
                Some(audio) => {
                    let total = audio.duration().unwrap_or(known);
                    audio.seek(total.mul_f64(fraction));
                }
                None => voice.start = known.mul_f64(fraction),
            }
            return Task::none();
        }
        let task = self.toggle_voice(msg_id, file, kind);
        if let Some(voice) = self.voice.as_mut().filter(|v| v.file_id == file) {
            voice.start = known.mul_f64(fraction);
        }
        task
    }

    /// Stop the voice message or music, remembering where it was.
    fn stop_voice(&mut self) {
        let Some(voice) = self.voice.take() else { return };
        if let Some(audio) = voice.audio.as_ref().filter(|a| !a.finished()) {
            let at = audio.position();
            if at > Duration::ZERO {
                self.resume.insert(voice.file_id, at);
            }
        }
    }

    fn open_viewer(&mut self, viewer: Viewer) -> Task<Message> {
        self.viewer = Some(viewer);
        self.viewer_focused = true;
        let video_file = match self.viewer_media() {
            Some(Media::Video { video, round, .. }) => {
                Some((video.id, if *round { SpeedKind::Round } else { SpeedKind::Video }))
            }
            _ => None,
        };
        let round_msg = self
            .viewer_media()
            .filter(|m| matches!(m, Media::Video { round: true, .. }))
            .and_then(|_| Some((self.open.as_ref()?.id, viewer.msg_id)));
        if let Some(open) = self.open.as_mut()
            && let Some(i) = open.position(viewer.msg_id)
        {
            open.cursor = i;
        }
        self.scroll_messages();
        match video_file {
            Some((file_id, _)) if self.video.as_ref().is_some_and(|v| v.file_id == file_id) => Task::none(),
            Some((file_id, kind)) => {
                // A video brings its own sound; don't talk over it.
                self.stop_voice();
                self.video = Some(VideoPlayback {
                    file_id,
                    round_msg,
                    kind,
                    movie: None,
                    speed_pending: false,
                });
                let (speed, volume) = (self.speeds.get(kind), self.volume);
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || player::open_video(file_id, speed, volume).map(Shared::new))
                            .await
                            .map_err(|e| e.to_string())?
                    },
                    move |result| Message::VideoReady { file_id, result },
                )
            }
            None => {
                self.video = None;
                Task::none()
            }
        }
    }

    fn close_viewer(&mut self) {
        let side = self.side_viewer();
        self.viewer = None;
        self.viewer_focused = false;
        self.video = None;
        // The messages get their width back.
        if side {
            self.scroll_messages();
        }
    }

    fn poll_players(&mut self) -> Task<Message> {
        let mut finished = Vec::new();
        if let Some(playback) = self.video.as_mut().filter(|p| p.speed_pending) {
            let speed = self.speeds.get(playback.kind);
            if let Some(movie) = playback.movie_mut() {
                playback.speed_pending = movie.set_speed(speed).is_err();
            }
        }
        // A round video counts as watched once it played to the end.
        if let Some(playback) = self.video.as_mut()
            && let Some(video) = playback.video()
            && (video.eos() || video.position() + Duration::from_millis(300) >= video.duration())
            && video.duration() > Duration::ZERO
            && let Some((chat_id, msg_id)) = playback.round_msg.take()
        {
            finished.push((chat_id, msg_id, false));
        }
        if let Some(recorder) = self.recorder.as_mut()
            && let Err(e) = recorder.poll()
        {
            self.stop_recording(false);
            self.error(e);
        }
        if let Some(voice) = self.voice.as_mut()
            && let Some(audio) = voice.audio.as_mut()
        {
            match audio.poll() {
                Ok(()) if audio.finished() => {
                    finished.push((voice.chat_id, voice.msg_id, voice.kind == SpeedKind::Music));
                    self.voice = None;
                }
                Ok(()) => {}
                Err(e) => {
                    self.voice = None;
                    self.error(e);
                }
            }
        }
        Task::batch(finished.into_iter().map(|(chat_id, msg_id, music)| {
            Task::batch([self.mark_listened(chat_id, msg_id), Self::fetch_next(chat_id, msg_id, music)])
        }))
    }

    /// The message right after one that finished playing.
    fn fetch_next(chat_id: i64, after: i64, music: bool) -> Task<Message> {
        Task::perform(tg::history(chat_id, after, -1, 2), move |result| Message::NextMedia {
            chat_id,
            after,
            music,
            next: result.ok().and_then(|msgs| msgs.into_iter().filter(|m| m.id > after).min_by_key(|m| m.id)),
        })
    }

    /// Continue with the next message if it is a voice or round video
    /// message too (music after music), unless something else started in
    /// the meantime.
    fn play_next(&mut self, chat_id: i64, after: i64, music: bool, next: Option<Msg>) -> Task<Message> {
        let Some(next) = next.filter(|m| !m.scheduled) else {
            return Task::none();
        };
        let same_kind = next.media.first().is_some_and(|m| (audio_kind(m) == SpeedKind::Music) == music);
        match next.media.first() {
            Some(media @ (Media::Voice { .. } | Media::Audio { .. }))
                if same_kind && self.voice.is_none() && self.video.is_none() =>
            {
                let (file, kind) = (media.audible().unwrap_or_default(), audio_kind(media));
                // Follow along when the finished message is under the cursor.
                if let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id)
                    && open.current().is_some_and(|m| m.id == after)
                    && let Some(i) = open.position(next.id)
                {
                    open.cursor = i;
                    self.scroll_messages();
                }
                self.play_audio(chat_id, next.id, next.sender, file, kind)
            }
            // Round videos play in the viewer: continue only from there.
            Some(Media::Video { round: true, .. })
                if !music
                    && self.viewer.is_some_and(|v| v.msg_id == after)
                    && self.open.as_ref().is_some_and(|o| o.id == chat_id && o.position(next.id).is_some()) =>
            {
                self.open_viewer(Viewer { msg_id: next.id, item: 0 })
            }
            // A voice message followed by a round video, in the open chat.
            Some(Media::Video { round: true, .. })
                if !music
                    && self.viewer.is_none()
                    && self.voice.is_none()
                    && self.open.as_ref().is_some_and(|o| o.id == chat_id && o.position(next.id).is_some()) =>
            {
                self.select_message(next.id);
                self.open_viewer(Viewer { msg_id: next.id, item: 0 })
            }
            // A round video that ended in the viewer, followed by a voice.
            Some(Media::Voice { file, .. })
                if self.viewer.is_some_and(|v| v.msg_id == after) && self.voice.is_none() =>
            {
                let file = *file;
                self.close_viewer();
                if let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id)
                    && let Some(i) = open.position(next.id)
                {
                    open.cursor = i;
                    self.scroll_messages();
                }
                self.play_audio(chat_id, next.id, next.sender, file, SpeedKind::Voice)
            }
            _ => Task::none(),
        }
    }

    /// Played to the end: mark it listened here and, for incoming
    /// messages, tell Telegram (the sender sees it too).
    fn mark_listened(&mut self, chat_id: i64, msg_id: i64) -> Task<Message> {
        let mut report = false;
        if let Some(open) = self.open.as_mut().filter(|o| o.id == chat_id)
            && let Some(i) = open.position(msg_id)
        {
            let msg = &mut open.msgs[i];
            for media in msg.media.iter_mut().filter(|m| m.unlistened()) {
                media.set_listened();
                report = !msg.outgoing;
            }
        }
        if report {
            Task::perform(tg::open_content(chat_id, msg_id), Message::Done)
        } else {
            Task::none()
        }
    }

    fn toggle_pause(&mut self) -> Task<Message> {
        if let Some(playback) = self.video.as_mut()
            && let Some(movie) = playback.movie.as_mut()
        {
            // Played to the end: it starts over at 1x, so the speed is set again.
            playback.speed_pending |= movie.video.eos();
            movie.video.set_paused(!movie.video.paused());
        } else if let Some(audio) = self.voice.as_mut().and_then(|v| v.audio.as_mut()) {
            audio.toggle_pause();
        } else if let Some(msg) = self.open.as_ref().and_then(OpenChat::current)
            && let Some(item) = msg.media.iter().position(|m| m.audible().is_some())
        {
            return self.activate_media(msg.id, item);
        }
        Task::none()
    }

    fn seek(&mut self, delta: f64) {
        if let Some(movie) = self.video.as_mut().and_then(VideoPlayback::movie_mut) {
            let target = (movie.video.position().as_secs_f64() + delta)
                .clamp(0.0, movie.video.duration().as_secs_f64());
            let _ = movie.seek(Duration::from_secs_f64(target));
        } else if let Some(audio) = self.voice.as_mut().and_then(|v| v.audio.as_mut()) {
            audio.seek_by(delta);
        }
    }

    /// The kind of media whose speed `[`/`]` change: what plays now, else
    /// voice messages.
    fn speed_kind(&self) -> SpeedKind {
        match (&self.video, &self.voice) {
            (Some(video), _) => video.kind,
            (None, Some(voice)) => voice.kind,
            (None, None) => SpeedKind::Voice,
        }
    }

    /// Speed of what plays now (or of voice messages).
    pub fn speed(&self) -> f64 {
        self.speeds.get(self.speed_kind())
    }

    /// One volume for videos and voice messages, kept across restarts.
    fn set_volume(&mut self, volume: f64) {
        self.volume = volume;
        player::save_volume(volume);
        if let Some(movie) = self.video.as_mut().and_then(VideoPlayback::movie_mut) {
            movie.set_volume(volume);
        }
        if let Some(audio) = self.voice.as_ref().and_then(|v| v.audio.as_ref()) {
            audio.set_volume(volume);
        }
        self.info(format!("Volume {}", player::fmt_volume(volume)));
    }

    fn set_speed(&mut self, speed: f64) {
        self.speeds.set(self.speed_kind(), speed);
        self.speeds.save();
        if let Some(playback) = self.video.as_mut()
            && let Some(movie) = playback.movie_mut()
        {
            playback.speed_pending = movie.set_speed(speed).is_err();
        }
        if let Some(audio) = self.voice.as_mut().and_then(|v| v.audio.as_mut()) {
            audio.set_speed(speed);
        }
        self.info(format!("Speed {}", player::fmt_speed(speed)));
    }

    // ---- keyboard ------------------------------------------------------

    fn on_key(&mut self, key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Task<Message> {
        if self.emoji.is_some() && matches!(self.mode, Mode::Insert | Mode::Text | Mode::Normal) {
            return self.key_emoji(key, physical, mods, text);
        }
        match self.mode {
            Mode::Insert => self.key_insert(key, physical, mods, text),
            Mode::Text => self.key_text(key, physical, mods),
            Mode::Command | Mode::Search => self.key_cmdline(key, physical, mods, text),
            Mode::Record => match normalize(key, physical, mods) {
                Some(NKey::Char('q') | NKey::Enter) => self.finish_recording(),
                Some(NKey::Esc) => {
                    self.stop_recording(false);
                    self.info("Recording discarded");
                    Task::none()
                }
                _ => Task::none(),
            },
            Mode::Normal => {
                let Some(key) = normalize(key, physical, mods) else {
                    return Task::none();
                };
                if self.confirm.is_some() {
                    return self.key_confirm(key);
                }
                if self.viewer_keys() {
                    return self.key_viewer(key);
                }
                // Visual selection: one key acts, like Vim's `d` and `y`.
                if self.visual.is_some() && self.focus == Focus::Messages {
                    match key {
                        NKey::Char('d' | 'x') => return self.act(Action::Delete),
                        NKey::Char('y' | 'Y') => return self.act(Action::Yank),
                        _ => {}
                    }
                }
                match self.pending.feed(key) {
                    Some(action) => self.act(action),
                    None => Task::none(),
                }
            }
        }
    }

    fn emoji_rows(&self) -> usize {
        view::emoji_rows(self.viewport().1)
    }

    fn open_emoji(&mut self) {
        if self.open.is_none() || self.auth != Auth::Ready {
            self.error("Open a chat first (Enter)");
            return;
        }
        self.focus = Focus::Messages;
        self.mode = Mode::Insert;
        self.emoji = Some(Picker::new(&self.recent_emoji));
        self.scroll_messages();
    }

    fn close_emoji(&mut self) {
        if self.emoji.take().is_some() {
            emoji::save_recent(&self.recent_emoji);
            self.scroll_messages();
        }
    }

    fn insert_emoji(&mut self, e: &str) {
        self.input.insert(e);
        emoji::remember(&mut self.recent_emoji, e);
        if matches!(self.mode, Mode::Normal | Mode::Text) {
            self.mode = Mode::Insert;
        }
    }

    /// Keys while the emoji panel is open: h/j/k/l move, Enter inserts,
    /// Tab switches groups, `/` searches by name, Esc closes.
    fn key_emoji(&mut self, key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Task<Message> {
        let rows = self.emoji_rows();
        let items: Vec<String> = self
            .emoji
            .as_ref()
            .map(|p| p.items(&self.recent_emoji).into_iter().map(str::to_owned).collect())
            .unwrap_or_default();
        let Some(picker) = self.emoji.as_mut() else {
            return Task::none();
        };
        let reacting = picker.react.is_some();
        if picker.searching {
            match key {
                Key::Named(Named::Escape | Named::Enter) => picker.searching = false,
                Key::Named(Named::Backspace) => {
                    picker.query.pop();
                    picker.cursor = 0;
                    picker.top = 0;
                }
                Key::Named(Named::ArrowDown) => picker.step(0, 1, items.len(), rows),
                Key::Named(Named::ArrowUp) => picker.step(0, -1, items.len(), rows),
                _ => {
                    if let Some(t) = text.filter(|t| !mods.control() && !t.chars().any(char::is_control)) {
                        picker.query.push_str(t);
                        picker.cursor = 0;
                        picker.top = 0;
                    }
                }
            }
            return Task::none();
        }
        if matches!(key, Key::Named(Named::Backspace)) {
            self.input.backspace();
            return Task::none();
        }
        if matches!(key, Key::Named(Named::Tab)) {
            if !reacting {
                picker.next_tab(if mods.shift() { -1 } else { 1 });
            }
            return Task::none();
        }
        let Some(nkey) = normalize(key, physical, mods) else {
            return Task::none();
        };
        let mut pick = None;
        match nkey {
            NKey::Esc | NKey::Ctrl('.') => {
                self.close_emoji();
                return Task::none();
            }
            NKey::Char('h') | NKey::Left => picker.step(-1, 0, items.len(), rows),
            NKey::Char('l') | NKey::Right => picker.step(1, 0, items.len(), rows),
            NKey::Char('j') | NKey::Down => picker.step(0, 1, items.len(), rows),
            NKey::Char('k') | NKey::Up => picker.step(0, -1, items.len(), rows),
            NKey::Ctrl('d') => picker.step(0, rows as isize / 2, items.len(), rows),
            NKey::Ctrl('u') => picker.step(0, -(rows as isize) / 2, items.len(), rows),
            NKey::Char('g') => picker.step(-(items.len() as isize), 0, items.len(), rows),
            NKey::Char('G') => picker.step(items.len() as isize, 0, items.len(), rows),
            // Reactions have no groups: digits pick the n-th one.
            NKey::Char(c @ '1'..='9') if reacting => pick = items.get(c as usize - '1' as usize).cloned(),
            NKey::Char(']') if !reacting => picker.next_tab(1),
            NKey::Char('[') if !reacting => picker.next_tab(-1),
            NKey::Char(c @ '0'..='9') if !reacting => picker.set_tab((c as usize - '0' as usize + 9) % 10),
            NKey::Char('/') => {
                picker.searching = true;
                picker.query.clear();
                picker.cursor = 0;
                picker.top = 0;
            }
            NKey::Enter | NKey::Space => pick = items.get(picker.cursor).cloned(),
            _ => {}
        }
        match pick {
            Some(e) => self.pick_emoji(e),
            None => Task::none(),
        }
    }

    /// An emoji chosen in the panel: typed, or put as the reaction.
    fn pick_emoji(&mut self, e: String) -> Task<Message> {
        let Some(react) = self.emoji.as_ref().and_then(|p| p.react.clone()) else {
            self.insert_emoji(&e);
            return Task::none();
        };
        self.close_emoji();
        if self.open.as_ref().is_none_or(|o| o.id != react.chat_id) {
            return Task::none();
        }
        self.toggle_reaction(react.msg_id, e)
    }

    fn reaction_chosen(&self, msg_id: i64, kind: &ReactionKind) -> bool {
        self.open
            .as_ref()
            .and_then(|o| o.position(msg_id).map(|i| &o.msgs[i]))
            .is_some_and(|m| m.reactions.iter().any(|r| r.chosen && r.kind == *kind))
    }

    /// Put the reaction on the message, or take it back if it's already there.
    fn toggle_reaction(&mut self, msg_id: i64, emoji: String) -> Task<Message> {
        let Some(chat_id) = self.open.as_ref().map(|o| o.id) else {
            return Task::none();
        };
        let kind = ReactionKind::Emoji(emoji.clone());
        if self.reaction_chosen(msg_id, &kind) {
            Task::perform(tg::remove_reaction(chat_id, msg_id, kind), Message::Done)
        } else {
            emoji::remember(&mut self.recent_emoji, &emoji);
            Task::perform(tg::add_reaction(chat_id, msg_id, emoji), Message::Done)
        }
    }

    fn key_confirm(&mut self, key: NKey) -> Task<Message> {
        let (chat_id, ids) = match self.confirm.take() {
            Some(Confirm::Delete { chat_id, ids }) => (chat_id, ids),
            Some(Confirm::Leave { chat_id, title }) if matches!(key, NKey::Char('y') | NKey::Enter) => {
                self.info(format!("Left {title}"));
                return Task::perform(tg::leave(chat_id), Message::Done);
            }
            Some(Confirm::Leave { .. }) => {
                self.info("Staying");
                return Task::none();
            }
            Some(Confirm::Quit) if matches!(key, NKey::Char('y') | NKey::Enter) => return self.handle(Message::Quit),
            Some(Confirm::Quit) => {
                self.info("Not quitting");
                return Task::none();
            }
            None => return Task::none(),
        };
        let revoke = match key {
            NKey::Char('y') => true,
            NKey::Char('m') => false,
            _ => {
                self.info("Cancelled");
                return Task::none();
            }
        };
        self.info(if revoke { "Deleted for everyone" } else { "Deleted for me" });
        Task::perform(tg::delete(chat_id, ids, revoke), Message::Done)
    }

    fn key_viewer(&mut self, key: NKey) -> Task<Message> {
        let (Some(open), Some(viewer)) = (&self.open, self.viewer) else {
            return Task::none();
        };
        let next = match key {
            NKey::Char('j' | 'n') | NKey::Down => open.viewable_near(viewer, 1),
            NKey::Char('k' | 'p') | NKey::Up => open.viewable_near(viewer, -1),
            NKey::Char('h') | NKey::Left => {
                self.seek(-SEEK_SHORT);
                None
            }
            NKey::Char('l') | NKey::Right => {
                self.seek(SEEK_SHORT);
                None
            }
            NKey::Char('H') => {
                self.seek(-SEEK_LONG);
                None
            }
            NKey::Char('L') => {
                self.seek(SEEK_LONG);
                None
            }
            NKey::Space => return self.toggle_pause(),
            NKey::Char(']' | '+') => {
                self.set_speed(player::faster(self.speed()));
                None
            }
            NKey::Char('[' | '-') => {
                self.set_speed(player::slower(self.speed()));
                None
            }
            NKey::Char('=') => {
                self.set_speed(1.0);
                None
            }
            NKey::Char('}') => {
                self.set_volume(player::louder(self.volume));
                None
            }
            NKey::Char('{') => {
                self.set_volume(player::quieter(self.volume));
                None
            }
            NKey::Esc | NKey::Enter | NKey::Char('q' | 'o') => {
                self.close_viewer();
                None
            }
            // Full screen ↔ beside the chat, for the rest of the session.
            NKey::Char('s') => {
                self.viewer_side = !self.viewer_side;
                self.viewer_focused = true;
                self.scroll_messages();
                None
            }
            // Beside the chat: the keys go to the messages, it plays on.
            NKey::Tab | NKey::Ctrl('h') if self.viewer_side => {
                self.viewer_focused = false;
                self.focus = Focus::Messages;
                None
            }
            _ => None,
        };
        match next {
            Some(next) => self.open_viewer(next),
            None => Task::none(),
        }
    }

    fn key_insert(&mut self, key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Task<Message> {
        if self.auth == Auth::Ready
            && mods.shift()
            && matches!(key, Key::Named(Named::Enter))
        {
            self.input.insert("\n");
            return Task::none();
        }
        if self.auth == Auth::Ready && mods.alt() && matches!(key, Key::Named(Named::Enter)) {
            return self.send_with(SendOptions {
                silent: true,
                ..SendOptions::default()
            });
        }
        if self.auth == Auth::Ready && mods.control() && key.to_latin(physical) == Some('.') {
            self.open_emoji();
            return Task::none();
        }
        if self.auth == Auth::Ready
            && matches!(key, Key::Named(Named::ArrowUp))
            && self.input.is_empty()
            && self.attachments.is_empty()
            && self.forward.is_none()
            && matches!(self.compose, Compose::New)
        {
            self.edit_last_own();
            return Task::none();
        }
        match edit(&mut self.input, key, physical, mods, text) {
            Edit::Submit if self.auth == Auth::Ready => self.send(),
            Edit::Submit => self.submit_auth(),
            // A message being written stays under a Vim cursor: Text mode.
            Edit::Cancel if self.auth == Auth::Ready && !self.input.is_empty() => {
                self.input.leave_insert();
                self.text_pending.clear();
                self.mode = Mode::Text;
                Task::none()
            }
            Edit::Cancel => {
                self.mode = Mode::Normal;
                Task::none()
            }
            Edit::Paste => self.read_clipboard(PasteTarget::Input(None)),
            Edit::Changed | Edit::Ignored => Task::none(),
        }
    }

    /// Text mode: Vim motions and operators in the message being written.
    fn key_text(&mut self, key: &Key, physical: Physical, mods: Modifiers) -> Task<Message> {
        let key = match key {
            Key::Named(Named::Backspace) => Some(NKey::Left),
            Key::Named(Named::Delete) => Some(NKey::Char('x')),
            Key::Named(Named::Home) => Some(NKey::Char('0')),
            Key::Named(Named::End) => Some(NKey::Char('$')),
            _ => normalize(key, physical, mods),
        };
        let Some(key) = key else {
            return Task::none();
        };
        match self.text_pending.feed(key, &mut self.input) {
            TextAction::None => Task::none(),
            TextAction::Insert => {
                self.mode = Mode::Insert;
                Task::none()
            }
            TextAction::Leave => {
                self.mode = Mode::Normal;
                Task::none()
            }
            TextAction::Submit => {
                let task = self.send();
                if self.input.is_empty() {
                    self.mode = Mode::Normal;
                }
                task
            }
            TextAction::Paste => self.read_clipboard(PasteTarget::Input(None)),
            TextAction::Yank(text) => {
                self.info("Copied to clipboard");
                iced::clipboard::write(text)
            }
        }
    }

    /// `↑` in an empty compose line: edit your last text message, like Telegram.
    fn edit_last_own(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let last = open
            .msgs
            .iter()
            .rposition(|m| m.outgoing && m.media.is_empty() && !m.sending && !m.scheduled && !m.deleted);
        let Some(i) = last else {
            self.error("No message of yours to edit here");
            return;
        };
        open.cursor = i;
        let (id, text) = (open.msgs[i].id, open.msgs[i].text.clone());
        self.focus = Focus::Messages;
        self.compose = Compose::Edit(id);
        self.input.insert(&text);
        self.scroll_messages();
    }

    fn key_cmdline(&mut self, key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Task<Message> {
        let search = self.mode == Mode::Search;
        let up = match key {
            Key::Named(Named::ArrowUp) => Some(true),
            Key::Named(Named::ArrowDown) => Some(false),
            _ if mods.control() => match key.to_latin(physical) {
                Some('p') => Some(true),
                Some('n') => Some(false),
                _ => None,
            },
            _ => None,
        };
        if let Some(up) = up {
            let history = if search { &mut self.search_history } else { &mut self.command_history };
            let current = self.cmdline.as_str().to_owned();
            let entry = if up { history.prev(&current) } else { history.next() }.map(str::to_owned);
            if let Some(entry) = entry {
                self.cmdline.set(&entry);
                if search && !self.search_messages {
                    return self.set_filter(entry);
                }
            }
            return Task::none();
        }
        let was_empty = self.cmdline.is_empty();
        let mut result = edit(&mut self.cmdline, key, physical, mods, text);
        if !matches!(result, Edit::Ignored) {
            let history = if search { &mut self.search_history } else { &mut self.command_history };
            if matches!(result, Edit::Submit) {
                history.push(self.cmdline.as_str());
                self.save_histories();
            } else {
                history.reset();
            }
        }
        // Like Vim: backspace on an empty command line leaves it.
        if was_empty && matches!(key, Key::Named(Named::Backspace)) {
            result = Edit::Cancel;
        }
        match result {
            Edit::Submit => {
                let line = self.cmdline.take();
                self.mode = Mode::Normal;
                if search && self.search_messages {
                    self.search_chat(line)
                } else if search {
                    self.focus = Focus::Chats;
                    // Typed too fast for the timer: search the server now.
                    self.search_global(self.filter.trim().to_owned())
                } else {
                    self.run_command(&line)
                }
            }
            Edit::Cancel => {
                self.cmdline.clear();
                self.mode = Mode::Normal;
                if search && !self.search_messages {
                    self.clear_filter();
                }
                Task::none()
            }
            Edit::Changed if search && !self.search_messages => self.set_filter(self.cmdline.as_str().to_owned()),
            Edit::Paste => self.read_clipboard(PasteTarget::Cmdline),
            Edit::Changed | Edit::Ignored => Task::none(),
        }
    }

    fn search_chat(&mut self, query: String) -> Task<Message> {
        let query = query.trim().to_owned();
        let Some(chat_id) = self.open.as_ref().map(|o| o.id) else {
            return Task::none();
        };
        if query.is_empty() {
            return self.next_match(1);
        }
        if query.len() > 1 && query.starts_with('#') && !query.contains(char::is_whitespace) {
            return self.show_tag(query);
        }
        self.info(format!("Searching for {query}…"));
        Task::perform(tg::search(chat_id, query.clone()), move |result| Message::Found {
            chat_id,
            query: query.clone(),
            result,
        })
    }

    fn jump_to_date(&mut self, arg: &str) -> Task<Message> {
        let Some(chat_id) = self.open.as_ref().map(|o| o.id) else {
            self.error("Open a chat first (Enter)");
            return Task::none();
        };
        let Some(date) = parse_date(arg) else {
            self.error(format!("Bad date: {arg} (try 01.05.2024, 2024-05-01, 01.05, yesterday)"));
            return Task::none();
        };
        // The last message of that day, so the whole day is right above it.
        let end_of_day = date
            .succ_opt()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|d| d.and_local_timezone(chrono::Local).earliest())
            .map_or(i32::MAX, |d| (d.timestamp() - 1).min(i64::from(i32::MAX)) as i32);
        self.info(format!("Jumping to {}", date.format("%d.%m.%Y")));
        Task::perform(tg::message_at(chat_id, end_of_day), move |result| Message::DateFound {
            chat_id,
            result,
        })
    }

    fn save_histories(&self) {
        let lines: Vec<String> = self
            .search_history
            .entries()
            .iter()
            .map(|e| format!("/{e}"))
            .chain(self.command_history.entries().iter().map(|e| format!(":{e}")))
            .collect();
        let path = history_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, lines.join("\n"));
    }

    fn read_clipboard(&mut self, target: PasteTarget) -> Task<Message> {
        self.paste = target;
        // The login form only takes text.
        if self.auth != Auth::Ready {
            return iced::clipboard::read().map(Message::Paste);
        }
        Task::perform(clipboard::read(), Message::Clip)
    }

    fn on_paste(&mut self, text: Option<String>) -> Task<Message> {
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            self.error("Clipboard is empty");
            return Task::none();
        };
        let text = text.replace("\r\n", "\n");
        match self.paste {
            PasteTarget::Cmdline => {
                self.cmdline.insert(&text.replace('\n', " "));
                if self.mode == Mode::Search && !self.search_messages {
                    return self.set_filter(self.cmdline.as_str().to_owned());
                }
            }
            PasteTarget::Input(position) => {
                if self.auth == Auth::Ready && self.open.is_none() {
                    self.error("Open a chat first (Enter)");
                    return Task::none();
                }
                match position {
                    Some(true) => self.input.home(),
                    Some(false) => self.input.end(),
                    None => {}
                }
                self.input.insert(&text);
                if self.mode == Mode::Text {
                    // Onto the last pasted char, like Vim's `p`.
                    self.input.leave_insert();
                }
                self.scroll_messages();
            }
        }
        Task::none()
    }

    fn run_command(&mut self, line: &str) -> Task<Message> {
        let line = line.trim();
        let (cmd, arg) = match line.split_once(' ') {
            Some((cmd, arg)) => (cmd, Some(arg.trim())),
            None => (line, None),
        };
        match (cmd, arg) {
            ("", _) => {}
            ("q" | "q!" | "qa" | "qa!" | "quit" | "wq" | "x", _) => return self.handle(Message::Quit),
            ("h" | "help", _) => self.info(HELP),
            ("noh" | "nohl" | "nohlsearch", _) => self.clear_filter(),
            ("posts", arg) => return self.search_posts(arg),
            ("theme" | "colorscheme" | "colo", None) => {
                let names: Vec<String> = Theme::ALL.iter().map(Theme::to_string).collect();
                self.info(names.join(", "));
            }
            ("theme" | "colorscheme" | "colo", Some(name)) => match find_theme(name) {
                Some(theme) => self.theme = theme,
                None => self.error(format!("Unknown theme: {name}")),
            },
            ("f" | "folder", None) => {
                let names: Vec<&str> = self.folders.iter().map(|f| f.name.as_str()).collect();
                self.info(names.join(", "));
            }
            ("f" | "folder", Some(name)) => {
                let wanted = normalize_name(name);
                let found = self
                    .folders
                    .iter()
                    .position(|f| normalize_name(&f.name).contains(&wanted));
                match found {
                    Some(index) => return self.switch_folder(index),
                    None => self.error(format!("No folder matches: {name}")),
                }
            }
            ("d" | "date", Some(value)) => return self.jump_to_date(value),
            ("a" | "attach", None) => return self.pick_files(),
            ("a" | "attach", Some(path)) => {
                self.attach(vec![expand_home(path)]);
                return Task::none();
            }
            ("autostart", arg) => return self.autostart(arg),
            ("cleancache", days) => {
                let days = match days.map(str::parse::<i32>) {
                    None => CACHE_DAYS,
                    Some(Ok(days)) if days >= 0 => days,
                    Some(_) => {
                        self.error("Usage: :cleancache [days unused, default 7]");
                        return Task::none();
                    }
                };
                self.info("Cleaning the cache…");
                return clean_cache(days);
            }
            ("emoji", None) => {
                self.open_emoji();
                return Task::none();
            }
            ("silent", _) => {
                return self.send_with(SendOptions {
                    silent: true,
                    ..SendOptions::default()
                });
            }
            ("later" | "schedule", Some(when)) => match parse_when(when, chrono::Local::now()) {
                Some(at) => {
                    return self.send_with(SendOptions {
                        at: Some(at),
                        ..SendOptions::default()
                    });
                }
                None => self.error(format!("Bad time: {when} (try 21:00, +30m, +2h, 01.11 09:00)")),
            },
            ("export", path) => return self.export(path),
            ("pin" | "pinned", _) => return self.handle(Message::JumpPinned),
            ("pins", _) => return self.handle(Message::ListPinned),
            ("s" | "search", Some(query)) => {
                self.search_messages = true;
                return self.search_chat(query.to_owned());
            }
            ("speed", Some(value)) => match value.trim_end_matches('x').parse::<f64>() {
                Ok(speed) if (0.25..=5.0).contains(&speed) => self.set_speed(speed),
                _ => self.error("Speed must be between 0.25 and 5"),
            },
            ("volume", Some(value)) => match value.trim_end_matches('%').parse::<f64>() {
                Ok(percent) if (0.0..=100.0).contains(&percent) => self.set_volume(percent / 100.0),
                _ => self.error("Volume must be between 0 and 100"),
            },
            ("volume", None) => self.info(format!("Volume {}", player::fmt_volume(self.volume))),
            (n, None) if n.parse::<usize>().is_ok() => {
                let line = n.parse::<usize>().unwrap_or(1).max(1) - 1;
                return self.jump(line);
            }
            (other, _) => self.error(format!("Not an editor command: {other}")),
        }
        Task::none()
    }

    /// Messages an action applies to: the Visual selection, or the one
    /// under the cursor.
    fn targets(&self) -> Vec<&Msg> {
        let Some(open) = self.open.as_ref().filter(|_| self.focus == Focus::Messages) else {
            return Vec::new();
        };
        match self.visual.and_then(|anchor| open.position(anchor)) {
            Some(anchor) => open.msgs[anchor.min(open.cursor)..=anchor.max(open.cursor)].iter().collect(),
            None => open.current().into_iter().collect(),
        }
    }

    /// The message is in the Visual selection.
    pub fn in_visual(&self, id: i64) -> bool {
        let (Some(anchor), Some(cursor)) = (self.visual, self.open.as_ref().and_then(|o| o.current())) else {
            return false;
        };
        (anchor.min(cursor.id)..=anchor.max(cursor.id)).contains(&id)
    }

    /// Number of messages in the Visual selection.
    pub fn visual_len(&self) -> usize {
        if self.visual.is_some() { self.targets().len() } else { 0 }
    }

    /// The message under the cursor, when the message pane has focus.
    fn focused_message(&self) -> Option<&Msg> {
        match self.focus {
            Focus::Messages => self.open.as_ref()?.current(),
            Focus::Chats | Focus::Folders => None,
        }
    }

    fn act(&mut self, action: Action) -> Task<Message> {
        self.status = None;
        match action {
            Action::Down(n) => self.step(n as isize),
            Action::Up(n) => self.step(-(n as isize)),
            Action::Top => self.jump(0),
            Action::Bottom if self.focus == Focus::Messages => self.to_latest(),
            Action::Bottom => self.jump(usize::MAX),
            Action::HalfDown => self.step(self.half_page() as isize),
            Action::HalfUp => self.step(-(self.half_page() as isize)),
            Action::NextFolder => self.cycle_folder(1),
            Action::PrevFolder => self.cycle_folder(-1),
            Action::Folder(index) => self.switch_folder(index),
            // h / l: one pane left / right (folders, chats, messages).
            Action::FocusChats => {
                self.focus = match self.focus {
                    Focus::Messages => Focus::Chats,
                    Focus::Chats | Focus::Folders => Focus::Folders,
                };
                Task::none()
            }
            Action::FocusMessages if self.focus == Focus::Messages && self.side_viewer() => {
                self.viewer_focused = true;
                Task::none()
            }
            Action::FocusMessages => {
                self.focus = match self.focus {
                    Focus::Folders => Focus::Chats,
                    Focus::Chats | Focus::Messages if self.open.is_some() => Focus::Messages,
                    other => other,
                };
                Task::none()
            }
            Action::ToggleFocus if self.focus == Focus::Messages && self.side_viewer() => {
                self.viewer_focused = true;
                Task::none()
            }
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Messages | Focus::Folders => Focus::Chats,
                    Focus::Chats if self.open.is_some() => Focus::Messages,
                    Focus::Chats => Focus::Chats,
                };
                Task::none()
            }
            Action::Open if self.focus == Focus::Folders => {
                self.focus = Focus::Chats;
                Task::none()
            }
            Action::Open if self.focus == Focus::Chats && self.hit.is_some() => {
                let Some((chat_id, msg_id)) = self.current_hit().map(|m| (m.chat_id, m.id)) else {
                    return Task::none();
                };
                self.open_chat_at(chat_id, Some(msg_id))
            }
            Action::Open => match (self.focus, self.selected) {
                (Focus::Chats, Some(id)) if self.forward.is_some() => self.forward_to(id, SendOptions::default()),
                (Focus::Chats, Some(id)) if self.auth == Auth::Ready => {
                    let reopen = self.open.as_ref().is_some_and(|o| o.id == id);
                    let task = self.open_chat(id);
                    // A channel opens next to the list, so j/k keep going through
                    // the chats; Enter again (or l, Tab) moves on to the posts.
                    if !reopen && self.chats.get(&id).is_some_and(|c| c.kind == ChatKind::Channel) {
                        self.focus = Focus::Chats;
                    }
                    task
                }
                (Focus::Messages, _) if self.focused_message().map(|m| m.id).is_some_and(|id| self.reveal(id)) => {
                    Task::none()
                }
                (Focus::Messages, _) => {
                    let target = self.focused_message().and_then(|m| {
                        // The track of an album that plays now, else the first.
                        let playing = self.voice.as_ref().map(|v| v.file_id);
                        let item = m
                            .media
                            .iter()
                            .position(|x| x.audible().is_some() && x.audible() == playing)
                            .or_else(|| m.media.iter().position(|x| x.viewable() || x.audible().is_some()))?;
                        Some((m.id, item))
                    });
                    match target {
                        Some((msg_id, item)) => self.activate_media(msg_id, item),
                        None if self.open.as_ref().is_some_and(OpenChat::pins_only) => self.leave_tag(),
                        None => Task::none(),
                    }
                }
                _ => Task::none(),
            },
            Action::Insert | Action::InsertStart | Action::Append => {
                if self.auth == Auth::Ready && self.open.is_none() {
                    self.error("Open a chat first (Enter)");
                } else if self.auth != Auth::Ready || self.may_post() {
                    self.input.checkpoint();
                    match action {
                        Action::InsertStart => self.input.home(),
                        Action::Append => self.input.end(),
                        _ => {}
                    }
                    self.mode = Mode::Insert;
                }
                Task::none()
            }
            Action::Command => {
                self.cmdline.clear();
                self.mode = Mode::Command;
                Task::none()
            }
            Action::Search => {
                if self.auth == Auth::Ready {
                    self.cmdline.clear();
                    self.mode = Mode::Search;
                    self.search_messages = self.focus == Focus::Messages && self.open.is_some();
                }
                Task::none()
            }
            Action::Attach if !self.may_post() => Task::none(),
            Action::Attach => self.pick_files(),
            Action::Emoji => {
                self.open_emoji();
                Task::none()
            }
            Action::React => match (self.open.as_ref().map(|o| o.id), self.focused_message().map(|m| m.id)) {
                (Some(chat_id), Some(msg_id)) => {
                    Task::perform(tg::available_reactions(chat_id, msg_id), move |result| Message::Reactions {
                        chat_id,
                        msg_id,
                        result,
                    })
                }
                _ => {
                    self.error("Select a message first (l)");
                    Task::none()
                }
            },
            Action::JumpPlaying => match self.voice.as_ref().map(|v| (v.chat_id, v.msg_id)) {
                Some((chat_id, msg_id)) => {
                    self.select_chat(chat_id);
                    self.open_chat_at(chat_id, Some(msg_id))
                }
                None => {
                    self.error("Nothing is playing");
                    Task::none()
                }
            },
            Action::Pinned => self.handle(Message::JumpPinned),
            Action::PinnedList => self.handle(Message::ListPinned),
            Action::GoToSource => self.go_to_source(),
            Action::OpenLink(n) => {
                let url = self.focused_message().map(|m| m.links.get(n.saturating_sub(1)).map(|l| l.url.clone()));
                match url {
                    Some(Some(url)) => self.open_link(url),
                    Some(None) if n > 1 => {
                        self.error(format!("The message has no link #{n}"));
                        Task::none()
                    }
                    Some(None) => {
                        self.error("No link in this message");
                        Task::none()
                    }
                    None => {
                        self.error("Select a message first (l)");
                        Task::none()
                    }
                }
            }
            Action::Comments => self.open_comments(),
            Action::JumpBack => match self.jumps.pop() {
                Some((chat_id, msg_id)) if self.open.as_ref().is_some_and(|o| o.id == chat_id) => {
                    self.jump_to_message(msg_id)
                }
                Some((chat_id, msg_id)) => self.open_chat_at(chat_id, Some(msg_id)),
                None => {
                    self.error("Nowhere to go back to (gf jumps)");
                    Task::none()
                }
            },
            Action::SearchNext => self.next_match(1),
            Action::SearchPrev => self.next_match(-1),
            Action::Delete => {
                // Deleted messages kept from the archive are gone already.
                let ids: Vec<i64> =
                    self.targets().iter().filter(|m| !m.deleted).flat_map(|m| m.members.iter().copied()).collect();
                match self.open.as_ref().map(|o| o.id) {
                    Some(chat_id) if self.read_only(chat_id) => {
                        self.error("Only the channel's admins delete its posts");
                    }
                    Some(chat_id) if !ids.is_empty() => {
                        self.visual = None;
                        self.confirm = Some(Confirm::Delete { chat_id, ids });
                    }
                    _ => self.error("Select a message first (l)"),
                }
                Task::none()
            }
            Action::Yank => self.yank(),
            Action::Paste { before } => self.read_clipboard(PasteTarget::Input(Some(before))),
            Action::Record => {
                if self.may_post() {
                    self.start_recording();
                }
                Task::none()
            }
            Action::PlayPause => self.toggle_pause(),
            Action::Faster => {
                self.set_speed(player::faster(self.speed()));
                Task::none()
            }
            Action::Slower => {
                self.set_speed(player::slower(self.speed()));
                Task::none()
            }
            Action::NormalSpeed => {
                self.set_speed(1.0);
                Task::none()
            }
            Action::Mute => self.toggle_mute(),
            Action::Leave => self.leave_or_join(),
            Action::Louder => {
                self.set_volume(player::louder(self.volume));
                Task::none()
            }
            Action::Quieter => {
                self.set_volume(player::quieter(self.volume));
                Task::none()
            }
            Action::SeekBack => {
                self.seek(-SEEK_SHORT);
                Task::none()
            }
            Action::SeekForward => {
                self.seek(SEEK_SHORT);
                Task::none()
            }
            Action::Reply if !self.may_post() => Task::none(),
            Action::Reply => {
                match self.focused_message().map(|m| m.id) {
                    Some(id) => {
                        if matches!(self.compose, Compose::Edit(_)) {
                            self.input.clear();
                        }
                        self.compose = Compose::Reply(id);
                        self.mode = Mode::Insert;
                        self.scroll_messages();
                    }
                    None => self.error("Select a message first (l)"),
                }
                Task::none()
            }
            Action::Edit => {
                let target = self.focused_message().map(|m| (m.id, m.outgoing && m.media.is_empty(), m.text.clone()));
                match target {
                    Some((id, true, text)) => {
                        self.compose = Compose::Edit(id);
                        self.input.clear();
                        self.input.insert(&text);
                        self.mode = Mode::Insert;
                        self.scroll_messages();
                    }
                    Some(_) => self.error("Only your own text messages can be edited"),
                    None => self.error("Select a message first (l)"),
                }
                Task::none()
            }
            Action::Forward => {
                let ids: Vec<i64> = self.targets().iter().flat_map(|m| m.members.iter().copied()).collect();
                match self.open.as_ref().map(|o| o.id) {
                    Some(chat) if !ids.is_empty() => {
                        let what = match self.visual_len() {
                            0 | 1 => "the message".to_owned(),
                            n => format!("{} messages", tg::fmt_count(n)),
                        };
                        self.visual = None;
                        self.end_forward();
                        let draft = std::mem::take(&mut self.input);
                        self.info(format!(
                            "Forward {what}: Enter sends to the chat, v marks several, i adds a comment (Esc cancels)"
                        ));
                        self.forward = Some(Forward { from: chat, ids, what, draft });
                        self.focus = Focus::Chats;
                    }
                    _ => self.error("Select a message first (l)"),
                }
                Task::none()
            }
            Action::Visual => {
                match self.focus {
                    Focus::Chats if self.forward.is_some() => {
                        if let Some(id) = self.selected {
                            match self.forward_marks.iter().position(|&m| m == id) {
                                Some(i) => {
                                    self.forward_marks.remove(i);
                                }
                                None => self.forward_marks.push(id),
                            }
                            let n = self.forward_marks.len();
                            self.info(match n {
                                0 => "No chats marked: Enter sends to the one under the cursor".to_owned(),
                                1 => "1 chat marked: Enter sends there, v marks more".to_owned(),
                                n => format!("{} chats marked: Enter sends to all of them", tg::fmt_count(n)),
                            });
                        }
                    }
                    Focus::Messages if self.open.as_ref().is_some_and(|o| !o.msgs.is_empty()) => {
                        if self.visual.take().is_none() {
                            self.visual = self.focused_message().map(|m| m.id);
                            self.info("Visual: j/k select, f forward, y copy, d delete, Esc cancel");
                        }
                    }
                    _ => self.error("v selects messages (l to get there), or marks chats while forwarding"),
                }
                Task::none()
            }
            Action::Cancel => {
                self.pending.clear();
                if self.visual.take().is_some() {
                    return Task::none();
                }
                // Attachments go one per Esc, newest first.
                if let Some(path) = self.attachments.pop() {
                    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    self.info(match self.attachments.len() {
                        0 => format!("Removed {name}"),
                        left => format!("Removed {name}, {left} left (Esc removes the next)"),
                    });
                    self.scroll_messages();
                    return Task::none();
                }
                let mut cancelled = false;
                if self.end_forward().is_some() {
                    self.info("Forward cancelled");
                    cancelled = true;
                }
                if matches!(self.compose, Compose::Edit(_)) {
                    self.input.clear();
                }
                cancelled |= !matches!(self.compose, Compose::New);
                self.compose = Compose::New;
                if !self.filter.is_empty() {
                    self.clear_filter();
                    cancelled = true;
                }
                if !cancelled && self.side_viewer() {
                    self.close_viewer();
                    return Task::none();
                }
                if !cancelled && self.open.as_ref().is_some_and(|o| o.tag.is_some()) {
                    return self.leave_tag();
                }
                if !cancelled && self.open.as_ref().is_some_and(|o| o.thread.is_some()) {
                    return self.leave_thread();
                }
                if !cancelled {
                    self.confirm = Some(Confirm::Quit);
                }
                self.scroll_messages();
                Task::none()
            }
        }
    }

    /// t.me links open here (at the post, `Ctrl-o` comes back); web and
    /// e-mail links in the system's browser or mail client.
    fn open_link(&mut self, url: String) -> Task<Message> {
        if let Some(id) = url.strip_prefix(SPOILER_LINK).and_then(|id| id.parse().ok()) {
            self.reveal(id);
            return Task::none();
        }
        if url.starts_with('#') {
            return self.show_tag(url);
        }
        let parsed = url::Url::parse(&url).ok();
        let host = parsed.as_ref().and_then(|u| u.host_str()).unwrap_or("");
        if ["t.me", "telegram.me", "telegram.dog"].contains(&host.trim_start_matches("www.")) {
            let from = self.focused_message().map(|m| (m.chat_id, m.id));
            self.info(format!("Opening {url}…"));
            return Task::perform(tg::resolve_link(url), move |result| Message::LinkResolved { from, result });
        }
        match parsed.as_ref().map(url::Url::scheme) {
            Some("http" | "https" | "mailto") => match open_external(&url) {
                Ok(()) => self.info(format!("Opened {url}")),
                Err(e) => self.error(format!("Can't open {url}: {e}")),
            },
            _ => self.error(format!("Not opening {url}: only web and e-mail links")),
        }
        Task::none()
    }

    /// `gf`: the original of a forwarded message (or at least its author's
    /// chat), otherwise the message it replies to.
    fn go_to_source(&mut self) -> Task<Message> {
        let Some(m) = self.focused_message() else {
            self.error("Select a message first (l)");
            return Task::none();
        };
        let from = (m.chat_id, m.id);
        let (chat, msg_id) = match (&m.forward, m.reply_to) {
            (Some(f), _) => match (f.source, f.sender) {
                (Some((chat, id)), _) => (Sender::Chat(chat), Some(id)),
                (None, Some(sender)) => (sender, None),
                (None, None) => {
                    self.error(format!("{} hides their account", f.name));
                    return Task::none();
                }
            },
            (None, Some(reply_to)) => (Sender::Chat(m.chat_id), Some(reply_to)),
            (None, None) => {
                self.error("Neither a forward nor a reply");
                return Task::none();
            }
        };
        if let (Sender::Chat(chat_id), Some(msg_id)) = (chat, msg_id)
            && chat_id == from.0
        {
            self.jumps.push(from);
            return self.jump_to_message(msg_id);
        }
        let load = match chat {
            Sender::Chat(chat_id) => Task::perform(tg::load_chat(chat_id), move |result| Message::GoTo { from, msg_id, result }),
            Sender::User(user_id) => Task::perform(tg::private_chat(user_id), move |result| Message::GoTo { from, msg_id, result }),
        };
        self.info("Opening the original…");
        load
    }

    /// Forward to the chats marked with `v`, or to this one when none are,
    /// with the compose line as the comment.
    fn forward_to(&mut self, chat_id: i64, options: SendOptions) -> Task<Message> {
        let mut to = std::mem::take(&mut self.forward_marks);
        let Some((forward, comment)) = self.end_forward() else {
            return Task::none();
        };
        if to.is_empty() {
            to.push(chat_id);
        }
        self.mode = Mode::Normal;
        self.info(match to.as_slice() {
            [one] => format!("Forwarded to {}", self.chats.get(one).map_or("chat", |c| c.title.as_str())),
            many => format!("Forwarded to {} chats", tg::fmt_count(many.len())),
        });
        let options = SendOptions { reply_to: None, ..options };
        Task::batch(to.into_iter().map(|chat| {
            let task = tg::forward(chat, forward.from, forward.ids.clone(), comment.clone(), options);
            Task::perform(task, Message::Done)
        }))
    }

    /// Stop forwarding and give the compose line back what it held before;
    /// returns the forward and the comment typed for it.
    fn end_forward(&mut self) -> Option<(Forward, String)> {
        let mut forward = self.forward.take()?;
        self.forward_marks.clear();
        let comment = std::mem::replace(&mut self.input, std::mem::take(&mut forward.draft));
        Some((forward, comment.as_str().trim().to_owned()))
    }

    fn start_recording(&mut self) {
        if self.auth != Auth::Ready || self.open.is_none() {
            self.error("Open a chat first (Enter)");
            return;
        }
        self.stop_voice();
        match Recorder::start(config::dirs().data_dir().join("voice")) {
            Ok(recorder) => {
                self.recorder = Some(recorder);
                self.mode = Mode::Record;
            }
            Err(e) => self.error(e),
        }
    }

    /// Drop the recorder; `keep` is false to discard the file.
    fn stop_recording(&mut self, keep: bool) -> Option<Recorder> {
        self.mode = Mode::Normal;
        let recorder = self.recorder.take()?;
        if keep {
            return Some(recorder);
        }
        recorder.cancel();
        None
    }

    fn finish_recording(&mut self) -> Task<Message> {
        let chat_id = self.open.as_ref().map(|o| o.id);
        let thread = self.open.as_ref().map_or(0, OpenChat::thread_root);
        let (Some(recorder), Some(chat_id)) = (self.stop_recording(true), chat_id) else {
            return Task::none();
        };
        self.info("Sending voice message…");
        Task::perform(stop_and_send(chat_id, thread, recorder), Message::Done)
    }

    fn yank(&mut self) -> Task<Message> {
        let text = match self.focus {
            Focus::Messages => {
                let texts: Vec<&str> = self.targets().iter().map(|m| m.text.as_str()).filter(|t| !t.is_empty()).collect();
                let text = texts.join("\n\n");
                self.visual = None;
                Some(text)
            }
            Focus::Chats => self.selected.and_then(|id| self.chats.get(&id)).map(|c| c.title.clone()),
            Focus::Folders => self.folders.get(self.folder).map(|f| f.name.clone()),
        };
        match text.filter(|t| !t.is_empty()) {
            Some(text) => {
                self.info("Copied to clipboard");
                iced::clipboard::write(text)
            }
            None => {
                self.error("Nothing to copy");
                Task::none()
            }
        }
    }

    // ---- mouse ---------------------------------------------------------

    fn select_message(&mut self, id: i64) {
        if let Some(open) = self.open.as_mut()
            && let Some(i) = open.position(id)
        {
            open.cursor = i;
            self.focus = Focus::Messages;
            self.viewer_focused = false;
            self.scroll_messages();
        }
    }

    fn on_scroll(&mut self, pane: Pane, delta: ScrollDelta) -> Task<Message> {
        let lines = match delta {
            ScrollDelta::Lines { y, .. } => y,
            ScrollDelta::Pixels { y, .. } => {
                self.scroll_pixels += y;
                let lines = (self.scroll_pixels / SCROLL_PIXELS).trunc();
                self.scroll_pixels -= lines * SCROLL_PIXELS;
                lines
            }
        };
        // Wheel up (positive) moves towards the top of the list.
        let steps = -(lines.round() as isize);
        if steps == 0 {
            return Task::none();
        }
        match pane {
            Pane::Folders => self.cycle_folder(steps.signum()),
            Pane::Chats => {
                self.focus = Focus::Chats;
                self.step(steps * 3)
            }
            Pane::Messages if self.open.is_some() => {
                self.focus = Focus::Messages;
                self.step(steps)
            }
            Pane::Messages => Task::none(),
            Pane::Emoji => {
                let rows = self.emoji_rows();
                let count = self.emoji.as_ref().map_or(0, |p| p.items(&self.recent_emoji).len());
                if let Some(picker) = self.emoji.as_mut() {
                    picker.scroll(steps, count, rows);
                }
                Task::none()
            }
        }
    }

    // ---- movement ------------------------------------------------------

    fn half_page(&self) -> usize {
        match self.focus {
            Focus::Chats => (view::chat_rows(self.size) / 2).max(1),
            Focus::Messages => 5,
            Focus::Folders => 3,
        }
    }

    fn current(&self) -> usize {
        match (self.focus, &self.open) {
            (Focus::Messages, Some(open)) => open.cursor,
            (Focus::Folders, _) => self.folder,
            _ => self.cursor_row().unwrap_or(0),
        }
    }

    fn step(&mut self, delta: isize) -> Task<Message> {
        // j/k scroll through a message taller than the screen first.
        if self.focus == Focus::Messages && delta.abs() == 1 && self.is_tall() {
            let max = self.tall_limit();
            if delta > 0 && self.tall_offset < max - 1.0 {
                self.tall_offset = (self.tall_offset + TALL_STEP).min(max);
                return self.scroll_tall();
            }
            if delta < 0 && self.tall_offset > 1.0 {
                self.tall_offset = (self.tall_offset - TALL_STEP).max(0.0);
                return self.scroll_tall();
            }
        }
        let target = self.current().saturating_add_signed(delta);
        let task = self.jump(target);
        // Coming from below, a tall message starts at its end.
        if self.focus == Focus::Messages && delta < 0 && self.is_tall() {
            self.tall_offset = self.tall_limit();
            return Task::batch([task, self.scroll_tall()]);
        }
        task
    }

    /// How far the tall message under the cursor can scroll.
    fn tall_limit(&self) -> f32 {
        let (width, height) = self.viewport();
        self.tall_max.unwrap_or_else(|| {
            self.open
                .as_ref()
                .and_then(OpenChat::current)
                .map_or(0.0, |m| (view::message_height(m, width) - height).max(0.0))
        })
    }

    fn reset_tall(&mut self) {
        self.tall_offset = 0.0;
        self.tall_max = None;
    }

    fn scroll_tall(&self) -> Task<Message> {
        if !self.is_tall() {
            return Task::none();
        }
        iced::widget::operation::scroll_to(
            TALL_ID,
            AbsoluteOffset {
                x: 0.0,
                y: self.tall_offset,
            },
        )
    }

    /// Move the cursor of the focused pane to `index` (clamped).
    fn jump(&mut self, index: usize) -> Task<Message> {
        if self.focus == Focus::Folders {
            return self.switch_folder(index.min(self.folders.len().saturating_sub(1)));
        }
        match (self.focus, self.open.as_mut()) {
            (Focus::Messages, Some(open)) => {
                if open.msgs.is_empty() {
                    return Task::none();
                }
                let before = open.cursor;
                open.cursor = index.min(open.msgs.len() - 1);
                let moved = open.cursor != before;
                let near_top = open.cursor < PREFETCH;
                let near_bottom = open.cursor + PREFETCH >= open.msgs.len();
                if moved {
                    self.reset_tall();
                }
                self.scroll_messages();
                let scroll = if moved { self.scroll_tall() } else { Task::none() };
                if near_top {
                    return Task::batch([scroll, self.load_history(true)]);
                }
                if near_bottom {
                    return Task::batch([scroll, self.load_newer()]);
                }
                return scroll;
            }
            _ => {
                if self.rows() == 0 {
                    return Task::none();
                }
                let index = index.min(self.rows() - 1);
                if let Some(hit) = index.checked_sub(self.list.len()) {
                    // m, U and yy act on the chat of the message.
                    self.hit = Some(hit);
                    self.selected = self.current_hit().map(|m| m.chat_id);
                    self.scroll_chats();
                    if hit + PREFETCH >= self.global.msgs.len() {
                        return self.more_hits();
                    }
                    return Task::none();
                }
                self.hit = None;
                self.selected = Some(self.list[index]);
                self.scroll_chats();
                let preview = self.schedule_preview();
                if index + PREFETCH >= self.list.len() {
                    return Task::batch([preview, self.load_more_chats()]);
                }
                preview
            }
        }
    }

    fn scroll_chats(&mut self) {
        let rows = view::chat_rows(self.size);
        let index = self.cursor_row().unwrap_or(0);
        if index < self.chat_top {
            self.chat_top = index;
        } else if index >= self.chat_top + rows {
            self.chat_top = index + 1 - rows;
        }
        self.chat_top = self.chat_top.min(self.rows().saturating_sub(rows));
    }

    fn scroll_messages(&mut self) {
        let tall = self.is_tall();
        let (width, height) = self.viewport();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.msgs.is_empty() {
            return;
        }
        let last = open.msgs.len() - 1;
        open.cursor = open.cursor.min(last);
        open.bottom = open.bottom.min(last);
        let (cursor, msgs) = (open.cursor, &open.msgs);
        let top_of = |bottom: usize| view::top_for_bottom(msgs, bottom, width, height);
        let bottom_of = |top: usize| view::bottom_for_top(msgs, top, width, height);
        if tall {
            open.bottom = cursor;
            return;
        }
        let mut bottom = open.bottom;
        // Keep a few messages below the cursor in view...
        bottom = bottom.max((cursor + SCROLLOFF).min(last));
        while bottom > cursor && cursor < top_of(bottom) {
            bottom -= 1;
        }
        // ...and a few above it.
        let want_top = cursor.saturating_sub(SCROLLOFF);
        if want_top < top_of(bottom) {
            bottom = bottom_of(want_top).max(cursor);
            while bottom > cursor && cursor < top_of(bottom) {
                bottom -= 1;
            }
        }
        open.bottom = bottom;
    }
}

/// Finalizing the OGG file waits for the pipeline, so do it off the UI thread.
async fn stop_and_send(chat_id: i64, thread: i64, recorder: Recorder) -> Result<(), String> {
    let recording = tokio::task::spawn_blocking(move || recorder.stop())
        .await
        .map_err(|e| e.to_string())??;
    tg::send_voice(chat_id, recording, SendOptions { thread, ..SendOptions::default() }).await
}

enum Edit {
    Submit,
    Cancel,
    Paste,
    Changed,
    Ignored,
}

/// Readline-style editing shared by Insert, Command and Search modes.
fn edit(input: &mut Input, key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Edit {
    if let Key::Named(named) = key {
        match named {
            Named::Escape => return Edit::Cancel,
            Named::Enter => return Edit::Submit,
            Named::Backspace => input.backspace(),
            Named::Delete => input.delete(),
            Named::ArrowLeft => input.left(),
            Named::ArrowRight => input.right(),
            Named::Home => input.home(),
            Named::End => input.end(),
            _ => return insert_text(input, mods, text),
        }
        return Edit::Changed;
    }

    if mods.control() {
        match key.to_latin(physical) {
            Some('[') => return Edit::Cancel,
            // Ctrl-V and Ctrl-Shift-V, like a terminal.
            Some('v' | 'V') => return Edit::Paste,
            Some('w') => input.delete_word(),
            Some('u') => input.delete_to_start(),
            Some('h') => input.backspace(),
            Some('a') => input.home(),
            Some('e') => input.end(),
            _ => return Edit::Ignored,
        }
        return Edit::Changed;
    }

    insert_text(input, mods, text)
}

fn insert_text(input: &mut Input, mods: Modifiers, text: Option<&str>) -> Edit {
    match text {
        Some(t) if !mods.alt() && !mods.logo() && !t.is_empty() && !t.chars().any(char::is_control) => {
            input.insert(t);
            Edit::Changed
        }
        _ => Edit::Ignored,
    }
}

/// Dates for `:date`: 01.05.2024, 01.05.24, 01.05 (this year), 2024-05-01,
/// today, yesterday.
fn parse_date(arg: &str) -> Option<chrono::NaiveDate> {
    use chrono::{Datelike, NaiveDate};
    let arg = arg.trim();
    let today = chrono::Local::now().date_naive();
    match arg.to_lowercase().as_str() {
        "today" | "сегодня" => return Some(today),
        "yesterday" | "вчера" => return today.pred_opt(),
        _ => {}
    }
    for format in ["%d.%m.%Y", "%Y-%m-%d", "%d/%m/%Y"] {
        if let Ok(date) = NaiveDate::parse_from_str(arg, format) {
            // `%Y` also accepts "24"; treat two digits as this century.
            if date.year() < 100 {
                return date.with_year(date.year() + 2000);
            }
            return Some(date);
        }
    }
    let (day, month) = arg.split_once('.')?;
    NaiveDate::from_ymd_opt(today.year(), month.parse().ok()?, day.parse().ok()?)
}


/// Hand a URL to the system (browser, mail client); doesn't wait for it.
fn open_external(url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    // rundll32 takes the URL as one argument; `cmd /c start` would parse its `&`s.
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap it, or every opened link leaves a zombie behind.
            std::thread::spawn(move || child.wait());
        })
        .map_err(|e| e.to_string())
}

fn clean_cache(days: i32) -> Task<Message> {
    icon::clean_cache(days.max(0) as u64);
    Task::perform(tg::clean_cache(days), Message::CacheCleaned)
}

/// Clean the media cache when the last cleanup is a week old.
fn weekly_cache_clean() -> Task<Message> {
    let stamp = config::dirs().data_dir().join("last_cache_clean");
    let now = chrono::Utc::now().timestamp();
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if now - last < i64::from(CACHE_DAYS) * 86_400 {
        return Task::none();
    }
    let _ = std::fs::write(&stamp, now.to_string());
    clean_cache(CACHE_DAYS)
}

fn history_path() -> PathBuf {
    config::dirs().data_dir().join("cmdline_history")
}

/// Load the saved `/` or `:` history.
fn load_history(prefix: char) -> History {
    let mut history = History::default();
    if let Ok(text) = std::fs::read_to_string(history_path()) {
        for line in text.lines() {
            if let Some(entry) = line.strip_prefix(prefix) {
                history.push(entry);
            }
        }
    }
    history
}

fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    match (path.strip_prefix("~/"), directories::UserDirs::new()) {
        (Some(rest), Some(dirs)) => dirs.home_dir().join(rest),
        _ => PathBuf::from(path),
    }
}

/// When to send a scheduled message: `21:00` (today, or tomorrow if past),
/// `+30m` / `+2h` / `+1d`, `01.11 09:00`, `01.11.2026 09:00`.
fn parse_when(arg: &str, now: chrono::DateTime<chrono::Local>) -> Option<i32> {
    use chrono::{Datelike, Duration as Span, NaiveDate, NaiveTime};
    let arg = arg.trim();
    if let Some(rel) = arg.strip_prefix('+') {
        let (num, unit) = rel.split_at(rel.find(|c: char| !c.is_ascii_digit())?);
        let n: i64 = num.parse().ok()?;
        let span = match unit {
            "m" | "min" => Span::minutes(n),
            "h" => Span::hours(n),
            "d" => Span::days(n),
            _ => return None,
        };
        return i32::try_from((now + span).timestamp()).ok();
    }
    let (date, time) = match arg.split_once(' ') {
        Some((date, time)) => (Some(date), time),
        None => (None, arg),
    };
    let time = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
    let date = match date {
        Some(d) => NaiveDate::parse_from_str(d, "%d.%m.%Y")
            .ok()
            .or_else(|| NaiveDate::parse_from_str(&format!("{d}.{}", now.year()), "%d.%m.%Y").ok())?,
        None => now.date_naive(),
    };
    let mut at = date.and_time(time).and_local_timezone(chrono::Local).earliest()?;
    if at <= now && arg.split_once(' ').is_none() {
        at += Span::days(1);
    }
    (at > now).then(|| i32::try_from(at.timestamp()).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn schedule_times() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 10, 16, 20, 0, 0).unwrap();
        let at = |s: &str| parse_when(s, now).map(|t| chrono::Local.timestamp_opt(i64::from(t), 0).unwrap());
        assert_eq!(at("21:30"), chrono::Local.with_ymd_and_hms(2026, 10, 16, 21, 30, 0).single());
        // Already past today: tomorrow.
        assert_eq!(at("09:00"), chrono::Local.with_ymd_and_hms(2026, 10, 17, 9, 0, 0).single());
        assert_eq!(at("+30m"), Some(now + chrono::Duration::minutes(30)));
        assert_eq!(at("+2h"), Some(now + chrono::Duration::hours(2)));
        assert_eq!(at("01.11 09:15"), chrono::Local.with_ymd_and_hms(2026, 11, 1, 9, 15, 0).single());
        assert_eq!(at("01.01.2020 10:00"), None);
        assert_eq!(at("soon"), None);
    }

    fn msg(id: i64, album: i64) -> Msg {
        Msg {
            id,
            chat_id: 1,
            sender: Sender::User(1),
            outgoing: false,
            date: 0,
            edited: false,
            text: format!("m{id}"),
            media: Vec::new(),
            album,
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
            preview: None,
            comments: None,
            thread: 0,
        }
    }

    #[test]
    fn comments_of_a_thread_only() {
        let shown = Shown { tag: None, thread: 10 };
        let comment = |id, thread, reply_to| Msg { thread, reply_to, ..msg(id, 0) };
        assert!(shown.keeps(&comment(11, 10, Some(10))));
        assert!(shown.keeps(&comment(12, 10, Some(11))));
        // No thread known, but it answers the post.
        assert!(shown.keeps(&comment(13, 0, Some(10))));
        assert!(!shown.keeps(&comment(14, 20, Some(20))));
        assert!(!shown.keeps(&comment(15, 0, None)));
        assert!(Shown { tag: None, thread: 0 }.keeps(&comment(15, 0, None)));
        // Answering the post is not quoted; answering a comment is.
        assert_eq!(comment(11, 10, Some(10)).in_thread(10).reply_to, None);
        assert_eq!(comment(12, 10, Some(11)).in_thread(10).reply_to, Some(11));
        assert_eq!(comment(16, 0, Some(10)).in_thread(0).reply_to, Some(10));
    }

    #[test]
    fn album_comments_come_from_any_part() {
        let mut chat = OpenChat::new(1);
        let part = |id, comments| Msg { comments, ..msg(id, 7) };
        chat.edit(|m| m.extend([part(1, None), part(2, Some(4))]));
        assert_eq!(chat.msgs[0].comments, Some(4));
    }

    #[test]
    fn pinned_bar_follows_the_cursor() {
        let mut chat = OpenChat::new(1);
        assert!(chat.shown_pin().is_none());
        chat.pins = vec![msg(8, 0), msg(5, 0), msg(2, 0)];
        chat.edit(|m| m.extend((1..=10).map(|id| msg(id, 0))));
        let mut shown = |cursor_id: i64| {
            chat.cursor = chat.position(cursor_id).unwrap();
            chat.shown_pin().map(|(i, p)| (i, p.id))
        };
        assert_eq!(shown(10), Some((0, 8)));
        assert_eq!(shown(8), Some((0, 8)));
        assert_eq!(shown(7), Some((1, 5)));
        assert_eq!(shown(2), Some((2, 2)));
        // Above all of them: the oldest.
        assert_eq!(shown(1), Some((2, 2)));

        chat.fill_pins(Some(5));
        assert!(chat.pins_only());
        assert_eq!(chat.msgs.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 5, 8]);
        assert_eq!(chat.cursor, 1);
        assert!(!chat.shown().keeps(&msg(11, 0)));
    }

    #[test]
    fn albums_merge_in_any_order() {
        let mut chat = OpenChat::new(1);
        chat.edit(|m| m.extend([msg(1, 0), msg(3, 7), msg(4, 7)]));
        assert_eq!(chat.msgs.len(), 2);
        // An older part of the album arrives later (previous history page).
        chat.edit(|m| m.push(msg(2, 7)));
        assert_eq!(chat.msgs.len(), 2);
        assert_eq!(chat.msgs[1].members, vec![2, 3, 4]);
        assert_eq!(chat.position(4), Some(1));
        // Seeing a part again does not duplicate it.
        chat.edit(|m| m.push(msg(3, 7)));
        assert_eq!(chat.msgs[1].members, vec![2, 3, 4]);
    }

    #[test]
    fn edits_keep_the_cursor_on_its_message() {
        let mut chat = OpenChat::new(1);
        chat.edit(|m| m.extend((10..20).map(|id| msg(id, 0))));
        chat.cursor = chat.position(15).unwrap();
        chat.edit(|m| m.extend((0..10).map(|id| msg(id, 0))));
        assert_eq!(chat.msgs[chat.cursor].id, 15);
        // Deleting the cursor's message moves it to the next one.
        chat.edit(|m| m.retain(|x| x.id != 15));
        assert_eq!(chat.msgs[chat.cursor].id, 16);
    }

    #[test]
    fn deleted_messages_from_the_archive_never_replace_live_ones() {
        let mut chat = OpenChat::new(1);
        chat.edit(|m| m.extend([msg(1, 0), msg(2, 0)]));
        let mut ghost = msg(2, 0);
        ghost.deleted = true;
        ghost.text = "old".into();
        let mut gone = msg(3, 0);
        gone.deleted = true;
        chat.edit(|m| m.extend([ghost, gone]));
        assert_eq!(chat.msgs.len(), 3);
        assert!(!chat.msgs[1].deleted);
        assert!(chat.msgs[2].deleted);
    }

    #[test]
    fn viewer_walks_photos_across_albums() {
        use crate::tg::FileRef;
        let photo = || Media::Photo {
            preview: FileRef { id: 1, width: 10, height: 10, size: 0 },
            full: FileRef { id: 2, width: 10, height: 10, size: 0 },
        };
        let mut chat = OpenChat::new(1);
        let mut a = msg(1, 0);
        a.media = vec![photo(), photo()];
        let b = msg(2, 0);
        let mut c = msg(3, 0);
        c.media = vec![photo()];
        chat.edit(|m| m.extend([a, b, c]));
        let start = Viewer { msg_id: 1, item: 1 };
        assert_eq!(chat.viewable_near(start, 1), Some(Viewer { msg_id: 3, item: 0 }));
        assert_eq!(chat.viewable_near(start, -1), Some(Viewer { msg_id: 1, item: 0 }));
        assert_eq!(chat.viewable_near(Viewer { msg_id: 3, item: 0 }, 1), None);
    }

    #[test]
    fn themes_by_loose_name() {
        assert_eq!(find_theme("tokyo night"), Some(Theme::TokyoNight));
        assert_eq!(find_theme("Catppuccin-Mocha"), Some(Theme::CatppuccinMocha));
        assert_eq!(find_theme("nope"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("01.05.2024"), NaiveDate::from_ymd_opt(2024, 5, 1));
        assert_eq!(parse_date("2024-05-01"), NaiveDate::from_ymd_opt(2024, 5, 1));
        assert_eq!(parse_date("01.05.24"), NaiveDate::from_ymd_opt(2024, 5, 1));
        assert!(parse_date("01.05").is_some());
        assert!(parse_date("yesterday").is_some());
        assert_eq!(parse_date("nonsense"), None);
    }

    #[test]
    fn folders_by_loose_name() {
        let mut folders = tg::folders(tdlib_rs::types::UpdateChatFolders::default());
        folders.push(Folder { name: "Личка".into(), ..folders[0].clone() });
        assert_eq!(find_folder(&folders, "личка"), Some(folders.len() - 1));
        assert_eq!(find_folder(&folders, " ЛИЧКА "), Some(folders.len() - 1));
        assert_eq!(find_folder(&folders, "work"), None);
    }

    #[test]
    fn found_messages_then_posts() {
        let global = Global {
            msgs: vec![msg(1, 0), msg(2, 0)],
            posts: vec![msg(7, 0)],
            ..Default::default()
        };
        assert_eq!(global.hits(), 3);
        let ids: Vec<i64> = (0..4).filter_map(|i| global.hit(i)).map(|m| m.id).collect();
        assert_eq!(ids, [1, 2, 7]);
    }
}
