//! Rendering. Lists are virtualized: only rows that fit the window are built,
//! so thousands of chats or messages cost nothing extra per frame.

use chrono::{DateTime, Local};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, canvas, center, column, container, image, mouse_area, progress_bar, rich_text, row, rule, scrollable,
    slider, space, span, stack, text, tooltip,
};
use iced::mouse::Interaction;
use iced::{Alignment, Color, ContentFit, Element, Font, Length, Size, Theme, font};
use iced_video_player::VideoPlayer;

use crate::app::{App, Auth, Busy, Chat, Compose, Confirm, Focus, Message, Pane, COMPOSE_ID, SPOILER_LINK, TALL_ID, VideoPlayback, VoicePlayback};
use crate::config;
use crate::emoji::{self, Picker};
use crate::fonts::{self, EMOJI};
use crate::media::Loaded;
use crate::player::{self, SpeedKind};
use crate::tg::{ChatKind, Media, Membership, Msg, Presence, Preview, Sender, Style, fmt_count, fmt_duration};
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
/// The ↓ button that brings a scrolled-up chat back to its newest messages.
const TO_LATEST: f32 = 40.0;
/// Space between the newest message and the line above the compose field.
const LIST_BOTTOM: f32 = 12.0;
pub const MAX_COMPOSE_LINES: usize = 8;
/// The 😀 button right of the compose line, and the space before it.
const COMPOSE_BUTTON: f32 = 32.0;
const COMPOSE_GAP: f32 = 8.0;
const BANNER_H: f32 = 36.0;
const REPLY_H: f32 = 22.0;
const PHOTO_MAX: Size = Size::new(420.0, 380.0);
const VIDEO_MAX: Size = Size::new(420.0, 340.0);
const ROUND_VIDEO: f32 = 220.0;
const GIF_MAX: Size = Size::new(360.0, 300.0);
const STICKER_MAX: Size = Size::new(170.0, 170.0);
const VOICE: Size = Size::new(380.0, 30.0);
/// A music file: name line, progress, time line.
const AUDIO: Size = Size::new(380.0, 48.0);
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
/// "Forwarded from …" line above a forwarded message.
const FORWARD_H: f32 = 18.0;
/// Reactions under a message: one chip, a line of chips with the space
/// between lines, and a pessimistic chip width.
const REACTION_CHIP_H: f32 = 24.0;
const REACTIONS_H: f32 = REACTION_CHIP_H + 4.0;
const REACTION_W: f32 = 68.0;
/// Line between posts or between messages far apart in time.
const BLOCK_H: f32 = 13.0;
/// "💬 12 comments" under a channel post.
const COMMENTS_H: f32 = 22.0;
const BLOCK_GAP: i32 = 3600;

pub fn chat_rows(size: Size) -> usize {
    (((size.height - STATUS_H - HEADER_H) / CHAT_ROW_H).floor() as usize).max(1)
}

/// Photos and videos beside the chat: half of the room right of the
/// chat list.
pub fn side_width(size: Size) -> f32 {
    ((size.width - FOLDER_W - SIDEBAR_W) / 2.0).floor().max(0.0)
}

/// Width of the chat pane: what the chat list and the viewer beside it
/// (`side`) leave.
fn pane_width(size: Size, side: bool) -> f32 {
    let side = if side { side_width(size) + 1.0 } else { 0.0 };
    size.width - FOLDER_W - SIDEBAR_W - 1.0 - side
}

/// Width of the compose text, between the pane's padding and the 😀 button.
pub fn compose_text_width(size: Size, emoji: bool, side: bool) -> f32 {
    let panel = if emoji { EMOJI_W } else { 0.0 };
    pane_width(size, side) - 2.0 * MSG_PAD_X - COMPOSE_BUTTON - COMPOSE_GAP - panel
}

/// Lines `text` takes wrapped at `width`. An estimate from an average
/// letter width: a wrong guess only scrolls the compose box a little.
pub fn compose_lines(text: &str, width: f32) -> usize {
    let per_line = (width / (TEXT * 0.55)).max(10.0) as usize;
    text.split('\n').map(|l| display_width(l).max(1).div_ceil(per_line)).sum()
}

fn compose_height(input: &Input, width: f32, banner: bool) -> f32 {
    let lines = compose_lines(input.as_str(), width).clamp(1, MAX_COMPOSE_LINES);
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
    side: bool,
) -> (f32, f32) {
    let panel = if emoji { EMOJI_W } else { 0.0 };
    let width = pane_width(size, side) - 2.0 * MSG_PAD_X - MSG_AVATAR - MSG_AVATAR_GAP - panel;
    let pinned = if pinned { PINNED_H + 1.0 } else { 0.0 };
    let player = if player { PLAYER_H + 1.0 } else { 0.0 };
    let compose = compose_height(compose, compose_text_width(size, emoji, side), banner);
    let height = size.height - STATUS_H - HEADER_H - 2.0 - pinned - player - compose - LIST_BOTTOM;
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
        Media::Audio { .. } => return Size::new(AUDIO.width.min(width - 20.0), AUDIO.height),
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

/// An album of music files: one track under another, not a grid.
fn tracklist(media: &[Media]) -> bool {
    media.len() > 1 && media.iter().all(|m| matches!(m, Media::Audio { .. }))
}

/// Size of every media item of a message, in order.
pub fn media_sizes(media: &[Media], width: f32) -> Vec<Size> {
    if media.len() <= 1 || tracklist(media) {
        media.iter().map(|m| media_size(m, width)).collect()
    } else {
        vec![album_layout(media.len(), width).1; media.len()]
    }
}

fn media_block_height(media: &[Media], width: f32) -> f32 {
    match media.len() {
        0 => 0.0,
        1 => media_size(&media[0], width).height + MEDIA_GAP,
        n if tracklist(media) => n as f32 * AUDIO.height + (n - 1) as f32 * ALBUM_GAP + MEDIA_GAP,
        n => {
            let (cols, cell) = album_layout(n, width);
            let rows = n.div_ceil(cols) as f32;
            rows * cell.height + (rows - 1.0) * ALBUM_GAP + MEDIA_GAP
        }
    }
}

/// Link preview card: text size, its line height, the picture beside the
/// text, padding, and the description's line limit.
const PREVIEW_TEXT: f32 = 13.0;
const PREVIEW_LINE_H: f32 = PREVIEW_TEXT * 1.35;
const PREVIEW_THUMB: f32 = 56.0;
const PREVIEW_PAD: f32 = 8.0;
const PREVIEW_LINES: usize = 3;

/// Width of a preview card's text in a message `width` wide.
fn preview_text_width(preview: &Preview, width: f32) -> f32 {
    let thumb = if preview.photo.is_some() { PREVIEW_THUMB + PREVIEW_PAD } else { 0.0 };
    width - 20.0 - 2.0 * PREVIEW_PAD - thumb
}

/// The description cut to what the card shows.
fn preview_description(preview: &Preview, width: f32) -> String {
    let per_line = (preview_text_width(preview, width) / (PREVIEW_TEXT * 0.6)).max(10.0) as usize;
    let mut out = String::new();
    let mut lines = 0;
    for line in preview.description.lines().filter(|l| !l.trim().is_empty()) {
        let room = (PREVIEW_LINES - lines) * per_line;
        lines += display_width(line).max(1).div_ceil(per_line);
        if !out.is_empty() {
            out.push('\n');
        }
        if lines > PREVIEW_LINES {
            out.extend(line.chars().take(room.saturating_sub(1)));
            out.push('…');
            break;
        }
        out.push_str(line);
        if lines == PREVIEW_LINES {
            break;
        }
    }
    out
}

fn preview_height(preview: &Preview, width: f32) -> f32 {
    let per_line = (preview_text_width(preview, width) / (PREVIEW_TEXT * 0.6)).max(10.0) as usize;
    let description: usize = preview_description(preview, width)
        .lines()
        .map(|l| display_width(l).max(1).div_ceil(per_line))
        .sum();
    let lines = usize::from(!preview.site.is_empty()) + usize::from(!preview.title.is_empty()) + description;
    let text = lines as f32 * PREVIEW_LINE_H;
    let thumb = if preview.photo.is_some() { PREVIEW_THUMB } else { 0.0 };
    text.max(thumb) + 2.0 * PREVIEW_PAD + MEDIA_GAP
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
    let reply = reply + if m.forward.is_some() { FORWARD_H } else { 0.0 };
    let per_row = ((width - 20.0) / REACTION_W).max(1.0) as usize;
    // Rows of chips, the padding above them and the column spacing.
    let rows = m.reactions.len().div_ceil(per_row);
    let reactions = if rows == 0 { 0.0 } else { rows as f32 * REACTIONS_H + 2.0 };
    let preview = m.preview.as_ref().map_or(0.0, |p| preview_height(p, width));
    let comments = if m.comments.is_some() { COMMENTS_H } else { 0.0 };
    16.0 + 18.0 + reply + media_block_height(&m.media, width) + lines as f32 * LINE_H + preview + reactions + comments
        + MSG_SPACING
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

/// The message is separated from the previous one by a line: every channel
/// post, and messages after an hour of silence. A new day has its date instead.
pub fn starts_block(msgs: &[Msg], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).and_then(|p| msgs.get(p)) else {
        return false;
    };
    let m = &msgs[i];
    !starts_day(msgs, i) && (m.sender == Sender::Chat(m.chat_id) || m.date - prev.date > BLOCK_GAP)
}

/// Height of message `i` in the list, with its date separator or line.
pub fn item_height(msgs: &[Msg], i: usize, width: f32) -> f32 {
    let separator = if starts_day(msgs, i) {
        DAY_H
    } else if starts_block(msgs, i) {
        BLOCK_H
    } else {
        0.0
    };
    message_height(&msgs[i], width) + separator
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

fn block_separator<'a>() -> Element<'a, Message> {
    container(rule::horizontal(1).style(|t: &Theme| rule::Style {
        color: t.extended_palette().background.strong.color,
        ..rule::default(t)
    }))
    .width(Length::Fill)
    .height(BLOCK_H - MSG_SPACING)
    .padding([0.0, 24.0])
    .align_y(Alignment::Center)
    .into()
}

/// A message of the list, below its date separator when it starts a day
/// (or a line when it starts a block).
fn list_item<'a>(app: &'a App, msgs: &'a [Msg], i: usize, selected: bool, width: f32) -> Element<'a, Message> {
    let message = message_view(app, &msgs[i], selected, width, starts_run(msgs, i));
    if starts_day(msgs, i) {
        column![day_separator(msgs[i].date), message].spacing(MSG_SPACING).into()
    } else if starts_block(msgs, i) {
        column![block_separator(), message].spacing(MSG_SPACING).into()
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
    } else if let Some(viewer) = media_viewer(app).filter(|_| !app.viewer_side) {
        viewer
    } else {
        let main = row![
            // The folder strip's own background separates it from the list.
            folder_strip(app),
            chat_list(app),
            rule::vertical(1),
            chat_pane(app)
        ]
        .height(Length::Fill);
        match media_viewer(app) {
            Some(viewer) => main.push(rule::vertical(1)).push(side_viewer(app, viewer)).into(),
            None => main.into(),
        }
    };
    column![body, status_line(app)].into()
}

/// Secondary text: the text color, faded. Readable on the background and
/// on highlighted rows alike.
/// User text (messages, titles, previews) with its emoji in the color
/// emoji font, see `fonts::EMOJI`.
fn emoji_text<'a>(s: &str) -> text::Rich<'a, (), Message> {
    rich_text(emoji_spans(s).collect::<Vec<text::Span<'a, ()>>>())
}

fn emoji_spans<'a, L>(s: &str) -> impl Iterator<Item = text::Span<'a, L>> {
    fonts::emoji_runs(s)
        .into_iter()
        .map(|(run, emoji)| span(run.to_owned()).font_maybe(emoji.then_some(EMOJI)))
}

/// A message's text with its formatting: links underlined in the theme's
/// color (clicking one opens it), bold, italic, code, and spoilers as solid
/// blocks until shown.
fn message_text<'a>(app: &App, m: &Msg) -> Element<'a, Message> {
    let palette = app.theme.palette();
    let hidden = app.hides(m);
    // Every place a link or a style starts or ends cuts the text into parts
    // shown alike.
    let mut cuts = vec![0, m.text.len()];
    cuts.extend(m.links.iter().flat_map(|l| [l.start, l.end]));
    cuts.extend(m.styles.iter().flat_map(|s| [s.start, s.end]));
    cuts.retain(|&c| m.text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut spans: Vec<text::Span<'a, String>> = Vec::new();
    for part in cuts.windows(2) {
        let (start, end) = (part[0], part[1]);
        let shown = &m.text[start..end];
        let link = m.links.iter().find(|l| l.start <= start && end <= l.end);
        let has = |style| m.styles.iter().any(|s| s.style == style && s.start <= start && end <= s.end);
        if hidden && has(Style::Spoiler) {
            // Emoji keep their colors whatever the text color: blocks instead.
            let blank: String = fonts::emoji_runs(shown)
                .into_iter()
                .map(|(run, emoji)| if emoji { "█".repeat(run.chars().count()) } else { run.to_owned() })
                .collect();
            spans.push(
                span(blank)
                    .color(palette.text)
                    .background(palette.text)
                    .link(format!("{SPOILER_LINK}{}", m.id)),
            );
            continue;
        }
        let font = Font {
            weight: if has(Style::Bold) { font::Weight::Bold } else { font::Weight::Normal },
            style: if has(Style::Italic) { font::Style::Italic } else { font::Style::Normal },
            ..if has(Style::Code) { Font::MONOSPACE } else { Font::DEFAULT }
        };
        let code = has(Style::Code).then(|| palette.text.scale_alpha(0.12));
        for piece in emoji_spans(shown) {
            let mut piece = match piece.font {
                Some(_) => piece,
                None => piece.font(font),
            };
            piece = piece
                .underline(link.is_some() || has(Style::Underline))
                .strikethrough(has(Style::Strike))
                .background_maybe(code);
            if let Some(link) = link {
                piece = piece.link(link.url.clone()).color(palette.primary);
            }
            spans.push(piece);
        }
    }
    let deleted = m.deleted;
    rich_text(spans)
        .size(TEXT)
        .style(move |t: &Theme| if deleted { muted_text(t) } else { text::Style::default() })
        .on_link_click(Message::OpenLink)
        .into()
}

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
        local.format(config::hm()).to_string()
    } else if short {
        local.format("%d.%m.%y").to_string()
    } else {
        local.format(&format!("%d.%m.%y {}", config::hm())).to_string()
    }
}

pub fn fmt_datetime(unix: i32) -> String {
    DateTime::from_timestamp(i64::from(unix), 0)
        .map(|d| d.with_timezone(&Local).format(&format!("%d.%m.%Y {}", config::hm())).to_string())
        .unwrap_or_default()
}

/// Full date for the hover tooltip: "Thursday, 16 October 2026, 19:42:07".
fn fmt_full_date(unix: i32) -> String {
    DateTime::from_timestamp(i64::from(unix), 0)
        .map(|d| d.with_timezone(&Local).format(&format!("%A, %-d %B %Y, {}", config::hms())).to_string())
        .unwrap_or_default()
}

/// "typing…" in a private chat, "Alice is typing…" or "Alice and 2 more
/// are typing…" in a group.
fn fmt_activity(app: &App, chat_id: i64) -> Option<String> {
    let acting = app.actions.get(&chat_id)?;
    let &(sender, label) = acting.first()?;
    if matches!(app.chats.get(&chat_id)?.kind, ChatKind::Private(_)) {
        return Some(format!("{label}…"));
    }
    let name = app.sender_name(sender);
    Some(match acting.len() {
        1 => format!("{name} is {label}…"),
        n => format!("{name} and {} more are {label}…", n - 1),
    })
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
                format!("last seen today at {}", local.format(config::hm()))
            } else if today.pred_opt() == Some(local.date_naive()) {
                format!("last seen yesterday at {}", local.format(config::hm()))
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
    let focused = app.focus == Focus::Folders;
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
                .style(move |t: &Theme| selection_style(t, selected, focused)),
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
        "Forward to… (Enter, i — comment, Esc)".to_owned()
    } else if app.filter.is_empty() {
        format!("{folder}  {}", fmt_count(app.list.len()))
    } else {
        let mut title = format!("/{}  {} chats", app.filter, fmt_count(app.list.len()));
        let hits = app.global.hits();
        if hits > 0 {
            title += &format!(" · {} messages", fmt_count(hits));
        }
        if app.global.loading {
            title += " · searching…";
        }
        title
    };
    let forwarding = app.forward.is_some();
    let header = container(text(title).size(13).style(move |t: &Theme| {
        if forwarding { primary_text(t) } else { muted_text(t) }
    }))
    .height(HEADER_H)
    .padding([0, 12])
    .align_y(Alignment::Center);

    let chats = app
        .list
        .iter()
        .filter_map(|id| app.chats.get(id))
        .map(|chat| chat_row(app, chat, app.hit.is_none() && app.selected == Some(chat.id), focused));
    // Messages found by the filter, below the chats.
    let hits = (0..app.global.hits())
        .filter_map(|i| Some((i, app.global.hit(i)?)))
        .map(|(i, m)| hit_row(app, m, i, app.hit == Some(i), focused));
    let rows = chats.chain(hits).skip(app.chat_top).take(chat_rows(app.size));

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

/// A message on the two preview lines of a row.
fn preview_of(m: &Msg, sender: Option<&str>) -> String {
    // Newlines would waste the two preview lines.
    let summary: String = m.summary().split_whitespace().collect::<Vec<_>>().join(" ");
    let line: String = summary.chars().take(200).collect();
    match sender {
        _ if m.outgoing => format!("You: {line}"),
        Some(name) if !name.is_empty() => format!("{name}: {line}"),
        _ => line,
    }
}

/// A chat not in your lists, found on the server.
fn chat_kind_label(chat: &Chat) -> &'static str {
    match chat.kind {
        ChatKind::Private(_) => "User",
        ChatKind::Group => "Group",
        ChatKind::Channel => "Channel",
    }
}

fn chat_row<'a>(app: &'a App, chat: &'a Chat, selected: bool, focused: bool) -> Element<'a, Message> {
    let time = chat.last.as_ref().map(|m| fmt_time(m.date, true)).unwrap_or_default();
    let mut preview = chat.last.as_ref().map(|m| preview_of(m, None)).unwrap_or_default();
    if !chat.listed() {
        preview = format!("{} · not in your chats   {preview}", chat_kind_label(chat));
    }

    // Text is clipped to its own column so it never runs under the time.
    let mut title_line = row![
        container(emoji_text(&chat.title).size(TEXT).wrapping(Wrapping::None))
            .width(Length::Fill)
            .clip(true),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if let Some(mark) = mark_text(chat.last.as_ref().and_then(|m| delivery_mark(app, m))) {
        title_line = title_line.push(mark);
    }
    // Marked with `v` to forward to.
    if app.forward.is_some() && app.forward_marks.contains(&chat.id) {
        title_line = title_line.push(text("✓").size(15).style(primary_text));
    }
    let title_line = title_line.push(text(time).size(12).style(muted_text));
    let preview = emoji_text(&preview).style(muted_text);
    list_row(avatar(app, chat), title_line, preview, chat.unread, selected, focused, Message::ClickChat(chat.id))
}

/// A message found by the chat filter: its chat, when, and the message
/// itself (in the text color, unlike a chat's last message).
fn hit_row<'a>(app: &'a App, m: &'a Msg, i: usize, selected: bool, focused: bool) -> Element<'a, Message> {
    // TDLib knows the chat, but its update may come a moment later.
    let chat = app.chats.get(&m.chat_id);
    let title = chat.map_or("", |c| c.title.as_str());
    let title_line = row![
        container(emoji_text(title).size(TEXT).wrapping(Wrapping::None))
            .width(Length::Fill)
            .clip(true),
        text(fmt_time(m.date, true)).size(12).style(muted_text),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let sender = chat.is_some_and(|c| c.kind == ChatKind::Group).then(|| app.sender_name(m.sender));
    let preview = emoji_text(&preview_of(m, sender));
    let avatar = avatar_image(app, chat.and_then(|c| c.avatar), m.chat_id, title, AVATAR);
    list_row(avatar, title_line, preview, 0, selected, focused, Message::ClickHit(i))
}

/// A row of the chat list: avatar, title line, two preview lines.
#[allow(clippy::too_many_arguments)]
fn list_row<'a>(
    avatar: Element<'a, Message>,
    title_line: Row<'a, Message>,
    preview: text::Rich<'a, (), Message>,
    unread: i32,
    selected: bool,
    focused: bool,
    on_press: Message,
) -> Element<'a, Message> {
    let mut preview_line = row![
        container(preview.size(13).wrapping(Wrapping::WordOrGlyph))
            .width(Length::Fill)
            .height(PREVIEW_H)
            .clip(true)
    ]
    .spacing(8);
    if unread > 0 {
        preview_line = preview_line.push(badge(fmt_count(unread)));
    }

    mouse_area(
        container(
            row![avatar, column![title_line, preview_line].spacing(4)]
                .spacing(10)
                .align_y(Alignment::Center),
        )
        .height(CHAT_ROW_H)
        .width(Length::Fill)
        .padding([7, 10])
        .clip(true)
        .style(move |t: &Theme| selection_style(t, selected, focused)),
    )
    .on_press(on_press)
    .interaction(Interaction::Pointer)
    .into()
}

/// Selected row: a strong tint of the accent color with a solid accent
/// outline when the pane has focus, so the active pane stands out; only a
/// neutral lift otherwise. The text keeps the theme's own colors, so it
/// stays readable either way.
fn selection_style(t: &Theme, selected: bool, focused: bool) -> container::Style {
    if !selected {
        return container::Style::default();
    }
    let p = t.extended_palette();
    if focused {
        container::Style {
            background: Some(p.primary.base.color.scale_alpha(0.35).into()),
            border: iced::border::width(2).color(p.primary.base.color).rounded(4),
            ..Default::default()
        }
    } else {
        container::Style {
            background: Some(p.background.weak.color.scale_alpha(0.5).into()),
            ..Default::default()
        }
    }
}

/// The message under the cursor: a neutral lift with a solid accent
/// outline, so sender colors, times and ticks stay readable. Messages taken
/// in by visual mode only get the lift and a faint outline.
fn message_style(t: &Theme, selected: bool, visual: bool) -> container::Style {
    let mut style = container::Style::default();
    let p = t.extended_palette();
    if selected {
        style.background = Some(p.background.weak.color.scale_alpha(0.7).into());
        style.border = style.border.width(2).color(p.primary.base.color).rounded(4);
    } else if visual {
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
            text(format!("{} {who}", playing_icon(voice))).size(13).wrapping(Wrapping::None).width(Length::Fill),
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

    // Comments: under the channel's name and picture.
    let thread = open.thread.as_ref();
    let chat = app.chats.get(&thread.map_or(open.id, |t| t.channel));
    let title = chat.map_or("", |c| c.title.as_str());
    let title = match thread {
        Some(_) => format!("Comments · {title}"),
        None => title.to_owned(),
    };
    let mut details: Vec<String> = Vec::new();
    if let Some(presence) = app.presence(open.id).map(fmt_presence).filter(|p| !p.is_empty()) {
        details.push(presence);
    }
    if open.pins_only() {
        details.push("pinned only — Enter or Esc to the chat".into());
    } else if let Some(tag) = &open.tag {
        details.push(format!("only {tag} — Esc for all"));
    }
    match (open.total, thread) {
        (Some(total), Some(_)) => details.push(format!("{} comments", fmt_count(total))),
        (Some(total), None) => details.push(format!("{} messages", fmt_count(total))),
        (None, _) => {}
    }
    let activity = fmt_activity(app, open.id);
    if let Some(activity) = &activity {
        details.push(activity.clone());
    }
    if thread.is_some() {
        details.push("Esc back to the post".into());
    }
    if !open.at_latest {
        details.push("viewing history — G for latest".into());
    }
    let online = app.presence(open.id) == Some(Presence::Online) || activity.is_some();
    let header_avatar = match chat {
        Some(chat) => avatar_image(app, chat.avatar, chat.id, &chat.title, HEADER_AVATAR),
        None => space::horizontal().width(HEADER_AVATAR).into(),
    };
    let header = container(
        row![
            header_avatar,
            column![
                emoji_text(&title).size(15).wrapping(Wrapping::None),
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
        let tall = scrollable(container(item).padding([0.0, MSG_PAD_X]))
            .id(TALL_ID)
            .on_scroll(Message::TallScroll)
            .direction(Direction::Vertical(Scrollbar::new().width(4).scroller_width(4)))
            .height(Length::Fill);
        container(tall)
            .padding(iced::Padding { bottom: LIST_BOTTOM, ..Default::default() })
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
                .padding(iced::Padding {
                    top: 0.0,
                    right: MSG_PAD_X,
                    bottom: LIST_BOTTOM,
                    left: MSG_PAD_X,
                })
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
    if let Some((index, pinned)) = open.shown_pin().filter(|_| !open.pins_only()) {
        pane = pane.push(pinned_bar(pinned, open.pins.len() - index, open.pins.len())).push(rule::horizontal(1));
    }
    let messages: Element<_> = mouse_area(messages)
        .on_scroll(|delta| Message::Scroll(Pane::Messages, delta))
        .into();
    // Scrolled up (or in old history): a way back down besides `G`.
    let scrolled_up = !open.at_latest || open.bottom + 1 < open.msgs.len();
    let messages: Element<_> = if scrolled_up {
        stack![messages, to_latest_button()].into()
    } else {
        messages
    };
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

/// Round ↓ in the corner of the message list.
fn to_latest_button<'a>() -> Element<'a, Message> {
    let button = mouse_area(
        container(text("↓").size(20))
            .width(TO_LATEST)
            .height(TO_LATEST)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|t: &Theme| {
                let p = t.extended_palette();
                container::Style {
                    background: Some(p.background.weak.color.into()),
                    text_color: Some(p.background.weak.text),
                    border: iced::border::rounded(TO_LATEST / 2.0).color(p.background.strong.color).width(1.0),
                    ..Default::default()
                }
            }),
    )
    .on_press(Message::ToLatest)
    .interaction(Interaction::Pointer);
    container(button)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding(iced::Padding { right: MSG_PAD_X + 8.0, bottom: LIST_BOTTOM + 8.0, ..Default::default() })
        .into()
}

fn emoji_panel<'a>(app: &'a App, picker: &'a Picker) -> Element<'a, Message> {
    let tabs = Row::with_children(emoji::TABS.iter().enumerate().map(|(i, (icon, _))| {
        let active = i == picker.tab && picker.query.is_empty();
        mouse_area(
            container(text(*icon).size(17).font(EMOJI))
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

    let tabs: Element<_> = if picker.react.is_some() {
        container(text("React to the message").size(13))
            .height(EMOJI_TABS_H - 4.0)
            .align_y(Alignment::Center)
            .into()
    } else {
        tabs.into()
    };

    let search = if picker.searching {
        format!("/{}▏", picker.query)
    } else if !picker.query.is_empty() {
        format!("/{}   Esc — close", picker.query)
    } else if picker.react.is_some() {
        "/ search   1–9 or Enter react".to_owned()
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
                            container(text(*e).size(22).font(EMOJI))
                                .width(EMOJI_CELL)
                                .height(EMOJI_CELL)
                                .align_x(Alignment::Center)
                                .align_y(Alignment::Center)
                                .style(move |t: &Theme| selection_style(t, selected, true)),
                        )
                        .on_press(Message::EmojiPick((*e).to_owned()))
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

/// The pinned message `number` (the oldest is 1) of `total`.
fn pinned_bar(pinned: &Msg, number: usize, total: usize) -> Element<'_, Message> {
    let summary: String = pinned.summary().split_whitespace().collect::<Vec<_>>().join(" ");
    let summary: String = summary.chars().take(400).collect();
    let (title, counter) = if total > 1 {
        (format!("Pinned message #{number}   gp, then n/N   gP all"), format!("{number} / {total}"))
    } else {
        ("Pinned message   gp".to_owned(), String::new())
    };
    mouse_area(
        container(
            row![
                text("📌").size(16),
                column![
                    text(title).size(11).style(primary_text),
                    // Two lines at most, wrapped within the pane.
                    container(emoji_text(&summary).size(13).wrapping(Wrapping::WordOrGlyph).width(Length::Fill))
                        .width(Length::Fill)
                        .max_height(PINNED_TEXT_H)
                        .clip(true),
                ]
                .width(Length::Fill)
                .spacing(1),
                // Its own click: the list of them all.
                mouse_area(text(counter).size(12).style(muted_text))
                    .on_press(Message::ListPinned)
                    .interaction(Interaction::Pointer),
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

/// Progress that can be dragged: of a music file (`target`), or of the
/// video in the viewer (`None`).
fn scrubber<'a>(app: &App, target: Option<(i64, usize)>, progress: f64, girth: f32) -> Element<'a, Message> {
    let value = match app.scrub {
        Some((t, fraction)) if t == target => fraction,
        _ => progress.clamp(0.0, 1.0),
    };
    slider(0.0..=1.0, value, move |f| Message::Scrub(target, f))
        .step(0.001)
        .on_release(Message::ScrubDone)
        .height(girth + 8.0)
        .style(move |t: &Theme, status| {
            let p = t.extended_palette();
            let handle = if status == slider::Status::Active { girth + 1.0 } else { girth + 3.0 };
            slider::Style {
                rail: slider::Rail {
                    backgrounds: (p.primary.base.color.into(), p.background.strong.color.into()),
                    width: girth,
                    border: iced::border::rounded(girth / 2.0),
                },
                handle: slider::Handle {
                    shape: slider::HandleShape::Circle { radius: handle },
                    background: p.primary.base.color.into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                },
            }
        })
        .into()
}

/// Waveform bars, each a click target: playing from there.
fn waveform_row<'a>(msg_id: i64, item: usize, played: &str, rest: &str) -> Element<'a, Message> {
    let total = (played.chars().count() + rest.chars().count()).max(1) as f64;
    let bars = played.chars().map(|c| (c, true)).chain(rest.chars().map(|c| (c, false)));
    Row::with_children(bars.enumerate().map(|(i, (c, done))| {
        let bar = text(c.to_string()).size(14).style(if done { primary_text } else { muted_text });
        mouse_area(bar)
            .on_press(Message::SeekVoice { msg_id, item, fraction: i as f64 / total })
            .interaction(Interaction::Pointer)
            .into()
    }))
    .into()
}

fn voice_view<'a>(
    app: &'a App,
    msg_id: i64,
    item: usize,
    duration: i32,
    waveform: &'a [u8],
    listened: bool,
) -> Element<'a, Message> {
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
        waveform_row(msg_id, item, &played, &rest),
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

/// A music file. Album tracks share the message, so what plays is told by
/// the file.
fn audio_view<'a>(
    app: &'a App,
    (msg_id, item): (i64, usize),
    file: i32,
    media: &Media,
    duration: i32,
    width: f32,
) -> Element<'a, Message> {
    let playback = app.voice.as_ref().filter(|v| v.file_id == file);
    let (icon, progress, time) = match playback.map(|v| v.audio.as_ref()) {
        Some(Some(audio)) => {
            let total = audio.duration().map_or(f64::from(duration), |d| d.as_secs_f64()).max(0.001);
            let pos = audio.position().as_secs_f64();
            let icon = if audio.paused() { "▶" } else { "⏸" };
            let time = format!("{} / {}  {}", fmt_secs(pos), fmt_secs(total), player::fmt_speed(app.speed()));
            (icon, pos / total, time)
        }
        Some(None) => ("…", 0.0, "loading…".to_owned()),
        None if duration > 0 => ("▶", 0.0, fmt_duration(duration)),
        None => ("▶", 0.0, String::new()),
    };
    let name = media.track_name().unwrap_or_default();
    let info = column![
        text(format!("🎵 {name}")).size(14).wrapping(Wrapping::None),
        scrubber(app, Some((msg_id, item)), progress, 3.0),
        text(time).size(12).style(muted_text),
    ]
    .spacing(1)
    .width(Length::Fill)
    .clip(true);
    container(row![text(icon).size(18).style(primary_text), info].spacing(10).align_y(Alignment::Center))
        .width(width)
        .height(AUDIO.height)
        .align_y(Alignment::Center)
        .into()
}

fn media_cell<'a>(
    app: &'a App,
    msg_id: i64,
    item: usize,
    media: &'a Media,
    size: Size,
    album: bool,
    hidden: bool,
) -> Element<'a, Message> {
    let radius = match media {
        Media::Sticker { .. } => 0.0,
        Media::Video { round: true, .. } => size.width / 2.0,
        _ => 8.0,
    };
    let cell: Element<_> = match media {
        // Under a spoiler: painted over until shown.
        _ if hidden => container(text("Spoiler · Enter shows").size(13).style(|t: &Theme| text::Style {
            color: Some(t.palette().background),
        }))
        .width(size.width)
        .height(size.height)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |t: &Theme| container::Style {
            background: Some(t.palette().text.scale_alpha(0.85).into()),
            border: iced::border::rounded(radius),
            ..Default::default()
        })
        .into(),
        Media::Voice { duration, waveform, listened, .. } => {
            voice_view(app, msg_id, item, *duration, waveform, *listened)
        }
        Media::Audio { file, duration, .. } => audio_view(app, (msg_id, item), *file, media, *duration, size.width),
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
    let hidden = m.spoiler && app.hides(m);
    if m.media.len() == 1 {
        return media_cell(app, m.id, 0, &m.media[0], sizes[0], false, hidden);
    }
    let cols = if tracklist(&m.media) { 1 } else { album_layout(m.media.len(), width).0 };
    let cells: Vec<Element<_>> = m
        .media
        .iter()
        .zip(&sizes)
        .enumerate()
        .map(|(i, (media, size))| media_cell(app, m.id, i, media, *size, true, hidden))
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
    container(emoji_text(&label).size(12).wrapping(Wrapping::None).style(muted_text))
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

/// Keys that act on the message under the cursor, shown on its header.
fn message_keys(app: &App, m: &Msg) -> String {
    // Only the channel's admins delete its posts.
    let read_only = app.read_only(m.chat_id);
    if app.visual.is_some() {
        let delete = if read_only { "" } else { "  d delete" };
        return format!("{} selected  f forward  y copy{delete}  Esc cancel", fmt_count(app.visual_len()));
    }
    let channel = app.chats.get(&m.chat_id).is_some_and(|c| c.kind == ChatKind::Channel);
    let mut keys = Vec::new();
    if app.hides(m) {
        keys.push("Enter spoiler");
    }
    if !m.links.is_empty() {
        keys.push("gx link");
    }
    if m.forward.is_some() {
        keys.push("gf original");
    } else if m.reply_to.is_some() {
        keys.push("gf reply");
    }
    if m.comments.is_some() {
        keys.push("gc comments");
    }
    match m.media.first() {
        Some(Media::Voice { .. } | Media::Audio { .. } | Media::Video { round: true, .. }) => keys.push("Enter play"),
        Some(media) if media.viewable() => keys.push("Enter open"),
        _ => {}
    }
    if !m.deleted {
        if !channel || m.outgoing {
            keys.push("r reply");
        }
        keys.push("R react");
        if m.outgoing && m.media.is_empty() {
            keys.push("e edit");
        }
        keys.push("f forward");
    }
    if !m.text.is_empty() {
        keys.push("yy copy");
    }
    if !m.deleted && !read_only {
        keys.push("dd delete");
    }
    keys.join("  ")
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
    if selected {
        header = header.push(space::horizontal()).push(
            text(message_keys(app, m))
                .size(11)
                .wrapping(Wrapping::None)
                .style(|t: &Theme| text::Style { color: Some(muted(t).scale_alpha(0.8)) }),
        );
    }

    let mut body = column![header].spacing(2);
    if let Some(forward) = &m.forward {
        let from = match forward.sender {
            Some(sender) if forward.name.is_empty() => app.sender_name(sender).to_owned(),
            Some(sender) => format!("{} ({})", app.sender_name(sender), forward.name),
            None => forward.name.clone(),
        };
        body = body.push(
            mouse_area(
                container(text(format!("Forwarded from {from}")).size(12).wrapping(Wrapping::None).style(primary_text))
                    .height(FORWARD_H - 2.0)
                    .align_y(Alignment::Center)
                    .clip(true),
            )
            .on_press(Message::ClickSource(m.id))
            .interaction(Interaction::Pointer),
        );
    }
    if let Some(reply_to) = m.reply_to {
        body = body.push(
            mouse_area(reply_quote(app, reply_to))
                .on_press(Message::ClickSource(m.id))
                .interaction(Interaction::Pointer),
        );
    }
    if !m.media.is_empty() {
        body = body.push(container(media_block(app, m, width)).padding([MEDIA_GAP / 2.0, 0.0]));
    }
    if !m.text.is_empty() {
        body = body.push(message_text(app, m));
    }
    if let Some(preview) = &m.preview {
        body = body.push(preview_card(app, preview, width));
    }

    if !m.reactions.is_empty() {
        body = body.push(reactions_row(m));
    }
    if let Some(count) = m.comments {
        let label = match count {
            0 => "💬 Leave a comment".to_owned(),
            1 => "💬 1 comment".to_owned(),
            n => format!("💬 {} comments", fmt_count(n)),
        };
        body = body.push(
            mouse_area(
                container(text(label).size(13).style(primary_text))
                    .height(COMMENTS_H - 2.0)
                    .align_y(Alignment::Center),
            )
            .on_press(Message::OpenComments(m.id))
            .interaction(Interaction::Pointer),
        );
    }

    let visual = app.in_visual(m.id);
    // While writing, the compose box is what stands out; the message
    // replied to or edited only keeps a faint outline.
    let typing = matches!(app.mode, Mode::Insert | Mode::Text);
    let (selected, visual) = (selected && !typing, visual || selected && typing);
    let bubble = container(body)
        .padding(iced::Padding { top: 6.0, bottom: 10.0, left: 10.0, right: 10.0 })
        .width(Length::Fill)
        .style(move |t: &Theme| message_style(t, selected, visual));
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

/// Site, title, the start of the description and the picture of a link;
/// clicking it opens the link.
fn preview_card<'a>(app: &'a App, preview: &'a Preview, width: f32) -> Element<'a, Message> {
    let bold = Font { weight: font::Weight::Bold, ..Font::DEFAULT };
    let line = |s: &'a str| text(s).size(PREVIEW_TEXT).font(bold).wrapping(Wrapping::None);
    let mut lines = column![].width(Length::Fill);
    if !preview.site.is_empty() {
        lines = lines.push(line(&preview.site).style(primary_text));
    }
    if !preview.title.is_empty() {
        lines = lines.push(line(&preview.title));
    }
    let description = preview_description(preview, width);
    if !description.is_empty() {
        lines = lines.push(emoji_text(&description).size(PREVIEW_TEXT));
    }
    let mut card = row![lines].spacing(PREVIEW_PAD);
    if let Some(photo) = preview.photo {
        let picture: Element<_> = match app.media.image(photo.id) {
            Some(handle) => image(handle.clone())
                .width(PREVIEW_THUMB)
                .height(PREVIEW_THUMB)
                .content_fit(ContentFit::Cover)
                .border_radius(4.0)
                .into(),
            None => placeholder(Size::new(PREVIEW_THUMB, PREVIEW_THUMB), "", 12.0, 4.0),
        };
        card = card.push(picture);
    }
    let card = container(card)
        .padding(PREVIEW_PAD)
        .width(Length::Fill)
        .max_height(preview_height(preview, width) - MEDIA_GAP)
        .clip(true)
        .style(|t: &Theme| container::Style {
            background: Some(t.palette().primary.scale_alpha(0.08).into()),
            border: iced::border::rounded(6.0).color(t.palette().primary.scale_alpha(0.35)).width(1.0),
            ..Default::default()
        });
    container(
        mouse_area(card)
            .on_press(Message::OpenLink(preview.url.clone()))
            .interaction(Interaction::Pointer),
    )
    .padding(iced::Padding { top: MEDIA_GAP, ..Default::default() })
    .into()
}

/// Reaction chips; yours are highlighted, a click puts or takes back one.
fn reactions_row(m: &Msg) -> Element<'_, Message> {
    let chips = m.reactions.iter().map(|r| {
        let chosen = r.chosen;
        mouse_area(
            container(
                row![text(r.kind.label()).size(14).font(EMOJI), text(fmt_count(r.count)).size(12)]
                    .spacing(4)
                    .align_y(Alignment::Center),
            )
            .height(REACTION_CHIP_H)
            .align_y(Alignment::Center)
            .padding([0.0, 8.0])
            .style(move |t: &Theme| {
                let p = t.extended_palette();
                let (background, border) = if chosen {
                    (p.primary.weak.color.scale_alpha(0.35), p.primary.base.color)
                } else {
                    (p.background.weak.color, Color::TRANSPARENT)
                };
                container::Style {
                    background: Some(background.into()),
                    border: iced::border::rounded(11.0).color(border).width(1.0),
                    ..Default::default()
                }
            }),
        )
        .on_press(Message::ToggleReaction { msg_id: m.id, kind: r.kind.clone() })
        .interaction(Interaction::Pointer)
        .into()
    });
    container(Row::with_children(chips).spacing(6).wrap().vertical_spacing(4.0))
        .padding(iced::Padding { top: 4.0, ..Default::default() })
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
            // The player says it shrinks, so a column would lay it out
            // first, at the video's own height, at the top: in a filling
            // container it gets all the room left above the progress bar,
            // and draws the picture in the middle of it.
            Some(video) => column![
                container(
                    VideoPlayer::new(video)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(ContentFit::Contain),
                )
                .width(Length::Fill)
                .height(Length::Fill),
                video_progress(app, video),
            ]
            .spacing(10)
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

/// The viewer beside the chat: a click gives it the keys, a line on top
/// shows it has them.
fn side_viewer<'a>(app: &App, viewer: Element<'a, Message>) -> Element<'a, Message> {
    let focused = app.viewer_focused;
    let accent = container(space::horizontal())
        .width(Length::Fill)
        .height(2)
        .style(move |t: &Theme| container::Style {
            background: focused.then(|| t.palette().primary.into()),
            ..Default::default()
        });
    mouse_area(column![accent, viewer].width(side_width(app.size)).height(Length::Fill))
        .on_press(Message::FocusViewer)
        .into()
}

/// Under a video in the viewer: time played, progress, length.
fn video_progress<'a>(app: &App, video: &iced_video_player::Video) -> Element<'a, Message> {
    let total = video.duration().as_secs_f64();
    let pos = video.position().as_secs_f64().min(total);
    let time = |secs: f64| {
        text(fmt_secs(secs)).size(12).font(Font::MONOSPACE).style(|_: &Theme| text::Style {
            color: Some(Color::from_rgb8(0xc8, 0xc8, 0xd0)),
        })
    };
    // While dragging, the time shown is where it goes.
    let pos = match app.scrub {
        Some((None, fraction)) => fraction * total,
        _ => pos,
    };
    let progress = if total > 0.0 { pos / total } else { 0.0 };
    row![time(pos), scrubber(app, None, progress, 4.0), time(total)]
        .spacing(10)
        .padding([0, 4])
        .align_y(Alignment::Center)
        .into()
}

/// How the cursor of a line editor shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caret {
    Hidden,
    /// Typing: a bar between two letters.
    Bar,
    /// Text mode: a block over the letter under the cursor, like Vim.
    Block,
}

/// Line editor contents with its cursor, the text wrapped at `width`. The
/// cursor is drawn over the text instead of being a character in it, so it
/// takes no room: a `▏` glyph is a whole cell wide and left what looked like
/// a space after the cursor.
fn input_text<'a>(input: &Input, shown_caret: Caret, mask: bool, prefix: &str, width: f32) -> Element<'a, Message> {
    let (before, after) = input.split();
    let show = |s: &str| if mask { "•".repeat(s.chars().count()) } else { s.to_owned() };
    let before = format!("{prefix}{}", show(before));
    let content = format!("{before}{}", show(after));
    let shown = emoji_text(&content).size(TEXT);
    if shown_caret == Caret::Hidden {
        return shown.into();
    }
    let (x, y, height, letter) = caret(&content, before.len(), width);
    let (left, bar_width, alpha) = match shown_caret {
        Caret::Block => (x, if letter > 0.0 { letter } else { TEXT * 0.55 }, 0.45),
        _ => ((x - 1.0).max(0.0), 2.0, 1.0),
    };
    let bar = container(space::horizontal().width(bar_width).height(height))
        .style(move |t: &Theme| container::background(t.palette().primary.scale_alpha(alpha)));
    // The text's layer sets the size of the stack, and an empty text has
    // none: a full-width line keeps the cursor from being cut off.
    let base = row![space::vertical().width(0).height(height), shown].width(Length::Fill);
    stack![base, container(bar).padding(iced::Padding { top: y, left, ..Default::default() })].into()
}

/// Where the cursor at byte `at` of `content` falls once it is laid out
/// like the compose text: x, top of its line, line height, and the width of
/// the letter under it (0 at the end of a line).
fn caret(content: &str, at: usize, width: f32) -> (f32, f32, f32, f32) {
    use iced::advanced::graphics::text::Paragraph;
    use iced::advanced::text::{self as core_text, Paragraph as _};
    let spans: Vec<text::Span<'_, ()>> = emoji_spans(content).collect();
    let paragraph = Paragraph::with_spans(core_text::Text {
        content: spans.as_slice(),
        bounds: Size::new(width, f32::INFINITY),
        size: TEXT.into(),
        line_height: text::LineHeight::default(),
        font: Font::DEFAULT,
        align_x: core_text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: Wrapping::Word,
    });
    // cosmic-text counts bytes from the start of each paragraph line.
    let line = content[..at].matches('\n').count();
    let offset = at - content[..at].rfind('\n').map_or(0, |i| i + 1);
    let fallback = text::LineHeight::default().to_absolute(TEXT.into()).0;
    let mut last = None;
    for run in paragraph.buffer().layout_runs().filter(|r| r.line_i == line) {
        if let Some(glyph) = run.glyphs.iter().find(|g| g.start >= offset) {
            return (glyph.x, run.line_top, run.line_height, glyph.w);
        }
        last = Some((run.line_w, run.line_top, run.line_height, 0.0));
    }
    last.unwrap_or((0.0, line as f32 * fallback, fallback, 0.0))
}

/// "📎 photo.png" or "📎 3 files: a.png, b.mp4, …"
fn attachments_label(paths: &[std::path::PathBuf]) -> String {
    let names: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()))
        .collect();
    match names.len() {
        1 => format!("📎 {}", names[0]),
        n => format!("📎 {} files: {}", fmt_count(n), names.join(", ")),
    }
}

fn compose_banner(app: &App) -> Option<Element<'_, Message>> {
    if let Some(forward) = &app.forward {
        return Some(banner(format!("↪ Forward {}, the text goes first as a comment", forward.what)));
    }
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
    Some(banner(label))
}

fn banner<'a>(label: String) -> Element<'a, Message> {
    row![
        text(label).size(18).wrapping(Wrapping::None).style(primary_text).width(Length::Fill),
        text("Esc — cancel").size(15).style(muted_text),
    ]
    .height(BANNER_H)
    .align_y(Alignment::Center)
    .into()
}

pub fn has_banner(app: &App) -> bool {
    app.forward.is_some() || app.compose != Compose::New || !app.attachments.is_empty()
}

/// In place of the compose line in a channel you can't post in.
fn channel_bar(app: &App, chat: &Chat) -> Element<'static, Message> {
    let mute = if app.muted(chat) { "m — unmute" } else { "m — mute" };
    let leave = if app.membership(chat) == Some(Membership::Left) { "U — subscribe" } else { "U — unsubscribe" };
    container(text(format!("Channel   {mute}   {leave}   gc — comments")).size(TEXT).style(muted_text))
        .padding([10.0, MSG_PAD_X])
        .width(Length::Fill)
        .height(20.0 + LINE_H)
        .align_y(Alignment::Center)
        .into()
}

fn compose(app: &App) -> Element<'_, Message> {
    if let Some(chat) = app.open.as_ref().filter(|o| app.read_only(o.id)).and_then(|o| app.chats.get(&o.id)) {
        return channel_bar(app, chat);
    }
    let caret = match app.mode {
        Mode::Insert => Caret::Bar,
        Mode::Text => Caret::Block,
        _ => Caret::Hidden,
    };
    let editing = caret != Caret::Hidden;
    let width = compose_text_width(app.size, app.emoji.is_some(), app.side_viewer());
    let lines = compose_lines(app.input.as_str(), width).clamp(1, MAX_COMPOSE_LINES);
    let content: Element<_> = if app.input.is_empty() && !editing {
        let hint = if app.forward.is_some() {
            "i — add a comment   Enter — send"
        } else {
            "i — write a message   q — record voice   ga — attach file   ge — emoji"
        };
        text(hint)
            .size(TEXT)
            .style(muted_text)
            .into()
    } else {
        // Longer than the box: scrolled to the cursor by the app.
        scrollable(input_text(&app.input, caret, false, "", width))
            .id(COMPOSE_ID)
            .direction(Direction::Vertical(Scrollbar::hidden()))
            .width(Length::Fill)
            .height(lines as f32 * LINE_H)
            .into()
    };
    let mut body = column![].width(Length::Fill);
    if let Some(banner) = compose_banner(app) {
        body = body.push(banner);
    }
    body = body.push(content);
    let open = app.emoji.is_some();
    let button = mouse_area(
        container(text("😀").size(18).line_height(1.0))
            .width(COMPOSE_BUTTON)
            .height(COMPOSE_BUTTON)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |t: &Theme| selection_style(t, open, true)),
    )
    .on_press(Message::ToggleEmoji)
    .interaction(Interaction::Pointer);
    container(row![body, button].spacing(COMPOSE_GAP).align_y(Alignment::End))
        .padding([10.0, MSG_PAD_X])
        .width(Length::Fill)
        .max_height(compose_height(&app.input, width, has_banner(app)))
        .clip(true)
        .style(move |t: &Theme| compose_style(t, editing))
        .into()
}

/// The compose box being written in: a tint and a solid outline of the
/// accent color, like the selected row of the focused pane.
fn compose_style(t: &Theme, editing: bool) -> container::Style {
    if !editing {
        return container::Style::default();
    }
    let p = t.extended_palette();
    container::Style {
        background: Some(p.primary.base.color.scale_alpha(0.12).into()),
        border: iced::border::width(2).color(p.primary.base.color).rounded(4),
        ..Default::default()
    }
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
            container(input_text(&app.input, if editing { Caret::Bar } else { Caret::Hidden }, mask, "", 380.0 - 24.0))
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

fn playing_icon(voice: &VoicePlayback) -> &'static str {
    if voice.kind == SpeedKind::Music { "🎵" } else { "🎤" }
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
    let voice = app.voice.as_ref()?;
    let audio = voice.audio.as_ref()?;
    let icon = if audio.paused() { "⏸" } else { playing_icon(voice) };
    let total = audio.duration().map_or(0.0, |d| d.as_secs_f64());
    Some(format!(
        "{icon} {} / {}  {speed}",
        fmt_secs(audio.position().as_secs_f64()),
        fmt_secs(total)
    ))
}

/// Something loading, right of the mode: a ring filling up with the
/// progress of the files on their way, or spinning when it's not known.
fn busy_indicator<'a>(app: &App, busy: Busy) -> Element<'a, Message> {
    let spin = app.now.saturating_duration_since(app.clock).as_secs_f32();
    let (progress, label) = match busy {
        Busy::Files { count, up, down, progress } => {
            let arrow = match (up, down) {
                (true, true) => "⇅",
                (true, false) => "↑",
                _ => "↓",
            };
            let what = if count > 1 { format!("{arrow} {} files", fmt_count(count)) } else { arrow.to_owned() };
            let label = match progress {
                Some(p) => format!("{what} {}%", (p * 100.0).floor() as u32),
                None => what,
            };
            (progress, label)
        }
        Busy::Loading => (None, "Loading".to_owned()),
    };
    row![
        canvas(Ring { progress, spin }).width(BUSY_RING).height(BUSY_RING),
        text(label).size(12).style(primary_text).wrapping(Wrapping::None),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

const BUSY_RING: f32 = 16.0;

/// A progress ring: an arc from the top as far as `progress` goes, or a
/// quarter turning round when it's `None`.
struct Ring {
    progress: Option<f32>,
    /// Seconds since the app started, for the turning arc.
    spin: f32,
}

impl canvas::Program<Message> for Ring {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        use std::f32::consts::{FRAC_PI_2, TAU};
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = frame.center();
        let width = 2.5;
        let radius = bounds.width.min(bounds.height) / 2.0 - width;
        let accent = theme.palette().primary;
        let stroke = |color| canvas::Stroke::default().with_width(width).with_color(color).with_line_cap(canvas::LineCap::Round);
        frame.stroke(&canvas::Path::circle(center, radius), stroke(accent.scale_alpha(0.25)));
        let (start, sweep) = match self.progress {
            Some(p) => (-FRAC_PI_2, p.clamp(0.0, 1.0) * TAU),
            None => ((self.spin * TAU) % TAU, TAU / 4.0),
        };
        if sweep > 0.0 {
            let arc = canvas::Path::new(|b| {
                b.arc(canvas::path::Arc {
                    center,
                    radius,
                    start_angle: iced::Radians(start),
                    end_angle: iced::Radians(start + sweep),
                })
            });
            frame.stroke(&arc, stroke(accent));
        }
        vec![frame.into_geometry()]
    }
}

fn status_line(app: &App) -> Element<'_, Message> {
    let mode = app.mode;
    let visual = app.visual.is_some() && mode == Mode::Normal;
    let badge = container(text(if visual { "VISUAL" } else { mode.label() }).size(12))
        .padding([3, 10])
        .style(move |t: &Theme| {
            let p = t.extended_palette();
            let pair = match mode {
                Mode::Normal if visual => p.secondary.strong,
                Mode::Normal => p.primary.strong,
                Mode::Insert => p.success.base,
                Mode::Text => p.success.strong,
                Mode::Command | Mode::Search => p.warning.base,
                Mode::Record => p.danger.base,
            };
            container::Style {
                background: Some(pair.color.into()),
                text_color: Some(pair.text),
                ..Default::default()
            }
        });

    let busy = app.busy().map(|busy| busy_indicator(app, busy));

    let viewer_hint = match app.viewer_media() {
        _ if !app.viewer_keys() => None,
        Some(Media::Video { .. }) if app.viewer_side => Some(
            "Space pause   h/l ±5s   H/L ±30s   [ ] speed   = 1x   { } volume   j/k next/prev   Tab messages   s full screen   Esc close",
        ),
        Some(Media::Video { .. }) => Some(
            "Space pause   h/l ±5s   H/L ±30s   [ ] speed   = 1x   { } volume   j/k next/prev   s beside the chat   Esc close",
        ),
        Some(_) if app.viewer_side => Some("j/k — next/previous   Tab — messages   s — full screen   Esc — close"),
        Some(_) => Some("j/k — next/previous   s — beside the chat   Esc — close"),
        None => None,
    };

    let middle: Element<_> = match mode {
        _ if app.confirm.is_some() => {
            let prompt = match &app.confirm {
                Some(Confirm::Delete { ids, .. }) => {
                    let what = if ids.len() > 1 { "these messages" } else { "this message" };
                    format!("Delete {what}?   y — for everyone   m — only for me   n/Esc — cancel")
                }
                Some(Confirm::Leave { title, .. }) => format!("Leave {title}?   y/Enter — leave   n/Esc — stay"),
                _ => "Quit vimgram?   y/Enter — quit   n/Esc — stay".to_owned(),
            };
            text(prompt)
                .size(13)
                .style(|t: &Theme| text::Style { color: Some(t.palette().danger) })
                .into()
        }
        Mode::Command => input_text(&app.cmdline, Caret::Bar, false, ":", f32::INFINITY),
        Mode::Search => input_text(&app.cmdline, Caret::Bar, false, "/", f32::INFINITY),
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
            format!("{}/{}", fmt_count(open.cursor + 1), fmt_count(open.msgs.len()))
        }
        _ => app
            .cursor_row()
            .map(|i| format!("{}/{}", fmt_count(i + 1), fmt_count(app.rows())))
            .unwrap_or_default(),
    };

    let mut line = row![badge].push(busy).push(container(middle).width(Length::Fill).clip(true))
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some(playback) = playback_status(app) {
        line = line.push(text(playback).size(13).style(primary_text));
    }
    line = line
        .push(text(if mode == Mode::Text { app.text_pending.display() } else { app.pending.display() }).size(13))
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

    #[test]
    fn preview_description_fits_three_lines() {
        let width = 20.0 + 2.0 * PREVIEW_PAD + 10.0 * PREVIEW_TEXT * 0.6 * 2.0; // 20 letters a line
        let preview = |description: &str| Preview {
            url: String::new(),
            site: "Site".into(),
            title: String::new(),
            description: description.into(),
            photo: None,
        };
        assert_eq!(preview_description(&preview("short"), width), "short");
        assert_eq!(preview_description(&preview("a\n\nb\nc\nd"), width), "a\nb\nc");
        let long = preview_description(&preview(&"x".repeat(100)), width);
        assert_eq!(long.chars().count(), 60);
        assert!(long.ends_with('…'));
        // Site line plus three of the description.
        assert_eq!(preview_height(&preview(&"x".repeat(100)), width), 4.0 * PREVIEW_LINE_H + 2.0 * PREVIEW_PAD + MEDIA_GAP);
    }

    #[test]
    fn side_viewer_takes_half_of_the_chat_pane() {
        let size = Size::new(1416.0, 800.0);
        let input = Input::default();
        let (full, _) = messages_viewport(size, &input, false, false, false, false, false);
        let (side, _) = messages_viewport(size, &input, false, false, false, false, true);
        assert_eq!(side_width(size), 500.0);
        assert_eq!(full - side, 501.0);
    }

    #[test]
    fn caret_sits_between_letters_and_follows_wrapping() {
        let wide = f32::INFINITY;
        let x = |content: &str, at: usize, width: f32| caret(content, at, width).0;
        let (start, top, height, letter) = caret("hello world", 0, wide);
        assert_eq!((start, top), (0.0, 0.0));
        assert!(height > TEXT && letter > 0.0);
        let (mid, end) = (x("hello world", 5, wide), x("hello world", 11, wide));
        assert!(0.0 < mid && mid < end);
        assert_eq!(caret("hello world", 11, wide).3, 0.0);
        // The letters before the cursor are where they are without it.
        assert_eq!(x("hello", 5, wide), mid);
        let (at, y, _, _) = caret("hello\nworld", 9, wide);
        assert_eq!(y, height);
        assert_eq!(at, x("wor", 3, wide));
        // A word that no longer fits goes to the next line, the cursor in it too.
        let narrow = x("hello wor", 9, wide);
        let (at, y, _, _) = caret("hello world", 9, narrow);
        assert_eq!(y, height);
        assert_eq!(at, x("wor", 3, wide));
    }

    #[test]
    fn compose_counts_wrapped_lines() {
        let width = 10.0 * TEXT * 0.55 * 4.0; // 40 letters a line
        assert_eq!(compose_lines("", width), 1);
        assert_eq!(compose_lines("short\nlines", width), 2);
        // A long line wraps; this is what hid the end of a long message.
        assert_eq!(compose_lines(&"word ".repeat(30), width), 4);
        assert_eq!(compose_lines(&format!("{}\nend", "x".repeat(81)), width), 4);
    }

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
    fn blocks_are_separated() {
        let noon = Local::now().date_naive().and_hms_opt(12, 0, 0).unwrap();
        let t = noon.and_local_timezone(Local).unwrap().timestamp() as i32;
        let chat = [msg_at(1, t), msg_at(2, t + 60), msg_at(3, t + 60 + 2 * 3600)];
        let starts: Vec<bool> = (0..chat.len()).map(|i| starts_block(&chat, i)).collect();
        assert_eq!(starts, [false, false, true], "an hour of silence starts a block");
        let width = 500.0;
        assert_eq!(item_height(&chat, 2, width), message_height(&chat[2], width) + BLOCK_H);
        // Every channel post is separated, but the first of a day gets the date only.
        let post = |id, date| Msg { sender: Sender::Chat(1), ..msg_at(id, date) };
        let channel = [post(1, t), post(2, t + 60)];
        assert_eq!([starts_block(&channel, 0), starts_block(&channel, 1)], [false, true]);
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

    #[test]
    fn music_album_is_a_tracklist() {
        let track = Media::Audio { file: 1, size: 0, duration: 0, title: String::new(), performer: String::new() };
        let tracks = vec![track.clone(), track.clone(), track];
        let sizes = media_sizes(&tracks, 1000.0);
        assert!(sizes.iter().all(|s| s.height == AUDIO.height && s.width == AUDIO.width));
        assert_eq!(media_block_height(&tracks, 1000.0), 3.0 * AUDIO.height + 2.0 * ALBUM_GAP + MEDIA_GAP);
    }
}
