//! Vim-style modal input: mode, Normal-mode key parsing and a line editor.

use iced::keyboard::key::{Code, Named, Physical};
use iced::keyboard::{Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// Normal mode inside the message being written: `Esc` from Insert.
    Text,
    Command,
    Search,
    /// Recording a voice message.
    Record,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Text => "TEXT",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Record => "RECORD",
        }
    }
}

/// A Normal-mode key, independent of the active keyboard layout: with a
/// Cyrillic layout `о` still means `j` and `Shift+п` still means `G`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NKey {
    Char(char),
    Ctrl(char),
    Alt(char),
    Enter,
    Esc,
    Tab,
    Space,
    Up,
    Down,
    Left,
    Right,
}

/// Punctuation with a Normal-mode meaning.
const COMMAND_CHARS: &[char] = &[':', '/', '[', ']', '=', '+', '-'];

pub fn normalize(key: &Key, physical: Physical, mods: Modifiers) -> Option<NKey> {
    if let Key::Named(named) = key {
        return match named {
            Named::Enter => Some(NKey::Enter),
            Named::Escape => Some(NKey::Esc),
            Named::Tab => Some(NKey::Tab),
            Named::Space => Some(NKey::Space),
            Named::ArrowUp => Some(NKey::Up),
            Named::ArrowDown => Some(NKey::Down),
            Named::ArrowLeft => Some(NKey::Left),
            Named::ArrowRight => Some(NKey::Right),
            _ => None,
        };
    }

    let latin = key.to_latin(physical).map(|c| {
        if c.is_ascii_alphabetic() && mods.shift() {
            c.to_ascii_uppercase()
        } else {
            c
        }
    });

    if mods.control() {
        return match latin {
            Some('[') => Some(NKey::Esc),
            Some(c) => Some(NKey::Ctrl(c.to_ascii_lowercase())),
            None => None,
        };
    }
    if mods.alt() {
        return latin.map(|c| NKey::Alt(c.to_ascii_lowercase()));
    }

    match latin {
        Some(c) if c.is_ascii_alphanumeric() || COMMAND_CHARS.contains(&c) => Some(NKey::Char(c)),
        // Punctuation that a non-latin layout moved elsewhere: fall back to
        // the physical key position of a US layout.
        _ => match (physical, mods.shift()) {
            (Physical::Code(Code::Semicolon), true) => Some(NKey::Char(':')),
            (Physical::Code(Code::Slash), false) => Some(NKey::Char('/')),
            (Physical::Code(Code::BracketLeft), false) => Some(NKey::Char('[')),
            (Physical::Code(Code::BracketRight), false) => Some(NKey::Char(']')),
            (Physical::Code(Code::Equal), false) => Some(NKey::Char('=')),
            (Physical::Code(Code::Equal), true) => Some(NKey::Char('+')),
            (Physical::Code(Code::Minus), false) => Some(NKey::Char('-')),
            // Line end, first letter and case of the compose text.
            (Physical::Code(Code::Digit4), true) => Some(NKey::Char('$')),
            (Physical::Code(Code::Digit6), true) => Some(NKey::Char('^')),
            (Physical::Code(Code::Backquote), true) => Some(NKey::Char('~')),
            _ => latin.map(NKey::Char),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down(usize),
    Up(usize),
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    NextFolder,
    PrevFolder,
    /// Zero-based folder index.
    Folder(usize),
    FocusChats,
    FocusMessages,
    ToggleFocus,
    Open,
    Insert,
    /// Write with the cursor at the start of the message (`I`).
    InsertStart,
    /// Write with the cursor at the end of the message (`a`, `A`).
    Append,
    Command,
    Search,
    Yank,
    /// Paste the clipboard into the message being composed.
    Paste { before: bool },
    /// Start recording a voice message, like `q` starts a macro.
    Record,
    PlayPause,
    Faster,
    Slower,
    NormalSpeed,
    Louder,
    Quieter,
    /// Turn the chat's notifications off, or back on.
    Mute,
    /// Leave a channel or group (asks first), or join one you are not in.
    Leave,
    SeekBack,
    SeekForward,
    Reply,
    React,
    Edit,
    Forward,
    Delete,
    /// Attach files (`ga`).
    Attach,
    /// Jump to the pinned message the bar shows, or the one before it (`gp`).
    Pinned,
    /// Only the pinned messages (`gP`).
    PinnedList,
    /// The original of a forwarded message, or the message replied to (`gf`).
    GoToSource,
    /// The comments of a channel post (`gc`).
    Comments,
    /// Messages: start / end a Visual selection (`V`).
    Visual,
    /// Mark / unmark the message, or the Visual range, to act on several
    /// with gaps between them; picking chats to forward to, the chat (`v`).
    Mark,
    /// Open the n-th (1-based) link of the message (`gx`, `2gx`).
    OpenLink(usize),
    /// Back to where `gf` came from (`Ctrl-o`).
    JumpBack,
    /// Forward again to where `Ctrl-o` came back from (`Ctrl-i`).
    JumpForward,
    /// Emoji panel (`ge`).
    Emoji,
    /// Back to the voice message playing in another chat (`gv`).
    JumpPlaying,
    /// The first unread message of the topmost unread private chat (`gu`)
    /// or channel (`gk`), in its folder.
    FirstUnread { channel: bool },
    SearchNext,
    SearchPrev,
    Cancel,
}

/// Pending count and operator prefix, e.g. `12` in `12j` or `g` in `gg`.
#[derive(Debug, Default)]
pub struct Pending {
    count: Option<usize>,
    prefix: Option<char>,
}

impl Pending {
    pub fn feed(&mut self, key: NKey) -> Option<Action> {
        if let NKey::Char(c @ '0'..='9') = key
            && (c != '0' || self.count.is_some())
            && self.prefix.is_none()
        {
            let digit = c.to_digit(10).unwrap() as usize;
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            return None;
        }

        let explicit = self.count.take();
        let count = explicit.unwrap_or(1);
        let prefix = self.prefix.take();

        match (prefix, key) {
            (Some('g'), NKey::Char('g')) => Some(Action::Top),
            // Like Vim tabs: `gt` next, `gT` previous, `3gt` third.
            (Some('g'), NKey::Char('t')) => Some(match explicit {
                Some(n) => Action::Folder(n.saturating_sub(1)),
                None => Action::NextFolder,
            }),
            (Some('g'), NKey::Char('T')) => Some(Action::PrevFolder),
            (Some('g'), NKey::Char('a')) => Some(Action::Attach),
            (Some('g'), NKey::Char('p')) => Some(Action::Pinned),
            (Some('g'), NKey::Char('P')) => Some(Action::PinnedList),
            (Some('g'), NKey::Char('f')) => Some(Action::GoToSource),
            (Some('g'), NKey::Char('c')) => Some(Action::Comments),
            (Some('g'), NKey::Char('x')) => Some(Action::OpenLink(explicit.unwrap_or(1))),
            (Some('g'), NKey::Char('e')) => Some(Action::Emoji),
            (Some('g'), NKey::Char('v')) => Some(Action::JumpPlaying),
            (Some('g'), NKey::Char('u')) => Some(Action::FirstUnread { channel: false }),
            (Some('g'), NKey::Char('k')) => Some(Action::FirstUnread { channel: true }),
            (Some('y'), NKey::Char('y')) => Some(Action::Yank),
            // `cc` changes the line in Vim; here it edits the message.
            (Some('c'), NKey::Char('c')) => Some(Action::Edit),
            (Some('d'), NKey::Char('d')) => Some(Action::Delete),
            (Some(_), _) => None,
            (None, NKey::Char(c @ ('g' | 'y' | 'c' | 'd'))) => {
                self.prefix = Some(c);
                self.count = explicit;
                None
            }
            (None, NKey::Char('J')) => Some(Action::NextFolder),
            (None, NKey::Char('K')) => Some(Action::PrevFolder),
            (None, NKey::Alt(c @ '1'..='9')) => Some(Action::Folder(c as usize - '1' as usize)),
            (None, NKey::Char('j') | NKey::Down | NKey::Ctrl('n')) => Some(Action::Down(count)),
            (None, NKey::Char('k') | NKey::Up | NKey::Ctrl('p')) => Some(Action::Up(count)),
            (None, NKey::Char('G')) => Some(Action::Bottom),
            (None, NKey::Ctrl('d')) => Some(Action::HalfDown),
            (None, NKey::Ctrl('u')) => Some(Action::HalfUp),
            (None, NKey::Ctrl('o')) => Some(Action::JumpBack),
            (None, NKey::Ctrl('i')) => Some(Action::JumpForward),
            // Ctrl-h/l like moving between Vim windows; Ctrl+arrows arrive as arrows.
            (None, NKey::Char('h') | NKey::Left | NKey::Ctrl('h')) => Some(Action::FocusChats),
            (None, NKey::Char('l') | NKey::Right | NKey::Ctrl('l')) => Some(Action::FocusMessages),
            (None, NKey::Tab) => Some(Action::ToggleFocus),
            (None, NKey::Enter | NKey::Char('o')) => Some(Action::Open),
            (None, NKey::Char('i')) => Some(Action::Insert),
            (None, NKey::Char('I')) => Some(Action::InsertStart),
            (None, NKey::Char('a' | 'A')) => Some(Action::Append),
            (None, NKey::Char(':')) => Some(Action::Command),
            (None, NKey::Char('/')) => Some(Action::Search),
            (None, NKey::Char('q')) => Some(Action::Record),
            (None, NKey::Char('Y')) => Some(Action::Yank),
            (None, NKey::Char('p')) => Some(Action::Paste { before: false }),
            (None, NKey::Char('P')) => Some(Action::Paste { before: true }),
            (None, NKey::Space) => Some(Action::PlayPause),
            (None, NKey::Char(']' | '+')) => Some(Action::Faster),
            (None, NKey::Char('[' | '-')) => Some(Action::Slower),
            (None, NKey::Char('=')) => Some(Action::NormalSpeed),
            (None, NKey::Char('}')) => Some(Action::Louder),
            (None, NKey::Char('{')) => Some(Action::Quieter),
            (None, NKey::Char('m')) => Some(Action::Mute),
            (None, NKey::Char('U')) => Some(Action::Leave),
            (None, NKey::Char('H')) => Some(Action::SeekBack),
            (None, NKey::Char('L')) => Some(Action::SeekForward),
            (None, NKey::Char('r')) => Some(Action::Reply),
            (None, NKey::Char('R')) => Some(Action::React),
            (None, NKey::Char('v')) => Some(Action::Mark),
            (None, NKey::Char('V')) => Some(Action::Visual),
            (None, NKey::Char('e')) => Some(Action::Edit),
            (None, NKey::Char('f')) => Some(Action::Forward),
            (None, NKey::Char('n')) => Some(Action::SearchNext),
            (None, NKey::Char('N')) => Some(Action::SearchPrev),
            (None, NKey::Esc) => Some(Action::Cancel),
            _ => None,
        }
    }

    /// Keys typed so far, shown in the status line like Vim's `showcmd`.
    pub fn display(&self) -> String {
        let mut s = self.count.map(|n| n.to_string()).unwrap_or_default();
        s.extend(self.prefix);
        s
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Single-buffer line editor used by Insert, Command and Search modes.
#[derive(Debug, Default, Clone)]
pub struct Input {
    text: String,
    /// Byte offset, always on a char boundary.
    cursor: usize,
    /// Earlier and undone states for `u` and `Ctrl-r`.
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
}

impl Input {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn split(&self) -> (&str, &str) {
        self.text.split_at(self.cursor)
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    pub fn backspace(&mut self) {
        let start = self.prev_boundary();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn delete(&mut self) {
        let end = self.next_boundary();
        self.text.replace_range(self.cursor..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.prev_boundary();
    }

    pub fn right(&mut self) {
        self.cursor = self.next_boundary();
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Ctrl-W: delete the word before the cursor.
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Ctrl-U: delete everything before the cursor.
    pub fn delete_to_start(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.undo.clear();
        self.redo.clear();
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.take();
    }
}

const UNDO_MAX: usize = 100;

/// What a key in Text mode asks of the app.
#[derive(Debug, PartialEq, Eq)]
pub enum TextAction {
    None,
    Insert,
    /// Back to the message list (`Esc`).
    Leave,
    Submit,
    /// Paste the clipboard at the cursor (moved already).
    Paste,
    Yank(String),
}

/// Where a motion goes, and what an operator then takes.
#[derive(Debug, Clone, Copy)]
enum Motion {
    /// Up to the byte, not including it (`w`, `b`, `h`, `l`, `$`).
    To(usize),
    /// Through the char at the byte (`e`).
    Through(usize),
    /// The whole lines from the cursor's to the byte's (`j`, `k`, `gg`, `G`).
    Lines(usize),
}

/// Vim's word classes: blanks, letters and digits, other signs.
fn class(c: char, big: bool) -> u8 {
    match c {
        c if c.is_whitespace() => 0,
        _ if big => 1,
        c if c.is_alphanumeric() || c == '_' => 1,
        _ => 2,
    }
}

/// Count, operator and `g` typed so far in Text mode: `3w`, `d2w`, `gg`.
#[derive(Debug, Default)]
pub struct TextPending {
    count: Option<usize>,
    /// Operator waiting for its motion, with the count typed before it.
    op: Option<(char, usize)>,
    g: bool,
}

impl TextPending {
    pub fn feed(&mut self, key: NKey, input: &mut Input) -> TextAction {
        if let NKey::Char(c @ '0'..='9') = key
            && (c != '0' || self.count.is_some())
        {
            let digit = c.to_digit(10).unwrap() as usize;
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            return TextAction::None;
        }
        let count = self.count.take();
        if key == NKey::Char('g') && !self.g {
            self.g = true;
            self.count = count;
            return TextAction::None;
        }
        let g = std::mem::take(&mut self.g);
        let n = count.unwrap_or(1).min(10_000);
        if let Some((op, first)) = self.op.take() {
            let n = n.saturating_mul(first);
            if !g && key == NKey::Char(op) {
                let last = input.line_down(input.cursor, n - 1);
                return input.operate(op, Motion::Lines(last));
            }
            // `cw` changes up to the end of the word, like Vim.
            let key = match key {
                NKey::Char(c @ ('w' | 'W')) if op == 'c' && input.char_at(input.cursor).is_some_and(|c| !c.is_whitespace()) => {
                    NKey::Char(if c == 'w' { 'e' } else { 'E' })
                }
                _ => key,
            };
            return match input.motion(key, n, g, true) {
                Some(motion) => input.operate(op, motion),
                None => TextAction::None,
            };
        }
        if let Some(motion) = input.motion(key, n, g, false) {
            input.cursor = match motion {
                Motion::To(at) | Motion::Through(at) | Motion::Lines(at) => at,
            };
            input.settle();
            return TextAction::None;
        }
        if g || count.is_some() && matches!(key, NKey::Esc) {
            return TextAction::None;
        }
        let at = input.cursor;
        let end = input.line_end(at);
        match key {
            NKey::Char(op @ ('d' | 'c' | 'y')) => {
                self.op = Some((op, n));
                TextAction::None
            }
            NKey::Char('x') => input.operate('d', Motion::To(input.chars_right(at, n))),
            NKey::Char('X') => input.operate('d', Motion::To(input.chars_left(at, n))),
            NKey::Char('D') => input.operate('d', Motion::To(end)),
            NKey::Char('C') => input.operate('c', Motion::To(end)),
            NKey::Char('s') => input.operate('c', Motion::To(input.chars_right(at, n))),
            NKey::Char('S') => input.operate('c', Motion::Lines(at)),
            NKey::Char('Y') => input.operate('y', Motion::Lines(at)),
            NKey::Char('i') => {
                input.checkpoint();
                TextAction::Insert
            }
            NKey::Char('a') => {
                input.checkpoint();
                input.cursor = input.chars_right(at, 1);
                TextAction::Insert
            }
            NKey::Char('I') => {
                input.checkpoint();
                input.cursor = input.first_letter(at);
                TextAction::Insert
            }
            NKey::Char('A') => {
                input.checkpoint();
                input.cursor = end;
                TextAction::Insert
            }
            NKey::Char('o') => {
                input.checkpoint();
                input.cursor = end;
                input.insert("\n");
                TextAction::Insert
            }
            NKey::Char('O') => {
                input.checkpoint();
                input.cursor = input.line_start(at);
                input.insert("\n");
                input.cursor -= 1;
                TextAction::Insert
            }
            NKey::Char('p') => {
                input.checkpoint();
                input.cursor = input.chars_right(at, 1);
                TextAction::Paste
            }
            NKey::Char('P') => {
                input.checkpoint();
                TextAction::Paste
            }
            NKey::Char('u') => {
                for _ in 0..n {
                    input.undo();
                }
                TextAction::None
            }
            NKey::Ctrl('r') => {
                for _ in 0..n {
                    input.redo();
                }
                TextAction::None
            }
            NKey::Char('~') => {
                let to = input.chars_right(at, n);
                if to > at {
                    input.checkpoint();
                    let swapped: String = input.text[at..to]
                        .chars()
                        .flat_map(|c| -> Vec<char> {
                            if c.is_uppercase() { c.to_lowercase().collect() } else { c.to_uppercase().collect() }
                        })
                        .collect();
                    input.text.replace_range(at..to, &swapped);
                    input.cursor = at + swapped.len();
                    input.settle();
                }
                TextAction::None
            }
            NKey::Esc => TextAction::Leave,
            NKey::Enter => TextAction::Submit,
            _ => TextAction::None,
        }
    }

    /// Keys typed so far, for the status line.
    pub fn display(&self) -> String {
        let mut s = self.op.map(|(op, n)| if n > 1 { format!("{n}{op}") } else { op.to_string() }).unwrap_or_default();
        s.extend(self.count.map(|n| n.to_string()));
        if self.g {
            s.push('g');
        }
        s
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

impl Input {
    /// Remember the text before a change, for `u`.
    pub fn checkpoint(&mut self) {
        if self.undo.last().is_none_or(|(text, _)| *text != self.text) {
            self.undo.push((self.text.clone(), self.cursor));
            if self.undo.len() > UNDO_MAX {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        // A checkpoint taken just before nothing changed is no step back.
        while let Some((text, cursor)) = self.undo.pop() {
            if text != self.text {
                self.redo.push((std::mem::replace(&mut self.text, text), self.cursor));
                self.cursor = cursor;
                self.settle();
                return;
            }
        }
    }

    fn redo(&mut self) {
        if let Some((text, cursor)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.text, text), self.cursor));
            self.cursor = cursor.min(self.text.len());
            while !self.text.is_char_boundary(self.cursor) {
                self.cursor -= 1;
            }
            self.settle();
        }
    }

    /// Leaving Insert: the cursor goes onto the char before it, like Vim.
    pub fn leave_insert(&mut self) {
        if self.cursor > self.line_start(self.cursor) {
            self.left();
        }
    }

    /// In Text mode the cursor is on a char: not past the end of a line
    /// that has some.
    pub fn settle(&mut self) {
        if self.cursor > self.line_start(self.cursor) && self.cursor == self.line_end(self.cursor) {
            self.left();
        }
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..].find('\n').map_or(self.text.len(), |i| at + i)
    }

    fn first_letter(&self, at: usize) -> usize {
        let start = self.line_start(at);
        let line = &self.text[start..self.line_end(at)];
        start + line.find(|c: char| !c.is_whitespace()).unwrap_or(line.len())
    }

    /// `n` chars right, not past the end of the line.
    fn chars_right(&self, at: usize, n: usize) -> usize {
        let line = &self.text[at..self.line_end(at)];
        at + line.char_indices().nth(n).map_or(line.len(), |(i, _)| i)
    }

    /// `n` chars left, not before the start of the line.
    fn chars_left(&self, at: usize, n: usize) -> usize {
        let start = self.line_start(at);
        self.text[start..at].char_indices().rev().nth(n - 1).map_or(start, |(i, _)| start + i)
    }

    /// Same column `n` lines down, or as far as there are lines.
    fn line_down(&self, at: usize, n: usize) -> usize {
        let column = self.text[self.line_start(at)..at].chars().count();
        let mut line = self.line_start(at);
        for _ in 0..n {
            let end = self.line_end(line);
            if end == self.text.len() {
                break;
            }
            line = end + 1;
        }
        self.column(line, column)
    }

    fn line_up(&self, at: usize, n: usize) -> usize {
        let column = self.text[self.line_start(at)..at].chars().count();
        let mut line = self.line_start(at);
        for _ in 0..n {
            if line == 0 {
                break;
            }
            line = self.line_start(line - 1);
        }
        self.column(line, column)
    }

    fn column(&self, line: usize, column: usize) -> usize {
        let text = &self.text[line..self.line_end(line)];
        line + text.char_indices().nth(column).map_or(text.len(), |(i, _)| i)
    }

    /// Start of the next word: past this word, then past blanks.
    fn word_forward(&self, at: usize, big: bool) -> usize {
        let mut chars = self.text[at..].char_indices().map(|(i, c)| (at + i, class(c, big))).peekable();
        let Some(&(_, first)) = chars.peek() else {
            return at;
        };
        let mut chars = chars.skip_while(|&(_, k)| first != 0 && k == first).skip_while(|&(_, k)| k == 0);
        chars.next().map_or(self.text.len(), |(i, _)| i)
    }

    /// Last char of this or the next word.
    fn word_end(&self, at: usize, big: bool) -> usize {
        let chars: Vec<(usize, u8)> = self.text[at..].char_indices().map(|(i, c)| (at + i, class(c, big))).collect();
        let mut i = 1;
        while i < chars.len() && chars[i].1 == 0 {
            i += 1;
        }
        if i >= chars.len() {
            return chars.last().map_or(at, |c| c.0);
        }
        while i + 1 < chars.len() && chars[i + 1].1 == chars[i].1 {
            i += 1;
        }
        chars[i].0
    }

    /// Start of this word, or of the one before.
    fn word_back(&self, at: usize, big: bool) -> usize {
        let chars: Vec<(usize, u8)> = self.text[..at].char_indices().map(|(i, c)| (i, class(c, big))).collect();
        let mut i = chars.len();
        while i > 0 && chars[i - 1].1 == 0 {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let kind = chars[i - 1].1;
        while i > 0 && chars[i - 1].1 == kind {
            i -= 1;
        }
        chars.get(i).map_or(0, |c| c.0)
    }

    fn motion(&self, key: NKey, n: usize, g: bool, op: bool) -> Option<Motion> {
        let at = self.cursor;
        let repeat = |f: &dyn Fn(usize) -> usize| (0..n).fold(at, |at, _| f(at));
        if g {
            return match key {
                NKey::Char('g') => Some(Motion::Lines(0)),
                _ => None,
            };
        }
        Some(match key {
            NKey::Char('h') | NKey::Left => Motion::To(self.chars_left(at, n)),
            NKey::Char('l') | NKey::Right | NKey::Space => Motion::To(self.chars_right(at, n)),
            NKey::Char('0') => Motion::To(self.line_start(at)),
            NKey::Char('^') => Motion::To(self.first_letter(at)),
            NKey::Char('$') => Motion::To(self.line_end(at)),
            NKey::Char(c @ ('w' | 'W')) => {
                let to = repeat(&|at| self.word_forward(at, c == 'W'));
                // `dw` on the last word of a line keeps the line break.
                Motion::To(if op { to.min(self.line_end(at)).max(at.min(to)) } else { to })
            }
            NKey::Char(c @ ('b' | 'B')) => Motion::To(repeat(&|at| self.word_back(at, c == 'B'))),
            NKey::Char(c @ ('e' | 'E')) => Motion::Through(repeat(&|at| self.word_end(at, c == 'E'))),
            NKey::Char('j') | NKey::Down => Motion::Lines(self.line_down(at, n)),
            NKey::Char('k') | NKey::Up => Motion::Lines(self.line_up(at, n)),
            NKey::Char('G') => Motion::Lines(self.line_start(self.text.len())),
            _ => return None,
        })
        .map(|m| match m {
            // Moving, `j` and `k` keep the column instead of taking lines.
            Motion::Lines(to) if !op && !matches!(key, NKey::Char('G')) => Motion::To(to),
            Motion::Lines(_) if !op => Motion::To(self.first_letter(self.line_start(self.text.len()))),
            m => m,
        })
    }

    /// `d`, `c` or `y` over what the motion covers.
    fn operate(&mut self, op: char, motion: Motion) -> TextAction {
        let at = self.cursor;
        let (mut start, mut end) = match motion {
            Motion::To(to) => (at.min(to), at.max(to)),
            Motion::Through(to) => {
                let (a, b) = (at.min(to), at.max(to));
                (a, b + self.char_at(b).map_or(0, char::len_utf8))
            }
            Motion::Lines(to) => (self.line_start(at.min(to)), self.line_end(at.max(to))),
        };
        let lines = matches!(motion, Motion::Lines(_));
        if op == 'y' {
            if start == end {
                return TextAction::None;
            }
            if !lines {
                self.cursor = start;
            }
            return TextAction::Yank(self.text[start..end].to_owned());
        }
        if lines && op == 'd' {
            // The line break goes too: the one after, or for the last line
            // the one before.
            if end < self.text.len() {
                end += 1;
            } else {
                start = start.saturating_sub(1);
            }
        }
        self.checkpoint();
        self.text.replace_range(start..end, "");
        self.cursor = start;
        if op == 'c' {
            return TextAction::Insert;
        }
        if lines {
            // Onto the line that took the deleted ones' place.
            self.cursor = self.first_letter(self.line_start(start));
        }
        self.settle();
        TextAction::None
    }
}

/// Command-line history, like Vim's: Up/Down recall entries that start
/// with what has been typed so far.
#[derive(Debug, Default)]
pub struct History {
    entries: Vec<String>,
    /// Index of the entry being shown while browsing.
    pos: Option<usize>,
    /// What was typed before browsing started; also the search prefix.
    draft: String,
}

impl History {
    const MAX: usize = 500;

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn push(&mut self, entry: &str) {
        let entry = entry.trim();
        self.reset();
        if entry.is_empty() {
            return;
        }
        self.entries.retain(|e| e != entry);
        self.entries.push(entry.to_owned());
        if self.entries.len() > Self::MAX {
            self.entries.remove(0);
        }
    }

    pub fn reset(&mut self) {
        self.pos = None;
        self.draft.clear();
    }

    /// Older entry matching the prefix typed before browsing.
    pub fn prev(&mut self, current: &str) -> Option<&str> {
        if self.pos.is_none() {
            self.draft = current.to_owned();
        }
        let end = self.pos.unwrap_or(self.entries.len());
        let found = self.entries[..end].iter().rposition(|e| e.starts_with(&self.draft))?;
        self.pos = Some(found);
        Some(&self.entries[found])
    }

    /// Newer entry, or back to the draft past the newest one.
    pub fn next(&mut self) -> Option<&str> {
        let pos = self.pos?;
        match self.entries[pos + 1..].iter().position(|e| e.starts_with(&self.draft)) {
            Some(offset) => {
                self.pos = Some(pos + 1 + offset);
                Some(&self.entries[pos + 1 + offset])
            }
            None => {
                self.pos = None;
                Some(&self.draft)
            }
        }
    }
}

impl Input {
    /// Replace the contents, cursor at the end.
    pub fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = self.text.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(keys: &str) -> Vec<Action> {
        let mut p = Pending::default();
        keys.chars().filter_map(|c| p.feed(NKey::Char(c))).collect()
    }

    #[test]
    fn counts_and_prefixes() {
        assert_eq!(feed("12j"), vec![Action::Down(12)]);
        assert_eq!(feed("gg"), vec![Action::Top]);
        assert_eq!(feed("yy"), vec![Action::Yank]);
        assert_eq!(feed("gzj"), vec![Action::Down(1)]);
        assert_eq!(feed("0j"), vec![Action::Down(1)]);
        assert_eq!(feed("10k"), vec![Action::Up(10)]);
        assert_eq!(feed("gt"), vec![Action::NextFolder]);
        assert_eq!(feed("3gt"), vec![Action::Folder(2)]);
        assert_eq!(feed("gTJK"), vec![Action::PrevFolder, Action::NextFolder, Action::PrevFolder]);
        assert_eq!(feed("cce"), vec![Action::Edit, Action::Edit]);
        assert_eq!(feed("ddnN"), vec![Action::Delete, Action::SearchNext, Action::SearchPrev]);
        assert_eq!(feed("gagpgP"), vec![Action::Attach, Action::Pinned, Action::PinnedList]);
        assert_eq!(feed("ge"), vec![Action::Emoji]);
        assert_eq!(feed("gv"), vec![Action::JumpPlaying]);
        assert_eq!(feed("gugk"), vec![Action::FirstUnread { channel: false }, Action::FirstUnread { channel: true }]);
        assert_eq!(feed("rR"), vec![Action::Reply, Action::React]);
        assert_eq!(feed("gf"), vec![Action::GoToSource]);
        assert_eq!(feed("gccc"), vec![Action::Comments, Action::Edit]);
        assert_eq!(feed("Vjf"), vec![Action::Visual, Action::Down(1), Action::Forward]);
        assert_eq!(feed("vjvf"), vec![Action::Mark, Action::Down(1), Action::Mark, Action::Forward]);
        assert_eq!(feed("gx2gx"), vec![Action::OpenLink(1), Action::OpenLink(2)]);
        let mut p = Pending::default();
        assert_eq!(p.feed(NKey::Ctrl('h')), Some(Action::FocusChats));
        assert_eq!(p.feed(NKey::Ctrl('l')), Some(Action::FocusMessages));
        assert_eq!(p.feed(NKey::Ctrl('o')), Some(Action::JumpBack));
        assert_eq!(p.feed(NKey::Ctrl('i')), Some(Action::JumpForward));
        assert_eq!(feed("][="), vec![Action::Faster, Action::Slower, Action::NormalSpeed]);
        assert_eq!(feed("}{"), vec![Action::Louder, Action::Quieter]);
        assert_eq!(feed("mU"), vec![Action::Mute, Action::Leave]);
        assert_eq!(feed("iIaA"), vec![Action::Insert, Action::InsertStart, Action::Append, Action::Append]);
    }

    /// Types `keys` in Text mode over `text` with the cursor at `|`.
    fn text(text: &str, keys: &str) -> (String, Vec<TextAction>) {
        let at = text.find('|').unwrap();
        let mut input = Input::default();
        input.insert(&text.replace('|', ""));
        input.cursor = at;
        let mut p = TextPending::default();
        let actions = keys
            .chars()
            .map(|c| {
                let key = match c {
                    '\u{1b}' => NKey::Esc,
                    '\u{12}' => NKey::Ctrl('r'),
                    c => NKey::Char(c),
                };
                p.feed(key, &mut input)
            })
            .filter(|a| *a != TextAction::None)
            .collect();
        let (before, after) = input.split();
        (format!("{before}|{after}"), actions)
    }

    #[test]
    fn text_mode_motions() {
        assert_eq!(text("|hello big world", "w").0, "hello |big world");
        assert_eq!(text("|hello big world", "2w").0, "hello big |world");
        assert_eq!(text("|hello big world", "e").0, "hell|o big world");
        assert_eq!(text("hello big wo|rld", "b").0, "hello big |world");
        assert_eq!(text("hello big wo|rld", "bb").0, "hello |big world");
        assert_eq!(text("|foo.bar baz", "w").0, "foo|.bar baz");
        assert_eq!(text("|foo.bar baz", "W").0, "foo.bar |baz");
        assert_eq!(text("he|llo", "$").0, "hell|o");
        assert_eq!(text("  he|llo", "0").0, "|  hello");
        assert_eq!(text("  he|llo", "^").0, "  |hello");
        assert_eq!(text("|hello", "lll").0, "hel|lo");
        assert_eq!(text("|hello", "9l").0, "hell|o");
        assert_eq!(text("ab|cd\nxy", "j").0, "abcd\nx|y");
        assert_eq!(text("abcd\nx|y", "k").0, "a|bcd\nxy");
        assert_eq!(text("abcd\nx|y", "gg").0, "|abcd\nxy");
        assert_eq!(text("a|bcd\nxy", "G").0, "abcd\n|xy");
        assert_eq!(text("привет мир|", "b").0, "привет |мир");
    }

    #[test]
    fn text_mode_operators() {
        assert_eq!(text("|hello big world", "dw").0, "|big world");
        assert_eq!(text("|hello big world", "d2w").0, "|world");
        assert_eq!(text("|hello big world", "2dw").0, "|world");
        assert_eq!(text("hello |big world", "cw"), ("hello | world".into(), vec![TextAction::Insert]));
        assert_eq!(text("hello |big world", "de").0, "hello | world");
        assert_eq!(text("hello big |world", "dw").0, "hello big| ");
        assert_eq!(text("hello |big world", "d$").0, "hello| ");
        assert_eq!(text("hello |big world", "D").0, "hello| ");
        assert_eq!(text("hello |big world", "db").0, "|big world");
        assert_eq!(text("|hello", "x").0, "|ello");
        assert_eq!(text("|hello", "3x").0, "|lo");
        assert_eq!(text("hel|lo", "X").0, "he|lo");
        assert_eq!(text("hell|o", "x").0, "hel|l");
        assert_eq!(text("one\nt|wo\nthree", "dd").0, "one\n|three");
        assert_eq!(text("one\ntwo\nthr|ee", "dd").0, "one\n|two");
        assert_eq!(text("o|ne\ntwo\nthree", "2dd").0, "|three");
        assert_eq!(text("o|ne\ntwo", "dj").0, "|");
        assert_eq!(text("one\nt|wo", "cc"), ("one\n|".into(), vec![TextAction::Insert]));
        assert_eq!(text("hello |big world", "yw"), ("hello |big world".into(), vec![TextAction::Yank("big ".into())]));
        assert_eq!(text("a|b\ncd", "yy").1, vec![TextAction::Yank("ab".into())]);
        assert_eq!(text("|ab", "~~").0, "A|B");
        assert_eq!(text("|ab", "~").0, "A|b");
        assert_eq!(text("hello |big world", "\u{1b}").1, vec![TextAction::Leave]);
        assert_eq!(text("hello |big world", "d\u{1b}x").0, "hello |ig world");
    }

    #[test]
    fn text_mode_insert_and_undo() {
        assert_eq!(text("he|llo", "a").0, "hel|lo");
        assert_eq!(text("he|llo", "A").0, "hello|");
        assert_eq!(text("  he|llo", "I").0, "  |hello");
        assert_eq!(text("a|b\ncd", "o").0, "ab\n|\ncd");
        assert_eq!(text("ab\nc|d", "O").0, "ab\n|\ncd");
        assert_eq!(text("|hello big world", "dwdwu").0, "|big world");
        assert_eq!(text("|hello big world", "dwdwuu").0, "|hello big world");
        assert_eq!(text("|hello big world", "dwdwuu\u{12}").0, "|big world");
        assert_eq!(text("|hello big world", "dwu2\u{12}").0, "|big world");
    }

    #[test]
    fn history_browses_with_prefix() {
        let mut h = History::default();
        for e in ["привет", "pin", "погода", "", "pin"] {
            h.push(e);
        }
        assert_eq!(h.entries(), ["привет", "погода", "pin"]);
        assert_eq!(h.prev(""), Some("pin"));
        assert_eq!(h.prev(""), Some("погода"));
        assert_eq!(h.prev(""), Some("привет"));
        assert_eq!(h.prev(""), None);
        assert_eq!(h.next(), Some("погода"));
        assert_eq!(h.next(), Some("pin"));
        assert_eq!(h.next(), Some(""));
        h.reset();
        assert_eq!(h.prev("п"), Some("погода"));
        assert_eq!(h.prev("п"), Some("привет"));
        assert_eq!(h.next(), Some("погода"));
        assert_eq!(h.next(), Some("п"));
    }

    #[test]
    fn normalizes_layouts() {
        use iced::keyboard::key::{Code, Physical};
        let key = |s: &str| Key::Character(s.into());
        let none = Modifiers::empty();
        // Cyrillic о is on the J key.
        assert_eq!(normalize(&key("о"), Physical::Code(Code::KeyJ), none), Some(NKey::Char('j')));
        assert_eq!(normalize(&key("П"), Physical::Code(Code::KeyG), Modifiers::SHIFT), Some(NKey::Char('G')));
        assert_eq!(normalize(&key("."), Physical::Code(Code::Slash), none), Some(NKey::Char('/')));
        assert_eq!(normalize(&key("Ж"), Physical::Code(Code::Semicolon), Modifiers::SHIFT), Some(NKey::Char(':')));
        assert_eq!(normalize(&key("х"), Physical::Code(Code::BracketLeft), none), Some(NKey::Char('[')));
        assert_eq!(normalize(&key("2"), Physical::Code(Code::Digit2), Modifiers::ALT), Some(NKey::Alt('2')));
        assert_eq!(normalize(&key("["), Physical::Code(Code::BracketLeft), Modifiers::CTRL), Some(NKey::Esc));
        assert_eq!(normalize(&Key::Named(Named::Space), Physical::Code(Code::Space), none), Some(NKey::Space));
    }

    #[test]
    fn input_editing_unicode() {
        let mut i = Input::default();
        i.insert("привет мир");
        i.backspace();
        assert_eq!(i.as_str(), "привет ми");
        i.delete_word();
        assert_eq!(i.as_str(), "привет ");
        i.left();
        i.left();
        i.insert("!");
        assert_eq!(i.as_str(), "приве!т ");
        i.delete_to_start();
        assert_eq!(i.as_str(), "т ");
    }
}
