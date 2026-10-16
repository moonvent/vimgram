//! Reading the Wayland clipboard with all its types (`wl-paste`), so
//! images and copied files can be pasted as attachments. iced's own
//! clipboard only does text; it remains the fallback.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    Text(String),
    /// Copied files, or an image saved to a temporary file.
    Files(Vec<PathBuf>),
    Empty,
}

/// Image types in order of preference, with the file extension to use.
const IMAGE_TYPES: [(&str, &str); 5] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
    ("image/bmp", "bmp"),
];

const TEXT_TYPES: [&str; 4] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING"];

fn wl_paste(mime: Option<&str>) -> Result<Vec<u8>, String> {
    let mut command = Command::new("wl-paste");
    command.arg("--no-newline");
    match mime {
        Some(mime) => command.args(["--type", mime]),
        None => command.arg("--list-types"),
    };
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("wl-paste: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!("wl-paste: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// Where pasted images are written before sending.
fn paste_dir() -> PathBuf {
    crate::config::dirs().cache_dir().join("paste")
}

/// Files of a `text/uri-list` that exist locally.
pub fn parse_uri_list(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| url::Url::parse(line).ok()?.to_file_path().ok())
        .collect()
}

/// What to paste, chosen from the offered types: copied files first, then
/// an image, then text.
pub fn choose(types: &[&str]) -> Option<&'static str> {
    if types.contains(&"text/uri-list") {
        return Some("text/uri-list");
    }
    if let Some((mime, _)) = IMAGE_TYPES.iter().find(|(mime, _)| types.contains(mime)) {
        return Some(mime);
    }
    TEXT_TYPES.iter().copied().find(|t| types.contains(t))
}

fn read_blocking() -> Result<Clip, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("not on Wayland".into());
    }
    let listed = String::from_utf8_lossy(&wl_paste(None)?).into_owned();
    let types: Vec<&str> = listed.lines().map(str::trim).collect();
    let Some(mime) = choose(&types) else {
        return Ok(Clip::Empty);
    };
    let data = wl_paste(Some(mime))?;
    if mime == "text/uri-list" {
        let files: Vec<PathBuf> = parse_uri_list(&String::from_utf8_lossy(&data))
            .into_iter()
            .filter(|p| p.is_file())
            .collect();
        if !files.is_empty() {
            return Ok(Clip::Files(files));
        }
        // Links to non-local things paste as text.
        return Ok(Clip::Text(String::from_utf8_lossy(&data).trim().to_owned()));
    }
    if let Some((_, ext)) = IMAGE_TYPES.iter().find(|(m, _)| *m == mime) {
        let dir = paste_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{}.{ext}", chrono::Local::now().format("pasted-%Y%m%d-%H%M%S%3f")));
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
        return Ok(Clip::Files(vec![path]));
    }
    Ok(Clip::Text(String::from_utf8_lossy(&data).into_owned()))
}

pub async fn read() -> Result<Clip, String> {
    tokio::task::spawn_blocking(read_blocking).await.map_err(|e| e.to_string())?
}

/// Remove pasted images older than a day (they have been sent by then).
pub fn clean_old_pastes() {
    let Ok(entries) = std::fs::read_dir(paste_dir()) else { return };
    let day = Duration::from_secs(86_400);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_files_then_images_then_text() {
        assert_eq!(choose(&["text/plain", "text/uri-list", "image/png"]), Some("text/uri-list"));
        // Browsers offer HTML next to the copied image.
        assert_eq!(choose(&["text/html", "image/jpeg", "image/png"]), Some("image/png"));
        assert_eq!(choose(&["text/html", "text/plain;charset=utf-8", "text/plain"]), Some("text/plain;charset=utf-8"));
        assert_eq!(choose(&["application/x-whatever"]), None);
    }

    #[test]
    fn parses_uri_lists() {
        let list = "# comment\r\nfile:///home/u/a%20b.png\r\nhttps://example.com/x\r\n";
        assert_eq!(parse_uri_list(list), vec![PathBuf::from("/home/u/a b.png")]);
    }
}
