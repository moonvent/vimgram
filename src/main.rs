// No console window behind the app in Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod archive;
mod autostart;
mod clipboard;
mod config;
mod emoji;
mod export;
mod fonts;
mod icon;
mod instance;
mod media;
mod notify;
mod nvim;
mod player;
mod stream;
mod tg;
mod tray;
mod view;
mod vim;
mod voice;

use app::App;
use config::Config;

fn main() -> iced::Result {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            let text = format!("Failed to load config: {e}\nExpected api_id and api_hash (from https://my.telegram.org) in it.");
            eprintln!("vimgram: {text}");
            // Without a console nobody would see the message.
            #[cfg(windows)]
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("vimgram")
                .set_description(&text)
                .show();
            std::process::exit(1);
        }
    };
    // `--hidden`: start in the tray (used by autostart).
    let hidden = std::env::args().any(|a| a == "--hidden");
    if instance::another_runs(hidden) {
        return Ok(());
    }
    if let Err(e) = fonts::setup() {
        eprintln!("vimgram: emoji will be blank: {e}; run scripts/fetch-emoji.sh");
    }
    // Not from debug builds: the menu would start target/debug/vimgram.
    #[cfg(target_os = "linux")]
    if !cfg!(debug_assertions)
        && let Err(e) = autostart::install_menu_entry()
    {
        eprintln!("vimgram: no application menu entry: {e}");
    }

    // A daemon rather than an application: closing the window to the tray
    // must not end the program (Wayland can't hide a window, only close it).
    iced::daemon(move || App::new(cfg.clone(), hidden), App::update, view::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .run()
}
