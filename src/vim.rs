//! Vim-style modal input: mode, Normal-mode key parsing and a line editor.

use iced::keyboard::key::{Code, Named, Physical};
use iced::keyboard::{Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
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
    SeekBack,
    SeekForward,
    Reply,
    Edit,
    Forward,
    Delete,
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
            (None, NKey::Char('h') | NKey::Left) => Some(Action::FocusChats),
            (None, NKey::Char('l') | NKey::Right) => Some(Action::FocusMessages),
            (None, NKey::Tab) => Some(Action::ToggleFocus),
            (None, NKey::Enter | NKey::Char('o')) => Some(Action::Open),
            (None, NKey::Char('i' | 'a')) => Some(Action::Insert),
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
            (None, NKey::Char('H')) => Some(Action::SeekBack),
            (None, NKey::Char('L')) => Some(Action::SeekForward),
            (None, NKey::Char('r')) => Some(Action::Reply),
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
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.take();
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
        assert_eq!(feed("gxj"), vec![Action::Down(1)]);
        assert_eq!(feed("0j"), vec![Action::Down(1)]);
        assert_eq!(feed("10k"), vec![Action::Up(10)]);
        assert_eq!(feed("gt"), vec![Action::NextFolder]);
        assert_eq!(feed("3gt"), vec![Action::Folder(2)]);
        assert_eq!(feed("gTJK"), vec![Action::PrevFolder, Action::NextFolder, Action::PrevFolder]);
        assert_eq!(feed("cce"), vec![Action::Edit, Action::Edit]);
        assert_eq!(feed("ddnN"), vec![Action::Delete, Action::SearchNext, Action::SearchPrev]);
        assert_eq!(feed("][="), vec![Action::Faster, Action::Slower, Action::NormalSpeed]);
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
