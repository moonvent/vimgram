//! Desktop notifications through the freedesktop D-Bus interface
//! (mako, dunst, swaync, … on Hyprland).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Notification servers interpret a subset of HTML in the body.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Notifications waiting for a click; each holds a thread.
static WAITING: AtomicUsize = AtomicUsize::new(0);
const MAX_WAITING: usize = 32;

/// Show a notification; resolves to `true` when it was clicked.
/// `image` is the sender's avatar (with the app badge), `icon` the app icon.
pub async fn show(title: String, body: String, image: Option<PathBuf>, icon: Option<PathBuf>) -> bool {
    let (tx, rx) = futures::channel::oneshot::channel();
    // A dedicated thread: waiting for a click can take hours, which would
    // starve tokio's blocking pool.
    let spawned = std::thread::Builder::new().name("notification".into()).spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.appname("vimgram").summary(&title).body(&escape(&body));
        match &icon {
            Some(icon) => n.icon(&icon.to_string_lossy()),
            None => n.icon("mail-message-new"),
        };
        if let Some(image) = &image {
            n.image_path(&image.to_string_lossy());
        }
        let wait = WAITING.fetch_add(1, Ordering::SeqCst) < MAX_WAITING;
        if wait {
            n.action("default", "Open");
        }
        let mut clicked = false;
        if let Ok(handle) = n.show()
            && wait
        {
            handle.wait_for_action(|action| clicked = action == "default");
        }
        WAITING.fetch_sub(1, Ordering::SeqCst);
        let _ = tx.send(clicked);
    });
    spawned.is_ok() && rx.await.unwrap_or(false)
}

/// Body text: first line-ish of the message, not the whole essay.
pub fn preview(text: &str) -> String {
    const MAX: usize = 200;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > MAX {
        format!("{}…", flat.chars().take(MAX).collect::<String>())
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn previews_are_short_and_flat() {
        assert_eq!(preview("hello\n\nworld"), "hello world");
        let long = "слово ".repeat(100);
        let p = preview(&long);
        assert!(p.ends_with('…'));
        assert_eq!(p.chars().count(), 201);
    }
}
