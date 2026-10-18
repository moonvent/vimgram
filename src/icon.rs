//! The app icon (window, tray, notifications), drawn in code so no theme
//! or asset file is needed, and avatars prepared for notifications.

use std::path::{Path, PathBuf};

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

const BLUE_TOP: [u8; 3] = [0x37, 0xbb, 0xfe];
const BLUE_BOTTOM: [u8; 3] = [0x00, 0x7d, 0xbb];
const BADGE: [u8; 3] = [0xe5, 0x39, 0x35];

fn rgb([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgba8(r, g, b, 0xff)
}

/// Paper plane outline in a unit square.
const PLANE: [(f32, f32); 6] = [
    (0.215, 0.495),
    (0.745, 0.285),
    (0.655, 0.725),
    (0.475, 0.605),
    (0.405, 0.695),
    (0.385, 0.560),
];

fn circle(cx: f32, cy: f32, r: f32) -> tiny_skia::Path {
    PathBuilder::from_circle(cx, cy, r).expect("valid circle")
}

/// The icon at `size` px; `unread` adds a red dot.
pub fn render(size: u32, unread: bool) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).expect("non-zero icon size");
    let s = size as f32;

    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.shader = tiny_skia::LinearGradient::new(
        tiny_skia::Point::from_xy(0.0, 0.0),
        tiny_skia::Point::from_xy(0.0, s),
        vec![
            tiny_skia::GradientStop::new(0.0, rgb(BLUE_TOP)),
            tiny_skia::GradientStop::new(1.0, rgb(BLUE_BOTTOM)),
        ],
        tiny_skia::SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("valid gradient");
    pixmap.fill_path(&circle(s / 2.0, s / 2.0, s / 2.0), &paint, FillRule::Winding, Transform::identity(), None);

    let mut plane = PathBuilder::new();
    for (i, (x, y)) in PLANE.iter().enumerate() {
        if i == 0 {
            plane.move_to(x * s, y * s);
        } else {
            plane.line_to(x * s, y * s);
        }
    }
    plane.close();
    let mut white = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    white.set_color(Color::WHITE);
    if let Some(path) = plane.finish() {
        pixmap.fill_path(&path, &white, FillRule::Winding, Transform::identity(), None);
    }

    if unread {
        let r = s * 0.2;
        let (cx, cy) = (s - r, r);
        pixmap.fill_path(&circle(cx, cy, r + s * 0.04), &white, FillRule::Winding, Transform::identity(), None);
        let mut red = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        red.set_color(rgb(BADGE));
        pixmap.fill_path(&circle(cx, cy, r), &red, FillRule::Winding, Transform::identity(), None);
    }
    pixmap
}

/// Tray icon: just the white plane on transparent, filling the square,
/// with a red dot for unread chats.
pub fn render_tray(size: u32, unread: bool) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).expect("non-zero icon size");
    let s = size as f32;
    let (min_x, max_x) = PLANE.iter().fold((f32::MAX, f32::MIN), |(lo, hi), (x, _)| (lo.min(*x), hi.max(*x)));
    let (min_y, max_y) = PLANE.iter().fold((f32::MAX, f32::MIN), |(lo, hi), (_, y)| (lo.min(*y), hi.max(*y)));
    let margin = s * 0.06;
    let scale = (s - 2.0 * margin) / (max_x - min_x).max(max_y - min_y);
    let (dx, dy) = (
        (s - (max_x - min_x) * scale) / 2.0 - min_x * scale,
        (s - (max_y - min_y) * scale) / 2.0 - min_y * scale,
    );
    let mut plane = PathBuilder::new();
    for (i, (x, y)) in PLANE.iter().enumerate() {
        let (x, y) = (x * scale + dx, y * scale + dy);
        if i == 0 {
            plane.move_to(x, y);
        } else {
            plane.line_to(x, y);
        }
    }
    plane.close();
    let mut white = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    white.set_color(Color::WHITE);
    if let Some(path) = plane.finish() {
        pixmap.fill_path(&path, &white, FillRule::Winding, Transform::identity(), None);
    }
    if unread {
        let r = s * 0.2;
        let mut red = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        red.set_color(rgb(BADGE));
        pixmap.fill_path(&circle(s - r, s - r, r), &red, FillRule::Winding, Transform::identity(), None);
    }
    pixmap
}

/// Straight (not premultiplied) RGBA.
pub fn rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// ARGB32 in network byte order, as the tray protocol wants.
pub fn argb(pixmap: &Pixmap) -> Vec<u8> {
    rgba(pixmap)
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, a]| [*a, *r, *g, *b])
        .collect()
}

pub fn window_icon() -> Option<iced::window::Icon> {
    let pixmap = render(128, false);
    iced::window::icon::from_rgba(rgba(&pixmap), 128, 128).ok()
}

fn cache_dir() -> PathBuf {
    crate::config::dirs().cache_dir().join("icons")
}

/// The icon as a PNG file, for notifications and the desktop entry.
pub fn png_path() -> Option<PathBuf> {
    let path = cache_dir().join("vimgram.png");
    if !path.exists() {
        std::fs::create_dir_all(cache_dir()).ok()?;
        render(256, false).save_png(&path).ok()?;
    }
    Some(path)
}

/// Drop cached notification images older than `days`.
pub fn clean_cache(days: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir()) else { return };
    let max_age = std::time::Duration::from_secs(days * 86_400);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old && entry.file_name().to_string_lossy().starts_with("avatar-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A round avatar with the app icon in its corner, so notification daemons
/// that show a single image (mako) still show both. Cached per photo: by
/// its content, as TDLib's file ids are reused after a restart.
pub fn notification_image(avatar: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(avatar).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let path = cache_dir().join(format!("avatar-{:016x}.png", hasher.finish()));
    if path.exists() {
        return Some(path);
    }
    const SIZE: u32 = 128;
    const BADGE_SIZE: u32 = 48;
    let photo = image::load_from_memory(&bytes)
        .ok()?
        .resize_to_fill(SIZE, SIZE, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let mut photo_pixmap = Pixmap::new(SIZE, SIZE)?;
    for (dst, src) in photo_pixmap.pixels_mut().iter_mut().zip(photo.pixels()) {
        let [r, g, b, a] = src.0;
        *dst = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }

    let mut out = Pixmap::new(SIZE, SIZE)?;
    let s = SIZE as f32;
    let mut mask = tiny_skia::Mask::new(SIZE, SIZE)?;
    mask.fill_path(&circle(s / 2.0, s / 2.0, s / 2.0), FillRule::Winding, true, Transform::identity());
    out.draw_pixmap(0, 0, photo_pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), Some(&mask));

    // White ring around the badge separates it from the photo.
    let b = BADGE_SIZE as f32;
    let (cx, cy) = (s - b / 2.0, s - b / 2.0);
    let mut white = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    white.set_color(Color::WHITE);
    out.fill_path(&circle(cx, cy, b / 2.0 + 3.0), &white, FillRule::Winding, Transform::identity(), None);
    let badge = render(BADGE_SIZE, false);
    out.draw_pixmap(
        (SIZE - BADGE_SIZE) as i32,
        (SIZE - BADGE_SIZE) as i32,
        badge.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    std::fs::create_dir_all(cache_dir()).ok()?;
    out.save_png(&path).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_round_and_has_a_plane() {
        let pixmap = render(64, false);
        let px = |x: u32, y: u32| pixmap.pixel(x, y).unwrap().demultiply();
        assert_eq!(px(0, 0).alpha(), 0, "corners are transparent");
        assert_eq!(px(32, 8).alpha(), 255);
        let plane = px((0.6 * 64.0) as u32, (0.45 * 64.0) as u32);
        assert_eq!((plane.red(), plane.green(), plane.blue()), (255, 255, 255));
        let dot = render(64, true);
        let red = dot.pixel(64 - 12, 12).unwrap().demultiply();
        assert!(red.red() > 200 && red.green() < 100);
    }

    #[test]
    fn tray_icon_is_a_bare_plane() {
        let pixmap = render_tray(64, false);
        let px = |x: u32, y: u32| pixmap.pixel(x, y).unwrap().demultiply();
        assert_eq!(px(0, 0).alpha(), 0);
        assert_eq!(px(63, 0).alpha(), 0, "no blue circle");
        let covered = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(covered > 64 * 64 / 5, "the plane fills the icon: {covered}");
        assert!(pixmap.pixels().iter().all(|p| {
            let c = p.demultiply();
            c.alpha() == 0 || (c.red(), c.green(), c.blue()) == (255, 255, 255)
        }));
    }

    #[test]
    fn argb_is_reordered() {
        let pixmap = render(16, false);
        let (rgba, argb) = (rgba(&pixmap), argb(&pixmap));
        assert_eq!(rgba.len(), argb.len());
        let i = (8 * 16 + 8) * 4;
        assert_eq!(&argb[i..i + 4], &[rgba[i + 3], rgba[i], rgba[i + 1], rgba[i + 2]]);
    }

    #[test]
    fn avatar_gets_a_badge() {
        let dir = std::env::temp_dir().join(format!("vimgram-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let avatar = dir.join("a.png");
        image::RgbaImage::from_pixel(160, 160, image::Rgba([0, 200, 0, 255])).save(&avatar).unwrap();
        let out = notification_image(&avatar).unwrap();
        let img = image::open(&out).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0).0[3], 0, "round");
        assert_eq!(img.get_pixel(40, 64).0, [0, 200, 0, 255]);
        let corner = img.get_pixel(128 - 10, 128 - 24).0;
        assert!(corner[2] > 150, "badge in the corner: {corner:?}");
        let _ = std::fs::remove_file(out);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn other_photos_get_other_images() {
        let dir = std::env::temp_dir().join(format!("vimgram-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (red, blue) = (dir.join("r.png"), dir.join("b.png"));
        image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 0, 0, 255])).save(&red).unwrap();
        image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 200, 255])).save(&blue).unwrap();
        let (a, b) = (notification_image(&red).unwrap(), notification_image(&blue).unwrap());
        assert_ne!(a, b);
        assert_eq!(notification_image(&red).unwrap(), a, "cached");
        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
        let _ = std::fs::remove_dir_all(dir);
    }
}
