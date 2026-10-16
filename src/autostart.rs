//! Start with the session. Off unless turned on with `:autostart on`.
//!
//! When the session activates `graphical-session.target` (uwsm, or a
//! Hyprland config that starts a target bound to it), a systemd user
//! service is enabled. Otherwise an XDG autostart entry is written, which
//! desktops like KDE and GNOME run.

use std::path::{Path, PathBuf};
use std::process::Command;

const UNIT: &str = "vimgram.service";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Systemd,
    Xdg,
}

fn config_dir() -> Option<PathBuf> {
    Some(directories::BaseDirs::new()?.config_dir().to_path_buf())
}

fn unit_path() -> Option<PathBuf> {
    Some(config_dir()?.join("systemd/user").join(UNIT))
}

fn entry_path() -> Option<PathBuf> {
    Some(config_dir()?.join("autostart/vimgram.desktop"))
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("systemctl: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

fn graphical_session() -> bool {
    systemctl(&["is-active", "--quiet", "graphical-session.target"]).is_ok()
}

/// How autostart is set up now, if at all.
pub fn current() -> Option<Method> {
    if systemctl(&["is-enabled", "--quiet", UNIT]).is_ok() {
        Some(Method::Systemd)
    } else if entry_path().is_some_and(|p| p.exists()) {
        Some(Method::Xdg)
    } else {
        None
    }
}

/// Quote for both desktop entries and systemd: "…" with \ " ` $ escaped.
fn quote(path: &Path, dollar: &str) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' | '"' | '`' => {
                out.push('\\');
                out.push(c);
            }
            '$' => out.push_str(dollar),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn unit(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=vimgram Telegram client\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart={} --hidden\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        quote(exe, "$$")
    )
}

pub fn entry(exe: &Path, icon: Option<&Path>) -> String {
    let mut entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=vimgram\n\
         Comment=Vim-style Telegram client\n\
         Exec={} --hidden\n\
         Terminal=false\n",
        quote(exe, "\\$")
    );
    if let Some(icon) = icon {
        entry.push_str(&format!("Icon={}\n", icon.display()));
    }
    entry
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// Enable autostart (blocking: runs systemctl).
pub fn enable() -> Result<Method, String> {
    disable()?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if graphical_session() {
        write(&unit_path().ok_or("no home directory")?, &unit(&exe))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", UNIT])?;
        Ok(Method::Systemd)
    } else {
        let icon = crate::icon::png_path();
        write(&entry_path().ok_or("no home directory")?, &entry(&exe, icon.as_deref()))?;
        Ok(Method::Xdg)
    }
}

/// Disable autostart (blocking).
pub fn disable() -> Result<(), String> {
    if let Some(unit) = unit_path().filter(|p| p.exists()) {
        let _ = systemctl(&["disable", UNIT]);
        std::fs::remove_file(unit).map_err(|e| e.to_string())?;
        let _ = systemctl(&["daemon-reload"]);
    }
    if let Some(entry) = entry_path().filter(|p| p.exists()) {
        std::fs::remove_file(entry).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_the_executable() {
        let exe = Path::new("/home/u/my $dir/vim\"gram");
        assert!(entry(exe, None).contains("Exec=\"/home/u/my \\$dir/vim\\\"gram\" --hidden\n"));
        assert!(unit(exe).contains("ExecStart=\"/home/u/my $$dir/vim\\\"gram\" --hidden\n"));
        assert!(unit(exe).contains("WantedBy=graphical-session.target"));
    }
}
