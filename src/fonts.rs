//! Emoji rendering.
//!
//! iced draws text with cosmic-text + swash, which renders bitmap (CBDT)
//! and COLRv0 color fonts but not COLRv1. Fedora and others now ship Noto
//! Color Emoji only as COLRv1, so every emoji came out as blank space. Faces
//! that can't be drawn are dropped from the font database and a drawable
//! emoji font is loaded instead.

use std::path::PathBuf;

use iced::advanced::graphics::text::cosmic_text::fontdb;

/// Where `scripts/fetch-emoji.sh` puts Noto Color Emoji (CBDT).
pub fn emoji_font_path() -> PathBuf {
    crate::config::dirs().data_dir().join("fonts/NotoColorEmoji.ttf")
}

/// The copy shipped with a packaged build: `fonts\` next to `bin\`.
fn bundled_font_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.parent()?.join("fonts/NotoColorEmoji.ttf"))
}

/// Fix emoji in iced's global font system. Call before the first frame.
pub fn setup() -> Result<(), String> {
    let path = emoji_font_path();
    let bytes = std::fs::read(&path)
        .or_else(|e| bundled_font_path().and_then(|p| std::fs::read(p).ok()).ok_or(e))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut fonts = iced::advanced::graphics::text::font_system()
        .write()
        .map_err(|_| "font system poisoned".to_string())?;
    remove_undrawable(fonts.raw().db_mut());
    fonts.load_font(bytes.into());
    Ok(())
}

/// Remove color fonts that only have COLRv1 glyphs (no outlines, no
/// bitmaps, no COLRv0 layers): cosmic-text picks them for emoji and then
/// draws nothing.
pub fn remove_undrawable(db: &mut fontdb::Database) -> usize {
    let ids: Vec<fontdb::ID> = db
        .faces()
        .filter(|face| face.families.iter().any(|(name, _)| name.contains("Emoji")))
        .map(|face| face.id)
        .collect();
    let mut removed = 0;
    for id in ids {
        let undrawable = db
            .with_face_data(id, colrv1_only)
            .unwrap_or(false);
        if undrawable {
            db.remove_face(id);
            removed += 1;
        }
    }
    removed
}

fn colrv1_only(data: &[u8], index: u32) -> bool {
    let Some(tables) = tables(data, index) else {
        return false;
    };
    let find = |tag: &[u8; 4]| tables.iter().find(|(t, _)| t == tag).map(|(_, offset)| *offset);
    if find(b"CBDT").is_some() || find(b"sbix").is_some() {
        return false;
    }
    match find(b"COLR") {
        Some(offset) => read_u16(data, offset) == Some(1),
        None => false,
    }
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// Table tags and offsets of an OpenType font (or one face of a collection).
fn tables(data: &[u8], index: u32) -> Option<Vec<([u8; 4], usize)>> {
    let start = if data.get(..4)? == b"ttcf" {
        read_u32(data, 12 + 4 * index as usize)? as usize
    } else {
        0
    };
    let count = read_u16(data, start + 4)? as usize;
    (0..count)
        .map(|i| {
            let record = start + 12 + 16 * i;
            let tag: [u8; 4] = data.get(record..record + 4)?.try_into().ok()?;
            Some((tag, read_u32(data, record + 8)? as usize))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::graphics::text::cosmic_text::{
        Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
    };

    #[test]
    fn reads_table_directory() {
        // Minimal font header with one COLR table (version 1) and no bitmaps.
        let mut font = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        font.extend_from_slice(b"COLR");
        font.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 28, 0, 0, 0, 2]);
        font.extend_from_slice(&[0, 1]);
        assert!(colrv1_only(&font, 0));
        font[28..30].copy_from_slice(&[0, 0]);
        assert!(!colrv1_only(&font, 0));
    }

    /// With the fix, an emoji rasterizes to a color bitmap instead of nothing.
    #[test]
    fn emoji_render_in_color() {
        let path = emoji_font_path();
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipped: {} missing (scripts/fetch-emoji.sh)", path.display());
            return;
        };
        let mut fonts = FontSystem::new();
        remove_undrawable(fonts.db_mut());
        fonts.db_mut().load_font_data(bytes);

        let mut buffer = Buffer::new(&mut fonts, Metrics::new(32.0, 40.0));
        buffer.set_text(&mut fonts, "🎤", &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut fonts, false);
        let mut cache = SwashCache::new();
        let glyph = buffer.layout_runs().next().unwrap().glyphs[0].physical((0.0, 0.0), 1.0);
        let image = cache.get_image_uncached(&mut fonts, glyph.cache_key).expect("emoji glyph");
        assert_eq!(image.content, SwashContent::Color);
        assert!(image.placement.width > 0);
    }
}
