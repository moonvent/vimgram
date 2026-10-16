//! Desktop notifications through the freedesktop D-Bus interface
//! (mako, dunst, swaync, … on Hyprland).

/// Notification servers interpret a subset of HTML in the body.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Show a notification without blocking the caller.
pub async fn show(title: String, body: String) {
    let _ = tokio::task::spawn_blocking(move || {
        notify_rust::Notification::new()
            .appname("vimgram")
            .summary(&title)
            .body(&escape(&body))
            .icon("mail-message-new")
            .show()
    })
    .await;
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
