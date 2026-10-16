//! Rendering. Lists are virtualized: only rows that fit the window are built,
//! so thousands of chats or messages cost nothing extra per frame.

use chrono::{DateTime, Local};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, center, column, container, image, mouse_area, progress_bar, row, rule, scrollable, space,
    stack, text, tooltip,
};
use iced::mouse::Interaction;
use iced::{Alignment, Color, ContentFit, Element, Length, Size, Theme};
use iced_video_player::VideoPlayer;

use crate::app::{App, Auth, Chat, Compose, Confirm, Focus, Message, Pane, TALL_ID, VideoPlayback};
use crate::emoji::{self, Picker};
use crate::media::Loaded;
use crate::player;
use crate::tg::{ChatKind, Media, Msg, Presence, Sender, fmt_duration};
use crate::vim::{Input, Mode};

pub const FOLDER_W: f32 = 76.0;
const FOLDER_ROW_H: f32 = 64.0;
pub const SIDEBAR_W: f32 = 340.0;
pub const CHAT_ROW_H: f32 = 72.0;
/// Two lines of 13px preview text.
const PREVIEW_H: f32 = 34.0;
const AVATAR: f32 = 42.0;
pub const STATUS_H: f32 = 26.0;
pub const HEADER_H: f32 = 44.0;
const TEXT: f32 = 15.0;
const LINE_H: f32 = TEXT * 1.35;
const MSG_PAD_X: f32 = 16.0;
const MSG_SPACING: f32 = 2.0;
const MAX_COMPOSE_LINES: usize = 8;
const BANNER_H: f32 = 24.0;
const REPLY_H: f32 = 22.0;
const PHOTO_MAX: Size = Size::new(420.0, 380.0);
const VIDEO_MAX: Size = Size::new(420.0, 340.0);
const ROUND_VIDEO: f32 = 220.0;
const GIF_MAX: Size = Size::new(360.0, 300.0);
const STICKER_MAX: Size = Size::new(170.0, 170.0);
const VOICE: Size = Size::new(380.0, 30.0);
const VOICE_BARS: usize = 48;
const MEDIA_GAP: f32 = 4.0;
/// Sender avatar next to messages.
const MSG_AVATAR: f32 = 34.0;
const MSG_AVATAR_GAP: f32 = 8.0;
const HEADER_AVATAR: f32 = 32.0;
/// Two lines of the pinned message.
const PINNED_TEXT_H: f32 = 13.0 * 1.3 * 2.0;
const PINNED_H: f32 = 11.0 * 1.3 + PINNED_TEXT_H + 13.0;
/// Bar of a voice message playing in another chat.
const PLAYER_H: f32 = 44.0;
/// Space above the visible messages worth filling with the end of the
/// previous (too tall) message.
const MIN_PARTIAL: f32 = 40.0;
const ALBUM_GAP: f32 = 4.0;
/// Emoji panel: grid cell, the tab bar and search line above the grid,
/// the name line below it.
const EMOJI_CELL: f32 = 36.0;
const EMOJI_PAD: f32 = 8.0;
const EMOJI_TABS_H: f32 = 34.0;
const EMOJI_LINE_H: f32 = 22.0;
pub const EMOJI_W: f32 = EMOJI_CELL * emoji::COLS as f32 + 2.0 * EMOJI_PAD + 1.0;

/// Grid rows that fit next to a message list of this height.
pub fn emoji_rows(height: f32) -> usize {
    (((height - EMOJI_TABS_H - 2.0 * EMOJI_LINE_H - 2.0 * EMOJI_PAD) / EMOJI_CELL).floor() as usize).max(1)
}

/// Date separator above the first message of each day.
const DAY_H: f32 = 30.0;

pub fn chat_rows(size: Size) -> usize {
    (((size.height - STATUS_H - HEADER_H) / CHAT_ROW_H).floor() as usize).max(1)
}

fn compose_height(input: &Input, banner: bool) -> f32 {
    let lines = input.as_str().split('\n').count().clamp(1, MAX_COMPOSE_LINES);
    20.0 + lines as f32 * LINE_H + if banner { BANNER_H } else { 0.0 }
}

/// Width and height available to the message list.
pub fn messages_viewport(
    size: Size,
    compose: &Input,
    banner: bool,
    pinned: bool,
    emoji: bool,
    player: bool,
) -> (f32, f32) {
    let panel = if emoji { EMOJI_W } else { 0.0 };
    let width = size.width - FOLDER_W - SIDEBAR_W - 1.0 - 2.0 * MSG_PAD_X - MSG_AVATAR - MSG_AVATAR_GAP - panel;
    let pinned = if pinned { PINNED_H + 1.0 } else { 0.0 };
    let player = if player { PLAYER_H + 1.0 } else { 0.0 };
    let height = size.height - STATUS_H - HEADER_H - 2.0 - pinned - player - compose_height(compose, banner);
    (width.max(100.0), height.max(LINE_H))
}

/// On-screen size of a single media item: scaled down to fit, never up.
pub fn media_size(media: &Media, width: f32) -> Size {
    let max = match media {
        Media::Photo { .. } => PHOTO_MAX,
        Media::Video { round: true, .. } => Size::new(ROUND_VIDEO, ROUND_VIDEO),
        Media::Video { .. } => VIDEO_MAX,
        Media::Animation { .. } => GIF_MAX,
        Media::Sticker { .. } => STICKER_MAX,
        Media::Voice { .. } => return Size::new(VOICE.width.min(width - 20.0), VOICE.height),
    };
    let bounds = Size::new(max.width.min(width - 20.0).max(40.0), max.height);
    let (w, h) = media.dims().map_or((1.0, 1.0), |(w, h)| (w as f32, h as f32));
    let scale = (bounds.width / w).min(bounds.height / h);
    // Small photos are not blown up, but round videos always fill their circle.
    let scale = if matches!(media, Media::Video { round: true, .. }) { scale } else { scale.min(1.0) };
    Size::new((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// Album grid: columns and the size of one square cell.
fn album_layout(count: usize, width: f32) -> (usize, Size) {
    let cols = match count {
        1 => 1,
        2 | 4 => 2,
        _ => 3,
    };
    let total = PHOTO_MAX.width.min(width - 20.0).max(60.0);
    let cell = ((total - ALBUM_GAP * (cols - 1) as f32) / cols as f32).floor();
    (cols, Size::new(cell, cell))
}

/// Size of every media item of a message, in order.
pub fn media_sizes(media: &[Media], width: f32) -> Vec<Size> {
    if media.len() <= 1 {
        media.iter().map(|m| media_size(m, width)).collect()
    } else {
        vec![album_layout(media.len(), width).1; media.len()]
    }
}

fn media_block_height(media: &[Media], width: f32) -> f32 {
    match media.len() {
        0 => 0.0,
        1 => media_size(&media[0], width).height + MEDIA_GAP,
        n => {
            let (cols, cell) = album_layout(n, width);
            let rows = n.div_ceil(cols) as f32;
            rows * cell.height + (rows - 1.0) * ALBUM_GAP + MEDIA_GAP
        }
    }
}

/// Emoji and CJK take about two columns of Latin text.
fn display_width(line: &str) -> usize {
    line.chars().map(|c| if (c as u32) < 0x2e80 { 1 } else { 2 }).sum()
}

/// Deliberately pessimistic height estimate, so whatever we consider
/// visible fits. Media sizes are exact.
pub fn message_height(m: &Msg, width: f32) -> f32 {
    let per_line = ((width - 20.0) / (TEXT * 0.6)).max(10.0) as usize;
    let lines: usize = if m.text.is_empty() {
        0
    } else {
        m.text
            .split('\n')
            .map(|l| display_width(l).max(1).div_ceil(per_line))
            .sum()
    };
    let reply = if m.reply_to.is_some() { REPLY_H } else { 0.0 };
    12.0 + 18.0 + reply + media_block_height(&m.media, width) + lines as f32 * LINE_H + MSG_SPACING
}

fn local_day(unix: i32) -> Option<chrono::NaiveDate> {
    Some(DateTime::from_timestamp(i64::from(unix), 0)?.with_timezone(&Local).date_naive())
}

/// The message is the first one of its day among the loaded ones.
pub fn starts_day(msgs: &[Msg], i: usize) -> bool {
    match i.checked_sub(1).and_then(|p| msgs.get(p)) {
        Some(prev) => local_day(prev.date) != local_day(msgs[i].date),
        None => true,
    }
}

/// Height of message `i` in the list, with its date separator.
pub fn item_height(msgs: &[Msg], i: usize, width: f32) -> f32 {
    message_height(&msgs[i], width) + if starts_day(msgs, i) { DAY_H } else { 0.0 }
}

/// "Today", "Yesterday", "Monday, 13 October", "2 March 2024".
pub fn fmt_day(unix: i32) -> String {
    let Some(day) = local_day(unix) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let days_ago = (today - day).num_days();
    match days_ago {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A, %-d %B").to_string(),
        _ if day.format("%Y").to_string() == today.format("%Y").to_string() => day.format("%-d %B").to_string(),
        _ => day.format("%-d %B %Y").to_string(),
    }
}

fn day_separator<'a>(unix: i32) -> Element<'a, Message> {
    container(
        container(
            text(fmt_day(unix))
                .size(12)
                .style(|t: &Theme| text::Style { color: Some(t.palette().text.scale_alpha(0.85)) }),
        )
            .padding([2.0, 10.0])
            .style(|t: &Theme| container::Style {
                background: Some(t.extended_palette().background.weak.color.into()),
                border: iced::border::rounded(10.0),
                ..Default::default()
            }),
    )
    .width(Length::Fill)
    .height(DAY_H - MSG_SPACING)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .into()
}

/// A message of the list, below its date separator when it starts a day.
fn list_item<'a>(app: &'a App, msgs: &'a [Msg], i: usize, selected: bool, width: f32) -> Element<'a, Message> {
    let message = message_view(app, &msgs[i], selected, width, starts_run(msgs, i));
    if starts_day(msgs, i) {
        column![day_separator(msgs[i].date), message].spacing(MSG_SPACING).into()
    } else {
        message
    }
}

/// First message that still fits entirely when `bottom` is the last one shown.
pub fn top_for_bottom(msgs: &[Msg], bottom: usize, width: f32, height: f32) -> usize {
    let mut used = 0.0;
    for i in (0..=bottom).rev() {
        used += item_height(msgs, i, width);
        if used > height {
            return (i + 1).min(bottom);
        }
    }
    0
}

/// Last message that still fits entirely when `top` is the first one shown.
pub fn bottom_for_top(msgs: &[Msg], top: usize, width: f32, height: f32) -> usize {
    let mut used = 0.0;
    for i in top..msgs.len() {
        used += item_height(msgs, i, width);
        if used > height {
            return i.saturating_sub(1).max(top);
        }
    }
    msgs.len().saturating_sub(1)
}

pub fn view(app: &App, _window: iced::window::Id) -> Element<'_, Message> {
    let body: Element<_> = if app.auth != Auth::Ready {
        auth_screen(app)
    } else if let Some(viewer) = media_viewer(app) {
        viewer
    } else {
        row![
            // The folder strip's own background separates it from the list.
            folder_strip(app),
            chat_list(app),
            rule::vertical(1),
            chat_pane(app)
        ]
        .height(Length::Fill)
        .into()
    };
    column![body, status_line(app)].into()
}

/// Secondary text: the text color, faded. Readable on the background and
/// on highlighted rows alike.
fn muted(theme: &Theme) -> Color {
    theme.palette().text.scale_alpha(0.6)
}

fn muted_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(muted(theme)) }
}

fn primary_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.palette().primary) }
}

fn fmt_time(date: i32, short: bool) -> String {
    let Some(utc) = DateTime::from_timestamp(i64::from(date), 0) else {
        return String::new();
    };
    let local = utc.with_timezone(&Local);
    if local.date_naive() == Local::now().date_naive() {
        local.format("%H:%M").to_string()
    } else if short {
        local.format("%d.%m.%y").to_string()
    } else {
        local.format("%d.%m.%y %H:%M").to_string()
    }
}

pub fn fmt_datetime(unix: i32) -> String {
    DateTime::from_timestamp(i64::from(unix), 0)
        .map(|d| d.with_timezone(&Local).format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_default()
}

/// Full date for the hover tooltip: "Thursday, 16 October 2026, 19:42:07".
fn fmt_full_date(unix: i32) -> String {
    DateTime::from_timestamp(i64::from(unix), 0)
        .map(|d| d.with_timezone(&Local).format("%A, %-d %B %Y, %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub fn fmt_presence(presence: Presence) -> String {
    match presence {
        Presence::Online => "online".into(),
        Presence::Offline(at) => {
            let Some(utc) = DateTime::from_timestamp(i64::from(at), 0) else {
                return String::new();
            };
            let local = utc.with_timezone(&Local);
            let today = Local::now().date_naive();
            if local.date_naive() == today {
                format!("last seen today at {}", local.format("%H:%M"))
            } else if today.pred_opt() == Some(local.date_naive()) {
                format!("last seen yesterday at {}", local.format("%H:%M"))
            } else {
                format!("last seen {}", local.format("%d.%m.%y"))
            }
        }
        Presence::Recently => "last seen recently".into(),
        Presence::LastWeek => "last seen within a week".into(),
        Presence::LastMonth => "last seen within a month".into(),
        Presence::Unknown => String::new(),
    }
}

/// Delivery mark of an outgoing message: sending, sent, read.
fn delivery_mark(app: &App, m: &Msg) -> Option<(&'static str, bool)> {
    if !m.outgoing || m.deleted {
        return None;
    }
    if m.sending {
        return Some(("🕓", false));
    }
    let read = app.chats.get(&m.chat_id).is_some_and(|c| c.read_outbox >= m.id);
    Some(if read { ("✓✓", true) } else { ("✓", false) })
}

fn mark_text<'a>(mark: Option<(&'static str, bool)>) -> Option<Element<'a, Message>> {
    let (mark, read) = mark?;
    Some(
        text(mark)
            .size(12)
            .style(move |t: &Theme| if read { primary_text(t) } else { muted_text(t) })
            .into(),
    )
}

fn fmt_secs(secs: f64) -> String {
    fmt_duration(secs.round() as i32)
}

fn badge<'a>(count: impl ToString) -> Element<'a, Message> {
    // Digits have no descenders and sit high in a normal line box: a tight
    // line and a pixel more room on top center them visually.
    container(text(count.to_string()).size(11).line_height(1.0))
        .height(18)
        .padding(iced::Padding { top: 1.0, right: 7.0, bottom: 0.0, left: 7.0 })
        .align_y(Alignment::Center)
        .style(|t: &Theme| {
            let p = t.extended_palette().primary.base;
            container::Style {
                background: Some(p.color.into()),
                text_color: Some(p.text),
                border: iced::border::rounded(9),
                ..Default::default()
            }
        })
        .into()
}

fn folder_strip(app: &App) -> Element<'_, Message> {
    let items = app.folders.iter().enumerate().map(|(i, folder)| {
        let selected = i == app.folder;
        let unread = app.folder_unread.get(&folder.key).copied().unwrap_or(0);
        let label = column![
            text(folder.icon).size(22),
            container(text(&folder.name).size(11).wrapping(Wrapping::None))
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .clip(true),
        ]
        .spacing(4)
        .align_x(Alignment::Center);
        let mut item = stack![
            container(label)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding([10, 4])
                .align_x(Alignment::Center)
        ];
        // Like Telegram: the unread counter sits on the icon's corner.
        if unread > 0 {
            item = item.push(
                container(badge(if unread > 99 { "99+".to_owned() } else { unread.to_string() }))
                    .width(Length::Fill)
                    .padding([4, 6])
                    .align_x(Alignment::End),
            );
        }
        mouse_area(
            container(item)
                .width(Length::Fill)
                .height(FOLDER_ROW_H)
                .clip(true)
                .style(move |t: &Theme| selection_style(t, selected, true)),
        )
        .on_press(Message::ClickFolder(i))
        .interaction(Interaction::Pointer)
        .into()
    });

    mouse_area(
        container(Column::with_children(items))
            .width(FOLDER_W)
            .height(Length::Fill)
            .style(|t: &Theme| container::Style {
                background: Some(t.extended_palette().background.weakest.color.into()),
                ..Default::default()
            }),
    )
    .on_scroll(|delta| Message::Scroll(Pane::Folders, delta))
    .into()
}

fn chat_list(app: &App) -> Element<'_, Message> {
    let focused = app.focus == Focus::Chats;
    let folder = app.folders.get(app.folder).map_or("", |f| f.name.as_str());
    let title = if app.forward.is_some() {
        "Forward to… (Enter — send, Esc — cancel)".to_owned()
    } else if app.filter.is_empty() {
        format!("{folder}  {}", app.list.len())
    } else {
        format!("{folder} /{}  {}", app.filter, app.list.len())
    };
    let forwarding = app.forward.is_some();
    let header = container(text(title).size(13).style(move |t: &Theme| {
        if forwarding { primary_text(t) } else { muted_text(t) }
    }))
    .height(HEADER_H)
    .padding([0, 12])
    .align_y(Alignment::Center);

    let rows = app
        .list
        .iter()
        .skip(app.chat_top)
        .take(chat_rows(app.size))
        .filter_map(|id| app.chats.get(id))
        .map(|chat| chat_row(app, chat, app.selected == Some(chat.id), focused));

    mouse_area(
        column![header, Column::with_children(rows)]
            .width(SIDEBAR_W)
            .height(Length::Fill),
    )
    .on_scroll(|delta| Message::Scroll(Pane::Chats, delta))
    .into()
}

const AVATAR_COLORS: [Color; 7] = [
    Color::from_rgb8(0xe1, 0x70, 0x76),
    Color::from_rgb8(0xf0, 0x9a, 0x5a),
    Color::from_rgb8(0xa6, 0x95, 0xe7),
    Color::from_rgb8(0x6c, 0xb8, 0x5a),
    Color::from_rgb8(0x4f, 0xb5, 0xb7),
    Color::from_rgb8(0x5a, 0x9c, 0xd6),
    Color::from_rgb8(0xe0, 0x6d, 0xa4),
];

fn initials(title: &str) -> String {
    title
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

fn avatar<'a>(app: &'a App, chat: &'a Chat) -> Element<'a, Message> {
    avatar_image(app, chat.avatar, chat.id, &chat.title, AVATAR)
}

/// Round photo, or colored initials while there is none.
fn avatar_image<'a>(app: &'a App, file: Option<i32>, id: i64, name: &str, size: f32) -> Element<'a, Message> {
    if let Some(handle) = file.and_then(|file| app.media.image(file)) {
        return image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(size / 2.0)
            .into();
    }
    let color = AVATAR_COLORS[id.unsigned_abs() as usize % AVATAR_COLORS.len()];
    container(text(initials(name)).size(size * 0.36))
        .width(size)
        .height(size)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            text_color: Some(Color::WHITE),
            border: iced::border::rounded(size / 2.0),
            ..Default::default()
        })
        .into()
}

fn chat_row<'a>(app: &'a App, chat: &'a Chat, selected: bool, focused: bool) -> Element<'a, Message> {
    let time = chat.last.as_ref().map(|m| fmt_time(m.date, true)).unwrap_or_default();
    let preview = chat
        .last
        .as_ref()
        .map(|m| {
            // Newlines would waste the two preview lines.
            let summary: String = m.summary().split_whitespace().collect::<Vec<_>>().join(" ");
            let line: String = summary.chars().take(200).collect();
            if m.outgoing { format!("You: {line}") } else { line }
        })
        .unwrap_or_default();

    // Text is clipped to its own column so it never runs under the time.
    let mut title_line = row![
        container(text(&chat.title).size(TEXT).wrapping(Wrapping::None))
            .width(Length::Fill)
            .clip(true),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if let Some(mark) = mark_text(chat.last.as_ref().and_then(|m| delivery_mark(app, m))) {
        title_line = title_line.push(mark);
    }
    let title_line = title_line.push(text(time).size(12).style(muted_text));

    let mut preview_line = row![
        container(
            text(preview)
                .size(13)
                .wrapping(Wrapping::WordOrGlyph)
                .style(muted_text)
        )
        .width(Length::Fill)
        .height(PREVIEW_H)
        .clip(true)
    ]
    .spacing(8);
    if chat.unread > 0 {
        preview_line = preview_line.push(badge(chat.unread));
    }

    mouse_area(
        container(
            row![avatar(app, chat), column![title_line, preview_line].spacing(4)]
                .spacing(10)
                .align_y(Alignment::Center),
        )
        .height(CHAT_ROW_H)
        .width(Length::Fill)
        .padding([7, 10])
        .clip(true)
        .style(move |t: &Theme| selection_style(t, selected, focused)),
    )
    .on_press(Message::ClickChat(chat.id))
    .interaction(Interaction::Pointer)
    .into()
}

/// Selected row: a tint of the accent color when the pane has focus, a
/// neutral lift otherwise. The text keeps the theme's own colors, so it
/// stays readable either way.
fn selection_style(t: &Theme, selected: bool, focused: bool) -> container::Style {
    if !selected {
        return container::Style::default();
    }
    let p = t.extended_palette();
    if focused {
        container::Style {
            background: Some(p.primary.base.color.scale_alpha(0.22).into()),
            border: iced::border::width(1).color(p.primary.base.color.scale_alpha(0.5)),
            ..Default::default()
        }
    } else {
        container::Style {
            background: Some(p.background.weak.color.scale_alpha(0.7).into()),
            ..Default::default()
        }
    }
}

/// The message under the cursor: a neutral lift with a thin accent
/// outline, so sender colors, times and ticks stay readable.
fn message_style(t: &Theme, selected: bool) -> container::Style {
    let mut style = container::Style::default();
    if selected {
        let p = t.extended_palette();
        style.background = Some(p.background.weak.color.scale_alpha(0.6).into());
        style.border = style.border.width(1).color(p.primary.base.color.scale_alpha(0.45));
    }
    style
}

fn sender_color(t: &Theme, sender: Sender) -> Color {
    let p = t.palette();
    let colors = [p.primary, p.success, p.warning, p.danger];
    let id = match sender {
        Sender::User(id) | Sender::Chat(id) => id,
    };
    colors[id.unsigned_abs() as usize % colors.len()]
}

/// A voice message from another chat keeps playing: what it is, its
/// progress, and a way back to it (click or `gv`).
fn player_bar(app: &App) -> Option<Element<'_, Message>> {
    let voice = app.playing_elsewhere()?;
    let chat = app.chats.get(&voice.chat_id).map_or("", |c| c.title.as_str());
    let sender = app.sender_name(voice.sender);
    let who = if sender == chat || chat.is_empty() { sender.to_owned() } else { format!("{sender} · {chat}") };
    let (icon, progress, time) = match &voice.audio {
        Some(audio) => {
            let total = audio.duration().map_or(0.0, |d| d.as_secs_f64()).max(0.001);
            let pos = audio.position().as_secs_f64();
            let icon = if audio.paused() { "▶" } else { "⏸" };
            (icon, (pos / total) as f32, format!("{} / {}", fmt_secs(pos), fmt_secs(total)))
        }
        None => ("…", 0.0, "loading…".to_owned()),
    };
    let button = |label: &'static str, message: Message| {
        mouse_area(
            container(text(label).size(16).style(primary_text))
                .width(28)
                .height(28)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press(message)
        .interaction(Interaction::Pointer)
    };
    let info = column![
        row![
            text(format!("🎤 {who}")).size(13).wrapping(Wrapping::None).width(Length::Fill),
            text(format!("{time}  {}  gv", player::fmt_speed(app.speed()))).size(12).style(muted_text),
        ]
        .spacing(8),
        progress_bar(0.0..=1.0, progress).girth(3),
    ]
    .spacing(5)
    .width(Length::Fill);
    let bar = row![
        button(icon, Message::PauseVoice),
        mouse_area(info).on_press(Message::JumpPlaying).interaction(Interaction::Pointer),
        button("✕", Message::StopVoice),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    Some(
        column![
            container(bar)
                .height(PLAYER_H)
                .width(Length::Fill)
                .padding([0.0, MSG_PAD_X])
                .align_y(Alignment::Center)
                .clip(true),
            rule::horizontal(1),
        ]
        .into(),
    )
}

fn chat_pane(app: &App) -> Element<'_, Message> {
    let bar = player_bar(app);
    let Some(open) = &app.open else {
        let hint = center(
            text("j/k — move, Enter — open chat, J/K — folders, :help — keys")
                .size(14)
                .style(muted_text),
        );
        return match bar {
            Some(bar) => column![bar, hint].into(),
            None => hint.into(),
        };
    };

    let chat = app.chats.get(&open.id);
    let title = chat.map_or("", |c| c.title.as_str());
    let mut details: Vec<String> = Vec::new();
    if let Some(presence) = app.presence(open.id).map(fmt_presence).filter(|p| !p.is_empty()) {
        details.push(presence);
    }
    if let Some(total) = open.total {
        details.push(format!("{total} messages"));
    }
    if !open.at_latest {
        details.push("viewing history — G for latest".into());
    }
    let online = app.presence(open.id) == Some(Presence::Online);
    let header_avatar = match chat {
        Some(chat) => avatar_image(app, chat.avatar, chat.id, &chat.title, HEADER_AVATAR),
        None => space::horizontal().width(HEADER_AVATAR).into(),
    };
    let header = container(
        row![
            header_avatar,
            column![
                text(title).size(15).wrapping(Wrapping::None),
                text(details.join(" · "))
                    .size(12)
                    .wrapping(Wrapping::None)
                    .style(move |t: &Theme| if online { primary_text(t) } else { muted_text(t) }),
            ]
            .spacing(1),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .height(HEADER_H)
    .width(Length::Fill)
    .padding([0.0, MSG_PAD_X])
    .align_y(Alignment::Center)
    .clip(true);

    let selected = (app.focus == Focus::Messages).then_some(open.cursor);
    let messages: Element<_> = if open.msgs.is_empty() {
        center(text(if open.loading { "Loading…" } else { "No messages" }).size(14)).into()
    } else if app.is_tall() {
        // Taller than the screen: the message gets its own scrollable,
        // moved by j/k (and the wheel) before the cursor moves on.
        let (width, _) = app.viewport();
        let i = open.cursor;
        let item = message_view(app, &open.msgs[i], selected == Some(i), width, starts_run(&open.msgs, i));
        scrollable(container(item).padding([0.0, MSG_PAD_X]))
            .id(TALL_ID)
            .on_scroll(Message::TallScroll)
            .direction(Direction::Vertical(Scrollbar::new().width(4).scroller_width(4)))
            .height(Length::Fill)
            .into()
    } else {
        let (width, height) = app.viewport();
        let top = top_for_bottom(&open.msgs, open.bottom, width, height);
        let used: f32 = (top..=open.bottom).map(|i| item_height(&open.msgs, i, width)).sum();
        let leftover = height - used;
        let items = (top..=open.bottom).map(|i| list_item(app, &open.msgs, i, selected == Some(i), width));
        // The spacer pushes messages to the bottom; heights are estimated
        // pessimistically, so the rendered range always fits.
        let mut children: Vec<Element<_>> = vec![space::vertical().height(Length::Fill).into()];
        // A message too tall to fit above the visible ones still shows its
        // end instead of leaving the space empty.
        if top > 0 && leftover >= MIN_PARTIAL {
            let i = top - 1;
            let partial = list_item(app, &open.msgs, i, false, width);
            children.push(
                scrollable(partial)
                    .direction(Direction::Vertical(Scrollbar::hidden()))
                    .anchor_bottom()
                    .height(leftover - MSG_SPACING)
                    .into(),
            );
        }
        children.extend(items);
        container(
            Column::with_children(children)
                .spacing(MSG_SPACING)
                .padding([0.0, MSG_PAD_X])
                .height(Length::Fill),
        )
        .height(Length::Fill)
        .clip(true)
        .into()
    };

    let mut pane = column![];
    if let Some(bar) = bar {
        pane = pane.push(bar);
    }
    let mut pane = pane.push(header).push(rule::horizontal(1));
    if let Some(pinned) = &open.pinned {
        pane = pane.push(pinned_bar(pinned)).push(rule::horizontal(1));
    }
    let messages: Element<_> = mouse_area(messages)
        .on_scroll(|delta| Message::Scroll(Pane::Messages, delta))
        .into();
    let messages: Element<_> = match &app.emoji {
        Some(picker) => row![messages, rule::vertical(1), emoji_panel(app, picker)]
            .height(Length::Fill)
            .into(),
        None => messages,
    };
    pane.push(messages)
        .push(rule::horizontal(1))
        .push(compose(app))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A message starts a run of messages from the same sender, which shows
/// the avatar (gaps of over 10 minutes and new days start a new run too).
fn starts_run(msgs: &[Msg], i: usize) -> bool {
    match i.checked_sub(1).and_then(|p| msgs.get(p)) {
        Some(prev) => {
            prev.sender != msgs[i].sender || msgs[i].date - prev.date > 600 || starts_day(msgs, i)
        }
        None => true,
    }
}

fn emoji_panel<'a>(app: &'a App, picker: &'a Picker) -> Element<'a, Message> {
    let tabs = Row::with_children(emoji::TABS.iter().enumerate().map(|(i, (icon, _))| {
        let active = i == picker.tab && picker.query.is_empty();
        mouse_area(
            container(text(*icon).size(17))
                .width(EMOJI_CELL * emoji::COLS as f32 / emoji::TABS.len() as f32)
                .height(EMOJI_TABS_H - 4.0)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |t: &Theme| selection_style(t, active, true)),
        )
        .on_press(Message::EmojiTab(i))
        .interaction(Interaction::Pointer)
        .into()
    }));

    let search = if picker.searching {
        format!("/{}▏", picker.query)
    } else if !picker.query.is_empty() {
        format!("/{}   Esc — close", picker.query)
    } else {
        "/ search   Tab groups   Enter insert".to_owned()
    };

    let items = picker.items(&app.recent_emoji);
    let (_, height) = app.viewport();
    let rows = emoji_rows(height);
    let grid: Element<_> = if items.is_empty() {
        text(if picker.query.is_empty() { "Nothing here yet" } else { "No emoji found" })
            .size(13)
            .style(muted_text)
            .into()
    } else {
        Column::with_children(
            items
                .chunks(emoji::COLS)
                .enumerate()
                .skip(picker.top)
                .take(rows)
                .map(|(row_i, chunk)| {
                    Row::with_children(chunk.iter().enumerate().map(|(col, e)| {
                        let selected = row_i * emoji::COLS + col == picker.cursor;
                        mouse_area(
                            container(text(*e).size(22))
                                .width(EMOJI_CELL)
                                .height(EMOJI_CELL)
                                .align_x(Alignment::Center)
                                .align_y(Alignment::Center)
                                .style(move |t: &Theme| selection_style(t, selected, true)),
                        )
                        .on_press(Message::EmojiPick(e))
                        .interaction(Interaction::Pointer)
                        .into()
                    }))
                    .into()
                }),
        )
        .into()
    };
    let name = items.get(picker.cursor).map_or("", |e| emoji::name(e));

    mouse_area(
        container(
            column![
                tabs,
                container(text(search).size(12).style(muted_text).wrapping(Wrapping::None))
                    .height(EMOJI_LINE_H)
                    .align_y(Alignment::Center)
                    .clip(true),
                container(grid).height(Length::Fill),
                container(text(name).size(12).style(muted_text).wrapping(Wrapping::None))
                    .height(EMOJI_LINE_H)
                    .align_y(Alignment::Center)
                    .clip(true),
            ]
            .spacing(0),
        )
        .width(EMOJI_W - 1.0)
        .height(Length::Fill)
        .padding(EMOJI_PAD)
        .clip(true),
    )
    .on_scroll(|delta| Message::Scroll(Pane::Emoji, delta))
    .into()
}

fn pinned_bar(pinned: &Msg) -> Element<'_, Message> {
    let summary: String = pinned.summary().split_whitespace().collect::<Vec<_>>().join(" ");
    let summary: String = summary.chars().take(400).collect();
    mouse_area(
        container(
            row![
                text("📌").size(16),
                column![
                    text("Pinned message   gp").size(11).style(primary_text),
                    // Two lines at most, wrapped within the pane.
                    container(text(summary).size(13).wrapping(Wrapping::WordOrGlyph).width(Length::Fill))
                        .width(Length::Fill)
                        .max_height(PINNED_TEXT_H)
                        .clip(true),
                ]
                .width(Length::Fill)
                .spacing(1),
            ]
            .width(Length::Fill)
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .height(PINNED_H)
        .width(Length::Fill)
        .padding([0.0, MSG_PAD_X])
        .align_y(Alignment::Center)
        .clip(true),
    )
    .on_press(Message::JumpPinned)
    .interaction(Interaction::Pointer)
    .into()
}

fn placeholder<'a>(size: Size, label: &'a str, label_size: f32, radius: f32) -> Element<'a, Message> {
    container(text(label).size(label_size))
        .width(size.width)
        .height(size.height)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |t: &Theme| container::Style {
            background: Some(t.extended_palette().background.weak.color.into()),
            border: iced::border::rounded(radius),
            ..Default::default()
        })
        .into()
}

/// Thumbnail, blurred mini-thumbnail or empty box, plus ▶ and duration.
fn video_cell<'a>(app: &'a App, media: &'a Media, size: Size, radius: f32) -> Element<'a, Message> {
    let Media::Video { thumb, mini, duration, listened, .. } = media else {
        return placeholder(size, "", 14.0, radius);
    };
    let still = thumb.and_then(|file| app.media.image(file)).or(mini.as_ref());
    let picture: Element<_> = match still {
        Some(handle) => image(handle.clone())
            .width(size.width)
            .height(size.height)
            .content_fit(ContentFit::Cover)
            .border_radius(radius)
            .into(),
        None => placeholder(size, "", 14.0, radius),
    };
    let dot = if *listened { "" } else { "  ●" };
    let label = container(text(format!("▶ {}{dot}", fmt_duration(*duration))).size(12))
        .padding([2, 8])
        .style(|_: &Theme| container::Style {
            background: Some(Color::from_rgba8(0, 0, 0, 0.6).into()),
            text_color: Some(Color::WHITE),
            border: iced::border::rounded(10),
            ..Default::default()
        });
    stack![
        picture,
        container(label)
            .width(size.width)
            .height(size.height)
            .padding(8)
            .align_x(if radius > 20.0 { Alignment::Center } else { Alignment::Start })
            .align_y(Alignment::End),
    ]
    .into()
}

/// Bars of the waveform, split into the played and the remaining part.
fn waveform_bars(waveform: &[u8], progress: f64) -> (String, String) {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let bars: String = (0..VOICE_BARS)
        .map(|i| {
            let v = if waveform.is_empty() { 0 } else { waveform[i * waveform.len() / VOICE_BARS] };
            BARS[(usize::from(v) * 7 / 31).min(7)]
        })
        .collect();
    let split = ((progress.clamp(0.0, 1.0) * VOICE_BARS as f64).round() as usize).min(VOICE_BARS);
    let at = bars.char_indices().nth(split).map_or(bars.len(), |(i, _)| i);
    (bars[..at].to_owned(), bars[at..].to_owned())
}

/// Blue dot of a voice or round video message nobody has played yet.
fn unlistened_dot<'a>() -> Element<'a, Message> {
    text("●").size(9).style(primary_text).into()
}

fn voice_view<'a>(app: &'a App, msg_id: i64, duration: i32, waveform: &'a [u8], listened: bool) -> Element<'a, Message> {
    let playback = app.voice.as_ref().filter(|v| v.msg_id == msg_id);
    let (icon, progress, time) = match playback.map(|v| v.audio.as_ref()) {
        Some(Some(audio)) => {
            let total = audio.duration().map_or(f64::from(duration), |d| d.as_secs_f64()).max(0.001);
            let pos = audio.position().as_secs_f64();
            let icon = if audio.paused() { "▶" } else { "⏸" };
            (icon, pos / total, format!("{} / {}", fmt_secs(pos), fmt_secs(total)))
        }
        Some(None) => ("…", 0.0, "loading…".to_owned()),
        None => ("▶", 0.0, fmt_duration(duration)),
    };
    let (played, rest) = waveform_bars(waveform, progress);
    let mut line = row![
        text(icon).size(16).style(primary_text),
        row![text(played).size(14).style(primary_text), text(rest).size(14).style(muted_text)],
        text(time).size(12).style(muted_text),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if !listened {
        line = line.push(unlistened_dot());
    }
    if playback.is_some() {
        line = line.push(text(player::fmt_speed(app.speed())).size(12).style(primary_text));
    }
    container(line).height(VOICE.height).align_y(Alignment::Center).into()
}

fn media_cell<'a>(app: &'a App, msg_id: i64, item: usize, media: &'a Media, size: Size, album: bool) -> Element<'a, Message> {
    let radius = match media {
        Media::Sticker { .. } => 0.0,
        Media::Video { round: true, .. } => size.width / 2.0,
        _ => 8.0,
    };
    let cell: Element<_> = match media {
        Media::Voice { duration, waveform, listened, .. } => voice_view(app, msg_id, *duration, waveform, *listened),
        Media::Video { .. } => video_cell(app, media, size, radius),
        _ => match media.inline_file().and_then(|file| app.media.get(file)) {
            Some(Loaded::Image(handle)) => image(handle.clone())
                .width(size.width)
                .height(size.height)
                .content_fit(if album { ContentFit::Cover } else { ContentFit::Contain })
                .border_radius(radius)
                .into(),
            Some(Loaded::Frames(frames)) => image(frames.at(app.now.saturating_duration_since(app.clock)).clone())
                .width(size.width)
                .height(size.height)
                .into(),
            Some(Loaded::Video(video)) => container(
                VideoPlayer::new(video)
                    .width(size.width)
                    .height(size.height)
                    .content_fit(ContentFit::Cover),
            )
            .width(size.width)
            .height(size.height)
            .clip(true)
            .into(),
            None => match media {
                Media::Sticker { emoji, .. } => placeholder(size, emoji, 48.0, radius),
                Media::Animation { .. } => placeholder(size, "GIF", 14.0, radius),
                _ => placeholder(size, "", 14.0, radius),
            },
        },
    };
    mouse_area(cell)
        .on_press(Message::ClickMedia { msg_id, item })
        .interaction(Interaction::Pointer)
        .into()
}

fn media_block<'a>(app: &'a App, m: &'a Msg, width: f32) -> Element<'a, Message> {
    let sizes = media_sizes(&m.media, width);
    if m.media.len() == 1 {
        return media_cell(app, m.id, 0, &m.media[0], sizes[0], false);
    }
    let (cols, _) = album_layout(m.media.len(), width);
    let cells: Vec<Element<_>> = m
        .media
        .iter()
        .zip(&sizes)
        .enumerate()
        .map(|(i, (media, size))| media_cell(app, m.id, i, media, *size, true))
        .collect();
    let mut rows = Vec::new();
    let mut cells = cells.into_iter();
    loop {
        let chunk: Vec<_> = cells.by_ref().take(cols).collect();
        if chunk.is_empty() {
            break;
        }
        rows.push(Row::with_children(chunk).spacing(ALBUM_GAP).into());
    }
    Column::with_children(rows).spacing(ALBUM_GAP).into()
}

fn reply_quote<'a>(app: &'a App, reply_to: i64) -> Element<'a, Message> {
    let quoted = app
        .open
        .as_ref()
        .and_then(|o| o.position(reply_to).map(|i| &o.msgs[i]));
    let label = match quoted {
        Some(q) => {
            let summary: String = q.summary().lines().next().unwrap_or("").chars().take(80).collect();
            format!("↪ {}: {summary}", app.sender_name(q.sender))
        }
        None => "↪ reply".to_owned(),
    };
    container(text(label).size(12).wrapping(Wrapping::None).style(muted_text))
        .height(REPLY_H - 4.0)
        .padding([0, 8])
        .align_y(Alignment::Center)
        .clip(true)
        .style(|t: &Theme| container::Style {
            border: iced::Border {
                color: t.palette().primary,
                width: 0.0,
                radius: 3.into(),
            },
            background: Some(t.extended_palette().background.weak.color.into()),
            ..Default::default()
        })
        .into()
}

fn message_view<'a>(app: &'a App, m: &'a Msg, selected: bool, width: f32, show_avatar: bool) -> Element<'a, Message> {
    let sender = m.sender;
    let mut header = row![
        text(app.sender_name(sender))
            .size(13)
            .style(move |t: &Theme| text::Style { color: Some(sender_color(t, sender)) }),
        text(fmt_time(m.date, false)).size(12).style(muted_text),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if m.edited {
        header = header.push(text("edited").size(12).style(muted_text));
    }
    if let Some(mark) = mark_text(delivery_mark(app, m)) {
        header = header.push(mark);
    }
    if m.deleted {
        header = header.push(
            text("🗑 deleted")
                .size(12)
                .style(|t: &Theme| text::Style { color: Some(t.palette().danger) }),
        );
    }

    let mut body = column![header].spacing(2);
    if let Some(reply_to) = m.reply_to {
        body = body.push(reply_quote(app, reply_to));
    }
    if !m.media.is_empty() {
        body = body.push(container(media_block(app, m, width)).padding([MEDIA_GAP / 2.0, 0.0]));
    }
    if !m.text.is_empty() {
        let deleted = m.deleted;
        body = body.push(text(&m.text).size(TEXT).style(move |t: &Theme| {
            if deleted { muted_text(t) } else { text::Style::default() }
        }));
    }

    let bubble = container(body)
        .padding([6, 10])
        .width(Length::Fill)
        .style(move |t: &Theme| message_style(t, selected));
    let channel = app.chats.get(&m.chat_id).is_some_and(|c| c.kind == ChatKind::Channel);
    let face: Element<_> = if show_avatar && !channel {
        let name = app.sender_name(sender);
        let id = match sender {
            Sender::User(id) | Sender::Chat(id) => id,
        };
        container(avatar_image(app, app.sender_avatar(sender), id, name, MSG_AVATAR))
            .padding(iced::Padding { top: 4.0, ..Default::default() })
            .into()
    } else {
        space::horizontal().width(MSG_AVATAR).into()
    };
    let line = row![face, bubble].spacing(MSG_AVATAR_GAP);
    let tip = container(text(fmt_full_date(m.date)).size(12))
        .padding([4, 8])
        .style(container::bordered_box);
    tooltip(
        mouse_area(line).on_press(Message::SelectMsg(m.id)),
        tip,
        tooltip::Position::FollowCursor,
    )
    .into()
}

/// Full-screen photo or video, shown instead of the panes.
fn media_viewer(app: &App) -> Option<Element<'_, Message>> {
    let content: Element<_> = match app.viewer_media()? {
        Media::Photo { preview, full } => {
            match app.media.image(full.id).or_else(|| app.media.image(preview.id)) {
                Some(handle) => image(handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                None => center(text("Loading…").size(14)).into(),
            }
        }
        Media::Video { .. } => match app.video.as_ref().and_then(VideoPlayback::video) {
            Some(video) => VideoPlayer::new(video)
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Contain)
                .into(),
            None => center(text("Loading video…").size(14).style(|_: &Theme| text::Style {
                color: Some(Color::WHITE),
            }))
            .into(),
        },
        _ => return None,
    };
    Some(
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(12)
            .style(|_: &Theme| container::Style {
                background: Some(Color::from_rgb8(0x0b, 0x0b, 0x0f).into()),
                ..Default::default()
            })
            .into(),
    )
}

/// Line editor contents with a cursor bar when it is being edited.
fn input_text<'a>(input: &Input, editing: bool, mask: bool, prefix: &str) -> Element<'a, Message> {
    let (before, after) = input.split();
    let show = |s: &str| if mask { "•".repeat(s.chars().count()) } else { s.to_owned() };
    let content = if editing {
        // A left-aligned bar hugs the previous character instead of floating
        // in the middle of a cell like `│`, which looked like a space.
        format!("{prefix}{}▏{}", show(before), show(after))
    } else {
        format!("{prefix}{}", show(input.as_str()))
    };
    text(content).size(TEXT).into()
}

/// "📎 photo.png" or "📎 3 files: a.png, b.mp4, …"
fn attachments_label(paths: &[std::path::PathBuf]) -> String {
    let names: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()))
        .collect();
    match names.len() {
        1 => format!("📎 {}", names[0]),
        n => format!("📎 {n} files: {}", names.join(", ")),
    }
}

fn compose_banner(app: &App) -> Option<Element<'_, Message>> {
    let attached = (!app.attachments.is_empty()).then(|| attachments_label(&app.attachments));
    let label = match app.compose {
        Compose::New => attached.clone()?,
        Compose::Reply(id) => {
            let quoted = app.open.as_ref().and_then(|o| o.position(id).map(|i| &o.msgs[i]));
            match quoted {
                Some(q) => {
                    let summary: String = q.summary().lines().next().unwrap_or("").chars().take(80).collect();
                    format!("↩ Reply to {}: {summary}", app.sender_name(q.sender))
                }
                None => "↩ Reply".to_owned(),
            }
        }
        Compose::Edit(_) => "✎ Editing message".to_owned(),
    };
    let label = match (app.compose, attached) {
        (Compose::Reply(_), Some(attached)) => format!("{attached}  ·  {label}"),
        _ => label,
    };
    Some(
        row![
            text(label).size(12).wrapping(Wrapping::None).style(primary_text).width(Length::Fill),
            text("Esc — cancel").size(12).style(muted_text),
        ]
        .height(BANNER_H)
        .align_y(Alignment::Center)
        .into(),
    )
}

pub fn has_banner(app: &App) -> bool {
    app.compose != Compose::New || !app.attachments.is_empty()
}

fn compose(app: &App) -> Element<'_, Message> {
    let editing = app.mode == Mode::Insert;
    let content = if app.input.is_empty() && !editing {
        text("i — write a message   q — record voice   ga — attach file   ge — emoji")
            .size(TEXT)
            .style(muted_text)
            .into()
    } else {
        input_text(&app.input, editing, false, "")
    };
    let mut body = column![].width(Length::Fill);
    if let Some(banner) = compose_banner(app) {
        body = body.push(banner);
    }
    body = body.push(content);
    let open = app.emoji.is_some();
    let button = mouse_area(
        container(text("😀").size(18).line_height(1.0))
            .width(32)
            .height(32)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |t: &Theme| selection_style(t, open, true)),
    )
    .on_press(Message::ToggleEmoji)
    .interaction(Interaction::Pointer);
    container(row![body, button].spacing(8).align_y(Alignment::End))
        .padding([10.0, MSG_PAD_X])
        .width(Length::Fill)
        .max_height(compose_height(&app.input, has_banner(app)))
        .clip(true)
        .into()
}

fn auth_screen(app: &App) -> Element<'_, Message> {
    let (title, hint, mask) = match &app.auth {
        Auth::Starting => ("Connecting…", String::new(), false),
        Auth::Phone => ("Phone number", "International format, e.g. +79991234567".into(), false),
        Auth::Code => ("Login code", "Sent to your Telegram app or via SMS".into(), false),
        Auth::Password { hint } => (
            "Two-step verification password",
            if hint.is_empty() { String::new() } else { format!("Hint: {hint}") },
            true,
        ),
        Auth::Info(info) => (info.as_str(), String::new(), false),
        Auth::Ready => unreachable!("auth screen is only shown before login"),
    };
    let asks_input = matches!(app.auth, Auth::Phone | Auth::Code | Auth::Password { .. });
    let editing = app.mode == Mode::Insert;

    let mut form = column![
        text("vimgram").size(36),
        space::vertical().height(12),
        text(title).size(18),
        text(hint).size(13).style(muted_text),
    ]
    .spacing(6)
    .width(380);

    if asks_input {
        form = form.push(
            container(input_text(&app.input, editing, mask, ""))
                .padding([8, 12])
                .width(Length::Fill)
                .style(move |t: &Theme| {
                    let p = t.extended_palette();
                    let mut style = container::bordered_box(t);
                    if editing {
                        style.border.color = p.primary.base.color;
                    }
                    style
                }),
        );
        form = form.push(
            text("i — type   Enter — submit   Esc — normal mode   :q — quit")
                .size(12)
                .style(muted_text),
        );
    }

    center(form).into()
}

/// Timer and a live level meter drawn with block characters.
fn recording_status(app: &App) -> Element<'_, Message> {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let Some(recorder) = &app.recorder else {
        return space::horizontal().into();
    };
    let secs = recorder.elapsed().as_secs();
    let levels = recorder.levels();
    let meter: String = levels[levels.len().saturating_sub(48)..]
        .iter()
        .map(|&l| BARS[((l * 7.0).round() as usize).min(7)])
        .collect();
    row![
        text(format!("● {}:{:02}", secs / 60, secs % 60))
            .size(13)
            .style(|t: &Theme| text::Style { color: Some(t.palette().danger) }),
        text(meter).size(13),
        text("q/Enter — send   Esc — discard").size(13).style(muted_text),
    ]
    .spacing(12)
    .into()
}

/// What is playing, for the right side of the status line.
fn playback_status(app: &App) -> Option<String> {
    let speed = player::fmt_speed(app.speed());
    if let Some(video) = app.video.as_ref().and_then(VideoPlayback::video) {
        let icon = if video.paused() { "⏸" } else { "▶" };
        return Some(format!(
            "{icon} {} / {}  {speed}",
            fmt_secs(video.position().as_secs_f64()),
            fmt_secs(video.duration().as_secs_f64())
        ));
    }
    let audio = app.voice.as_ref()?.audio.as_ref()?;
    let icon = if audio.paused() { "⏸" } else { "🎤" };
    let total = audio.duration().map_or(0.0, |d| d.as_secs_f64());
    Some(format!(
        "{icon} {} / {}  {speed}",
        fmt_secs(audio.position().as_secs_f64()),
        fmt_secs(total)
    ))
}

fn status_line(app: &App) -> Element<'_, Message> {
    let mode = app.mode;
    let badge = container(text(mode.label()).size(12))
        .padding([3, 10])
        .style(move |t: &Theme| {
            let p = t.extended_palette();
            let pair = match mode {
                Mode::Normal => p.primary.strong,
                Mode::Insert => p.success.base,
                Mode::Command | Mode::Search => p.warning.base,
                Mode::Record => p.danger.base,
            };
            container::Style {
                background: Some(pair.color.into()),
                text_color: Some(pair.text),
                ..Default::default()
            }
        });

    let viewer_hint = match app.viewer_media() {
        Some(Media::Video { .. }) => {
            Some("Space pause   h/l ±5s   H/L ±30s   [ ] speed   = 1x   j/k next/prev   Esc close")
        }
        Some(_) => Some("j/k — next/previous   Esc — close"),
        None => None,
    };

    let middle: Element<_> = match mode {
        _ if app.confirm.is_some() => {
            let Some(Confirm::Delete { ids, .. }) = &app.confirm else { unreachable!() };
            let what = if ids.len() > 1 { "these messages" } else { "this message" };
            text(format!("Delete {what}?   y — for everyone   m — only for me   n/Esc — cancel"))
                .size(13)
                .style(|t: &Theme| text::Style { color: Some(t.palette().danger) })
                .into()
        }
        Mode::Command => input_text(&app.cmdline, true, false, ":"),
        Mode::Search => input_text(&app.cmdline, true, false, "/"),
        Mode::Record => recording_status(app),
        _ => match (&app.status, viewer_hint) {
            (Some(status), _) => {
                let error = status.error;
                text(&status.text)
                    .size(13)
                    .wrapping(Wrapping::None)
                    .style(move |t: &Theme| text::Style {
                        color: error.then(|| t.palette().danger),
                    })
                    .into()
            }
            (None, Some(hint)) => text(hint).size(13).style(muted_text).into(),
            (None, None) => space::horizontal().into(),
        },
    };

    let position = match (&app.open, app.focus) {
        (Some(open), Focus::Messages) if !open.msgs.is_empty() => {
            format!("{}/{}", open.cursor + 1, open.msgs.len())
        }
        _ => app
            .selected_index()
            .map(|i| format!("{}/{}", i + 1, app.list.len()))
            .unwrap_or_default(),
    };

    let mut line = row![badge, container(middle).width(Length::Fill).clip(true)]
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some(playback) = playback_status(app) {
        line = line.push(text(playback).size(13).style(primary_text));
    }
    line = line
        .push(text(app.pending.display()).size(13))
        .push(text(position).size(13));

    container(line)
        .height(STATUS_H)
        .padding(iced::Padding { right: 12.0, ..Default::default() })
        .align_y(Alignment::Center)
        .style(|t: &Theme| {
            let p = t.extended_palette().background.weak;
            container::Style {
                background: Some(p.color.into()),
                text_color: Some(p.text),
                ..Default::default()
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_at(id: i64, date: i32) -> Msg {
        Msg {
            id,
            chat_id: 1,
            sender: Sender::User(1),
            outgoing: false,
            date,
            edited: false,
            text: "hi".into(),
            media: Vec::new(),
            album: 0,
            members: vec![id],
            reply_to: None,
            sending: false,
            scheduled: false,
            deleted: false,
            local_files: Vec::new(),
        }
    }

    #[test]
    fn days_are_separated() {
        let noon = Local::now().date_naive().and_hms_opt(12, 0, 0).unwrap();
        let unix = |dt: chrono::NaiveDateTime| dt.and_local_timezone(Local).unwrap().timestamp() as i32;
        let today = unix(noon);
        let yesterday = unix(noon - chrono::Duration::days(1));
        let msgs = [msg_at(1, yesterday), msg_at(2, yesterday + 60), msg_at(3, today), msg_at(4, today + 60)];
        let starts: Vec<bool> = (0..msgs.len()).map(|i| starts_day(&msgs, i)).collect();
        assert_eq!(starts, [true, false, true, false]);
        assert_eq!(fmt_day(today), "Today");
        assert_eq!(fmt_day(yesterday), "Yesterday");
        let long_ago = noon - chrono::Duration::days(800);
        assert!(fmt_day(unix(long_ago)).ends_with(&long_ago.format("%Y").to_string()), "older years show the year");
        // The separator is part of the height used for layout.
        let width = 500.0;
        assert_eq!(item_height(&msgs, 2, width), message_height(&msgs[2], width) + DAY_H);
        assert_eq!(item_height(&msgs, 3, width), message_height(&msgs[3], width));
        // A new day also shows the sender's avatar again.
        assert!(starts_run(&msgs, 2));
        assert!(!starts_run(&msgs, 3));
    }

    #[test]
    fn waveform_split() {
        let (played, rest) = waveform_bars(&[31; 10], 0.5);
        assert_eq!(played.chars().count(), VOICE_BARS / 2);
        assert_eq!(rest.chars().count(), VOICE_BARS / 2);
        let (played, rest) = waveform_bars(&[], 1.0);
        assert_eq!((played.chars().count(), rest.len()), (VOICE_BARS, 0));
    }

    #[test]
    fn album_grid() {
        assert_eq!(album_layout(2, 1000.0).0, 2);
        assert_eq!(album_layout(4, 1000.0).0, 2);
        assert_eq!(album_layout(5, 1000.0).0, 3);
    }
}
