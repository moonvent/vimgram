//! Desktop notifications through the freedesktop D-Bus interface
//! (mako, dunst, swaync, … on Hyprland).

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Notification servers interpret a subset of HTML in the body.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Notifications waiting for a click; each holds a thread.
static WAITING: AtomicUsize = AtomicUsize::new(0);
const MAX_WAITING: usize = 32;

/// A notification on screen: which message it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    chat_id: i64,
    msg_id: i64,
    id: u32,
}

static SHOWN: Mutex<Vec<Shown>> = Mutex::new(Vec::new());
const MAX_SHOWN: usize = 500;

fn shown() -> std::sync::MutexGuard<'static, Vec<Shown>> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Notifications of `chat_id` up to message `up_to`, taken off the list.
fn take_read(chat_id: i64, up_to: i64) -> Vec<u32> {
    let mut shown = shown();
    let (read, unread): (Vec<Shown>, Vec<Shown>) =
        shown.iter().partition(|s| s.chat_id == chat_id && s.msg_id <= up_to);
    *shown = unread;
    read.into_iter().map(|s| s.id).collect()
}

/// Close the notifications of messages read in the meantime (here or on
/// another device), like the official apps do.
pub async fn withdraw(chat_id: i64, up_to: i64) {
    let ids = take_read(chat_id, up_to);
    if ids.is_empty() {
        return;
    }
    let Ok(conn) = zbus::Connection::session().await else {
        return;
    };
    for id in ids {
        let _ = conn
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "CloseNotification",
                &(id,),
            )
            .await;
    }
}

/// Show a notification about message `msg_id` of `chat_id`; resolves to
/// `true` when it was clicked. `image` is the chat photo (with the app
/// badge), `icon` the app icon.
pub async fn show(
    title: String,
    body: String,
    image: Option<PathBuf>,
    icon: Option<PathBuf>,
    chat_id: i64,
    msg_id: i64,
) -> bool {
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
        if let Ok(handle) = n.show() {
            let id = handle.id();
            {
                let mut shown = shown();
                if shown.len() >= MAX_SHOWN {
                    shown.remove(0);
                }
                shown.push(Shown { chat_id, msg_id, id });
            }
            if wait {
                handle.wait_for_action(|action| clicked = action == "default");
                // Closed or clicked: nothing left to withdraw.
                shown().retain(|s| s.id != id);
            }
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
    fn read_messages_are_withdrawn() {
        shown().extend([
            Shown { chat_id: -77, msg_id: 1, id: 9001 },
            Shown { chat_id: -77, msg_id: 5, id: 9002 },
            Shown { chat_id: -78, msg_id: 1, id: 9003 },
        ]);
        assert_eq!(take_read(-77, 3), vec![9001]);
        assert_eq!(take_read(-77, 10), vec![9002]);
        assert!(take_read(-77, 10).is_empty());
        assert_eq!(take_read(-78, i64::MAX), vec![9003]);
    }

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
