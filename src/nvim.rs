//! Themes from Neovim colorschemes: a headless `nvim` loads the user's
//! config, applies the colorscheme and reports the colors of a few
//! highlight groups, which become the six colors of an iced palette.

use iced::theme::Palette;
use iced::{Color, Theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A config that hangs (a prompt, a plugin waiting on the network) gives
/// up after this.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Runs in the headless nvim. `nvim_get_hl` gives `{}` for a group
/// without colors, which `vim.json` writes as `[]`, so only fg and bg are
/// passed on, null when unset.
const SCRIPT: &str = r#"
local name = vim.env.VIMGRAM_COLORSCHEME
if name ~= nil and name ~= '' then
  local ok, err = pcall(vim.cmd.colorscheme, name)
  if not ok then
    io.stdout:write(vim.json.encode({ error = tostring(err) }))
    return
  end
end
local groups = {}
for _, group in ipairs(vim.split(vim.env.VIMGRAM_GROUPS, ',')) do
  local hl = vim.api.nvim_get_hl(0, { name = group, link = false })
  groups[group] = { fg = hl.fg or vim.NIL, bg = hl.bg or vim.NIL }
end
io.stdout:write(vim.json.encode({ name = vim.g.colors_name or 'default', groups = groups }))
"#;

/// Where each palette color comes from: the first group that has one.
const PRIMARY: &[&str] = &["Function", "Identifier", "Title"];
const SUCCESS: &[&str] = &["String", "DiagnosticOk", "DiffAdd"];
const WARNING: &[&str] = &["DiagnosticWarn", "WarningMsg", "Todo"];
const DANGER: &[&str] = &["DiagnosticError", "ErrorMsg", "Error"];

#[derive(Debug, Default, Deserialize)]
struct Hl {
    fg: Option<u32>,
    bg: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Report {
    error: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    groups: HashMap<String, Hl>,
}

/// The theme of a colorscheme, or of the one Neovim starts with when
/// `scheme` is empty.
pub fn theme(scheme: &str) -> Result<Theme, String> {
    let groups = [&["Normal"], PRIMARY, SUCCESS, WARNING, DANGER].concat();
    let mut child = Command::new("nvim")
        .args(["--headless", "-c", "lua load(vim.env.VIMGRAM_LUA)()", "-c", "qa!"])
        .env("VIMGRAM_LUA", SCRIPT)
        .env("VIMGRAM_COLORSCHEME", scheme)
        .env("VIMGRAM_GROUPS", groups.join(","))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nvim: {e}"))?;
    let started = Instant::now();
    while child.try_wait().map_err(|e| format!("nvim: {e}"))?.is_none() {
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("nvim: no answer in 5 s".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_string(&mut out).map_err(|e| format!("nvim: {e}"))?;
    }
    let report: Report = serde_json::from_str(&out).map_err(|_| format!("nvim: no colors from {out:?}"))?;
    if let Some(error) = report.error {
        return Err(format!("nvim: {error}"));
    }
    Ok(Theme::custom(format!("nvim: {}", report.name), palette(&report.groups)))
}

fn palette(groups: &HashMap<String, Hl>) -> Palette {
    let fg = |names: &[&str]| names.iter().find_map(|n| groups.get(*n)?.fg).map(rgb);
    let normal = groups.get("Normal");
    // A transparent colorscheme leaves the terminal's background showing:
    // black, or white under dark text.
    let text = normal.and_then(|n| n.fg).map(rgb);
    let background = normal.and_then(|n| n.bg).map(rgb).unwrap_or(match text {
        Some(t) if luminance(t) < 0.5 => Color::WHITE,
        _ => Color::BLACK,
    });
    let fallback = if luminance(background) < 0.5 { Palette::DARK } else { Palette::LIGHT };
    Palette {
        background,
        text: text.unwrap_or(fallback.text),
        primary: fg(PRIMARY).unwrap_or(fallback.primary),
        success: fg(SUCCESS).unwrap_or(fallback.success),
        warning: fg(WARNING).unwrap_or(fallback.warning),
        danger: fg(DANGER).unwrap_or(fallback.danger),
    }
}

fn rgb(c: u32) -> Color {
    Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(json: &str) -> HashMap<String, Hl> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn transparent_is_black() {
        let p = palette(&groups(r#"{"Normal":{"fg":10398636,"bg":null},"Function":{"fg":2526163},"Identifier":{"fg":1}}"#));
        assert_eq!(p.background, Color::BLACK);
        assert_eq!(p.text, rgb(10398636));
        assert_eq!(p.primary, rgb(2526163));
        assert_eq!(p.danger, Palette::DARK.danger);
    }

    #[test]
    fn transparent_under_dark_text_is_white() {
        let p = palette(&groups(r#"{"Normal":{"fg":1052688}}"#));
        assert_eq!(p.background, Color::WHITE);
        assert_eq!(p.primary, Palette::LIGHT.primary);
    }

    #[test]
    fn falls_through_groups() {
        let p = palette(&groups(r#"{"Normal":{"fg":16777215,"bg":1118481},"Function":{"fg":null},"Identifier":{"fg":255}}"#));
        assert_eq!(p.background, rgb(1118481));
        assert_eq!(p.primary, rgb(255));
    }
}
