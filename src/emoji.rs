//! Emoji picker: tabs of recent emoji and Unicode groups, a grid moved
//! with h/j/k/l, and `/` to search by name.

use emojis::{Emoji, Group, SkinTone};

pub const COLS: usize = 8;
const RECENT_MAX: usize = 32;

/// Tab icons: recent first, then one per Unicode group.
pub const TABS: [(&str, Option<Group>); 10] = [
    ("🕓", None),
    ("😀", Some(Group::SmileysAndEmotion)),
    ("👋", Some(Group::PeopleAndBody)),
    ("🐻", Some(Group::AnimalsAndNature)),
    ("🍔", Some(Group::FoodAndDrink)),
    ("🚗", Some(Group::TravelAndPlaces)),
    ("⚽", Some(Group::Activities)),
    ("💡", Some(Group::Objects)),
    ("🔣", Some(Group::Symbols)),
    ("🏁", Some(Group::Flags)),
];

fn matches(e: &Emoji, query: &str) -> bool {
    e.name().contains(query) || e.shortcodes().any(|c| c.contains(query))
}

fn base_tone(e: &&'static Emoji) -> bool {
    matches!(e.skin_tone(), None | Some(SkinTone::Default))
}

#[derive(Debug, Clone, Default)]
pub struct Picker {
    pub tab: usize,
    pub cursor: usize,
    /// First grid row on screen.
    pub top: usize,
    /// Typing a search query (after `/`).
    pub searching: bool,
    pub query: String,
    /// Picking a reaction to this message instead of an emoji to type.
    pub react: Option<React>,
}

#[derive(Debug, Clone)]
pub struct React {
    pub chat_id: i64,
    pub msg_id: i64,
    /// Emoji allowed as reactions there, in Telegram's order.
    pub choices: Vec<String>,
}

impl Picker {
    pub fn new(recent: &[String]) -> Self {
        Picker {
            tab: if recent.is_empty() { 1 } else { 0 },
            ..Default::default()
        }
    }

    pub fn reactions(react: React) -> Self {
        Picker {
            react: Some(react),
            ..Default::default()
        }
    }

    /// Emoji shown: search results, the current tab or the reactions.
    pub fn items<'a>(&'a self, recent: &'a [String]) -> Vec<&'a str> {
        let query = self.query.trim().to_lowercase();
        if let Some(react) = &self.react {
            return react
                .choices
                .iter()
                .map(String::as_str)
                .filter(|c| query.is_empty() || emojis::get(c).is_some_and(|e| matches(e, &query)))
                .collect();
        }
        if !query.is_empty() {
            return emojis::iter()
                .filter(|e| matches(e, &query))
                .map(Emoji::as_str)
                .collect();
        }
        match TABS[self.tab].1 {
            None => recent.iter().filter_map(|r| emojis::get(r)).map(Emoji::as_str).collect(),
            Some(group) => group.emojis().filter(base_tone).map(Emoji::as_str).collect(),
        }
    }

    pub fn set_tab(&mut self, tab: usize) {
        self.tab = tab % TABS.len();
        self.cursor = 0;
        self.top = 0;
        self.query.clear();
        self.searching = false;
    }

    pub fn next_tab(&mut self, step: isize) {
        let n = TABS.len() as isize;
        self.set_tab(((self.tab as isize + step).rem_euclid(n)) as usize);
    }

    /// Move the cursor by `dx` cells and `dy` rows, keeping it on the grid
    /// and its row on screen.
    pub fn step(&mut self, dx: isize, dy: isize, count: usize, rows: usize) {
        if count == 0 {
            self.cursor = 0;
            return;
        }
        let target = self.cursor as isize + dx + dy * COLS as isize;
        self.cursor = target.clamp(0, count as isize - 1) as usize;
        self.scroll_to_cursor(rows);
    }

    pub fn scroll_to_cursor(&mut self, rows: usize) {
        let row = self.cursor / COLS;
        let rows = rows.max(1);
        if row < self.top {
            self.top = row;
        } else if row >= self.top + rows {
            self.top = row + 1 - rows;
        }
    }

    /// Wheel: move the view by whole rows.
    pub fn scroll(&mut self, rows_delta: isize, count: usize, rows: usize) {
        let total = count.div_ceil(COLS);
        let max_top = total.saturating_sub(rows.max(1));
        self.top = (self.top as isize + rows_delta).clamp(0, max_top as isize) as usize;
    }
}

/// Put an emoji first in the recent list.
pub fn remember(recent: &mut Vec<String>, emoji: &str) {
    recent.retain(|r| r != emoji);
    recent.insert(0, emoji.to_owned());
    recent.truncate(RECENT_MAX);
}

pub fn name(emoji: &str) -> &'static str {
    emojis::get(emoji).map_or("", Emoji::name)
}

fn recent_path() -> std::path::PathBuf {
    crate::config::dirs().data_dir().join("recent_emoji")
}

pub fn load_recent() -> Vec<String> {
    std::fs::read_to_string(recent_path())
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

pub fn save_recent(recent: &[String]) {
    let path = recent_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, recent.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_and_search() {
        let mut picker = Picker::new(&[]);
        assert_eq!(picker.tab, 1, "no recent emoji yet: smileys");
        let smileys = picker.items(&[]);
        assert_eq!(smileys[0], "😀");
        assert!(!smileys.iter().any(|e| e.contains('\u{1F3FB}')), "no skin tone variants");
        picker.query = "thumbs up".into();
        assert!(picker.items(&[]).contains(&"👍"));
        picker.query = "fire".into();
        assert!(picker.items(&[]).contains(&"🔥"));
        picker.set_tab(0);
        assert_eq!(picker.items(&["🔥".into(), "😀".into()]), ["🔥", "😀"]);
        picker.next_tab(-1);
        assert_eq!(picker.tab, TABS.len() - 1);
    }

    #[test]
    fn reactions_are_searchable() {
        let choices = vec!["👍".to_owned(), "❤".to_owned(), "🔥".to_owned()];
        let mut picker = Picker::reactions(React { chat_id: 1, msg_id: 2, choices });
        assert_eq!(picker.items(&[]), ["👍", "❤", "🔥"], "kept as Telegram spells them");
        picker.query = "fire".into();
        assert_eq!(picker.items(&[]), ["🔥"]);
    }

    #[test]
    fn grid_movement() {
        let mut p = Picker::default();
        p.step(1, 0, 20, 2);
        assert_eq!(p.cursor, 1);
        p.step(0, 1, 20, 2);
        assert_eq!(p.cursor, 9);
        p.step(0, 1, 20, 2);
        assert_eq!(p.cursor, 17);
        assert_eq!(p.top, 1, "the cursor row stays on screen");
        p.step(0, 5, 20, 2);
        assert_eq!(p.cursor, 19, "clamped to the last emoji");
        p.step(-100, 0, 20, 2);
        assert_eq!((p.cursor, p.top), (0, 0));
    }

    #[test]
    fn recent_is_most_recent_first() {
        let mut recent = vec!["😀".to_owned(), "🔥".to_owned()];
        remember(&mut recent, "🔥");
        assert_eq!(recent, ["🔥", "😀"]);
    }
}
