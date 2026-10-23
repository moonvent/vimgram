//! One vimgram at a time: a second start (from the application menu, say)
//! shows the running one's window and exits; TDLib can't open its database
//! twice anyway. They talk over a Unix socket. Elsewhere a second start
//! runs as before.

#[cfg(unix)]
mod imp {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    use futures::{SinkExt, Stream, StreamExt};

    const SHOW: &str = "show";

    /// Bound by `claim`, taken by `listen`.
    static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

    fn path() -> PathBuf {
        let dirs = crate::config::dirs();
        dirs.runtime_dir().unwrap_or(dirs.cache_dir()).join("vimgram.sock")
    }

    /// True when another vimgram runs: it was asked to show its window,
    /// unless this start is `hidden` (autostart). Otherwise this one takes
    /// the socket over.
    pub fn another_runs(hidden: bool) -> bool {
        another_runs_at(&path(), hidden)
    }

    fn another_runs_at(path: &Path, hidden: bool) -> bool {
        if let Ok(mut running) = UnixStream::connect(path) {
            if !hidden {
                let _ = running.write_all(SHOW.as_bytes());
            }
            return true;
        }
        // Left over by a vimgram that has ended.
        let _ = std::fs::remove_file(path);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match UnixListener::bind(path) {
            Ok(listener) => *LISTENER.lock().unwrap() = Some(listener),
            Err(e) => eprintln!("vimgram: {}: {e}; a second start won't find this one", path.display()),
        }
        false
    }

    /// Yields each time a second start asks for the window.
    pub fn listen() -> impl Stream<Item = ()> {
        iced::stream::channel(4, async |mut out| {
            let Some(listener) = LISTENER.lock().unwrap().take() else {
                return;
            };
            let (shows, mut requests) = futures::channel::mpsc::unbounded();
            std::thread::spawn(move || {
                while accept(&listener) {
                    if shows.unbounded_send(()).is_err() {
                        break;
                    }
                }
            });
            while requests.next().await.is_some() {
                if out.send(()).await.is_err() {
                    break;
                }
            }
        })
    }

    /// Waits for a second start that wants the window; false once the
    /// socket fails.
    fn accept(listener: &UnixListener) -> bool {
        for mut stream in listener.incoming() {
            let Ok(stream) = stream.as_mut() else {
                return false;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut request = String::new();
            let _ = stream.take(64).read_to_string(&mut request);
            if request == SHOW {
                return true;
            }
        }
        false
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn second_start_finds_the_first() {
            let dir = std::env::temp_dir().join(format!("vimgram-instance-{}", std::process::id()));
            let path = dir.join("vimgram.sock");
            std::fs::create_dir_all(&dir).unwrap();
            // A socket left by a vimgram that has ended doesn't count.
            drop(UnixListener::bind(&path).unwrap());
            assert!(!another_runs_at(&path, false));
            let first = LISTENER.lock().unwrap().take().unwrap();
            let waiting = std::thread::spawn(move || accept(&first));
            assert!(another_runs_at(&path, false));
            assert!(waiting.join().unwrap());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use futures::Stream;

    pub fn another_runs(_hidden: bool) -> bool {
        false
    }

    pub fn listen() -> impl Stream<Item = ()> {
        futures::stream::empty()
    }
}

pub use imp::{another_runs, listen};

/// Bring the open window to the front. Hyprland ignores a window asking
/// for focus itself unless `misc:focus_on_activate` is set: ask it directly.
pub fn raise() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let window = format!("pid:{}", std::process::id());
        std::thread::spawn(move || {
            let _ = std::process::Command::new("hyprctl")
                .args(["dispatch", "focuswindow", &window])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
}
