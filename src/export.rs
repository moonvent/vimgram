//! Export a chat to PDF. Telegram itself only exports HTML/JSON (Desktop
//! app) and TDLib has no export at all, so the PDF is laid out here from
//! HTML with `printpdf`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use chrono::{DateTime, Local};

use crate::archive::Archive;
use crate::tg::{self, Msg, Sender};

/// Fetch the whole history of a chat, oldest first, including messages
/// deleted later that the archive remembers.
pub async fn collect(chat_id: i64, archive: Option<Archive>) -> Result<Vec<Msg>, String> {
    const PAGE: i32 = 100;
    let mut msgs: Vec<Msg> = Vec::new();
    let mut from = 0;
    loop {
        let page = tg::history(chat_id, from, 0, PAGE).await?;
        let Some(oldest) = page.first().map(|m| m.id) else { break };
        if oldest == from {
            break;
        }
        from = oldest;
        msgs.extend(page);
    }
    if let Some(archive) = archive {
        let deleted = tokio::task::spawn_blocking(move || archive.deleted_between(chat_id, 0, i64::MAX))
            .await
            .map_err(|e| e.to_string())??;
        msgs.extend(deleted.into_iter().map(Msg::from));
    }
    msgs.sort_by_key(|m| m.id);
    msgs.dedup_by_key(|m| m.id);
    Ok(msgs.into_iter().filter(|m| !m.scheduled).collect())
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn date(unix: i32) -> String {
    DateTime::from_timestamp(i64::from(unix), 0)
        .map(|d| d.with_timezone(&Local).format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_default()
}

pub fn html(title: &str, msgs: &[Msg], names: &HashMap<Sender, String>) -> String {
    let mut out = String::from(
        r#"<html><head><style>
        body { font-family: sans-serif; font-size: 10px; color: #222222; }
        h1 { font-size: 18px; margin-bottom: 2px; }
        .meta { color: #888888; font-size: 9px; margin-bottom: 12px; }
        .msg { margin-bottom: 7px; }
        .who { color: #2f63c7; font-weight: bold; }
        .me { color: #2e8b57; font-weight: bold; }
        .time { color: #999999; font-size: 8px; }
        .deleted { color: #b03a2e; }
        </style></head><body>"#,
    );
    out.push_str(&format!(
        "<h1>{}</h1><div class=\"meta\">{} messages, exported {}</div>",
        escape(title),
        msgs.len(),
        Local::now().format("%d.%m.%Y %H:%M")
    ));
    for m in msgs {
        let who = names.get(&m.sender).map_or("…", String::as_str);
        let class = if m.outgoing { "me" } else { "who" };
        let mut flags = String::new();
        if m.edited {
            flags.push_str(" · edited");
        }
        if m.deleted {
            flags.push_str(" · deleted");
        }
        let body = escape(&m.full_summary()).replace('\n', "<br/>");
        let style = if m.deleted { " deleted" } else { "" };
        out.push_str(&format!(
            "<div class=\"msg{style}\"><span class=\"{class}\">{}</span> \
             <span class=\"time\">{}{flags}</span><div>{body}</div></div>",
            escape(who),
            date(m.date)
        ));
    }
    out.push_str("</body></html>");
    out
}

/// Lay out and write the PDF (blocking, CPU heavy for big chats).
pub fn write_pdf(html: &str, path: &PathBuf) -> Result<(), String> {
    let options = printpdf::GeneratePdfOptions {
        margin_top: Some(15.0),
        margin_bottom: Some(15.0),
        margin_left: Some(15.0),
        margin_right: Some(15.0),
        ..Default::default()
    };
    let mut warnings = Vec::new();
    let doc = printpdf::PdfDocument::from_html(html, &BTreeMap::new(), &BTreeMap::new(), &options, &mut warnings)?;
    let bytes = doc.save(&printpdf::PdfSaveOptions::default(), &mut warnings);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Default location: ~/Documents/vimgram/<chat>-<date>.pdf
pub fn default_path(title: &str) -> PathBuf {
    let dirs = directories::UserDirs::new();
    let base = dirs
        .as_ref()
        .and_then(|d| d.document_dir().map(|p| p.to_path_buf()))
        .or_else(|| dirs.as_ref().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    let name: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    base.join("vimgram")
        .join(format!("{name}-{}.pdf", Local::now().format("%Y%m%d-%H%M")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: i64, text: &str, deleted: bool) -> Msg {
        Msg {
            id,
            chat_id: 1,
            sender: Sender::User(5),
            outgoing: false,
            date: 1_760_000_000,
            edited: false,
            text: text.into(),
            media: Vec::new(),
            album: 0,
            members: vec![id],
            reply_to: None,
            sending: false,
            scheduled: false,
            deleted,
            local_files: Vec::new(),
            reactions: Vec::new(),
            forward: None,
            links: Vec::new(),
            styles: Vec::new(),
            spoiler: false,
        }
    }

    #[test]
    fn html_escapes_and_marks_deleted() {
        let names = HashMap::from([(Sender::User(5), "Аня".to_owned())]);
        let html = html("<Chat>", &[msg(1, "a < b", false), msg(2, "gone", true)], &names);
        assert!(html.contains("&lt;Chat&gt;"));
        assert!(html.contains("a &lt; b"));
        assert!(html.contains("msg deleted"));
        assert!(html.contains("Аня"));
    }

    #[test]
    fn writes_a_pdf() {
        let names = HashMap::from([(Sender::User(5), "Аня".to_owned())]);
        let html = html("Тест", &[msg(1, "Привет, мир", false)], &names);
        let path = std::env::temp_dir().join(format!("vimgram-export-{}.pdf", std::process::id()));
        write_pdf(&html, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_path_is_safe() {
        let path = default_path("a/b: c");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("a_b__c-"));
        assert!(name.ends_with(".pdf"));
    }
}
