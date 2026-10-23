//! Start with the session. Off unless turned on with `:autostart on`.
//!
//! Linux: when the session activates `graphical-session.target` (uwsm, or
//! a Hyprland config that starts a target bound to it), a systemd user
//! service is enabled. Otherwise an XDG autostart entry is written, which
//! desktops like KDE and GNOME run.
//!
//! Windows: a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Also the Linux application menu entry, which launchers (wofi, rofi,
//! fuzzel) and desktops list: written at every start of a release build.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    #[cfg(target_os = "linux")]
    Systemd,
    #[cfg(target_os = "linux")]
    Xdg,
    #[cfg(windows)]
    Registry,
}

impl Method {
    /// What `:autostart` reports while it is on.
    pub fn status(self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Method::Systemd => "Autostart is on (systemd user service vimgram.service)",
            #[cfg(target_os = "linux")]
            Method::Xdg => "Autostart is on (~/.config/autostart/vimgram.desktop)",
            #[cfg(windows)]
            Method::Registry => "Autostart is on (HKCU\\…\\CurrentVersion\\Run)",
        }
    }

    /// What `:autostart on` reports.
    pub fn enabled(self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Method::Systemd => "Autostart on: vimgram.service starts with the graphical session, in the tray",
            #[cfg(target_os = "linux")]
            Method::Xdg => "Autostart on: ~/.config/autostart/vimgram.desktop (starts in the tray)",
            #[cfg(windows)]
            Method::Registry => "Autostart on: vimgram starts when you sign in to Windows",
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{current, disable, enable, install_menu_entry};
#[cfg(windows)]
pub use windows::{current, disable, enable};

#[cfg(not(any(target_os = "linux", windows)))]
pub fn current() -> Option<Method> {
    None
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn enable() -> Result<Method, String> {
    Err("autostart is not supported on this platform".into())
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn disable() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Method;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const UNIT: &str = "vimgram.service";

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

    /// `hidden`: start in the tray (autostart) rather than with a window
    /// (the application menu).
    pub fn entry(exe: &Path, icon: Option<&Path>, hidden: bool) -> String {
        let mut entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=vimgram\n\
             Comment=Vim-style Telegram client\n\
             Exec={}{}\n\
             Terminal=false\n",
            quote(exe, "\\$"),
            if hidden { " --hidden" } else { "" }
        );
        if !hidden {
            entry.push_str(
                "Categories=Network;InstantMessaging;Chat;\n\
                 Keywords=Telegram;tg;messenger;\n\
                 StartupWMClass=vimgram\n",
            );
        }
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
            write(&entry_path().ok_or("no home directory")?, &entry(&exe, icon.as_deref(), true))?;
            Ok(Method::Xdg)
        }
    }

    /// Put vimgram into the application menu
    /// (`~/.local/share/applications/vimgram.desktop`), pointing at this
    /// executable. Left alone when already the same.
    pub fn install_menu_entry() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let path = directories::BaseDirs::new()
            .ok_or("no home directory")?
            .data_dir()
            .join("applications/vimgram.desktop");
        let icon = crate::icon::png_path();
        let contents = entry(&exe, icon.as_deref(), false);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == contents) {
            return Ok(());
        }
        write(&path, &contents)
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
}

#[cfg(windows)]
mod windows {
    use std::process::Command;

    use super::Method;

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "vimgram";

    fn reg(args: &[&str]) -> Result<(), String> {
        let output = Command::new("reg").args(args).output().map_err(|e| format!("reg: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!("reg: {}", String::from_utf8_lossy(&output.stderr).trim()))
        }
    }

    pub fn current() -> Option<Method> {
        reg(&["query", KEY, "/v", VALUE]).ok().map(|()| Method::Registry)
    }

    pub fn enable() -> Result<Method, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let command = format!("\"{}\" --hidden", exe.display());
        reg(&["add", KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &command, "/f"])?;
        Ok(Method::Registry)
    }

    pub fn disable() -> Result<(), String> {
        if current().is_some() {
            reg(&["delete", KEY, "/v", VALUE, "/f"])?;
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::path::Path;

    use super::linux::*;

    #[test]
    fn quotes_the_executable() {
        let exe = Path::new("/home/u/my $dir/vim\"gram");
        assert!(entry(exe, None, true).contains("Exec=\"/home/u/my \\$dir/vim\\\"gram\" --hidden\n"));
        assert!(entry(exe, None, false).contains("Exec=\"/home/u/my \\$dir/vim\\\"gram\"\n"));
        assert!(unit(exe).contains("ExecStart=\"/home/u/my $$dir/vim\\\"gram\" --hidden\n"));
        assert!(unit(exe).contains("WantedBy=graphical-session.target"));
    }
}
