mod app;
mod archive;
mod autostart;
mod clipboard;
mod config;
mod export;
mod fonts;
mod icon;
mod media;
mod notify;
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
            eprintln!("vimgram: failed to load config: {e}");
            eprintln!("Expected api_id and api_hash (from https://my.telegram.org) in it.");
            std::process::exit(1);
        }
    };
    // `--hidden`: start in the tray (used by autostart).
    let hidden = std::env::args().any(|a| a == "--hidden");
    if let Err(e) = fonts::setup() {
        eprintln!("vimgram: emoji will be blank: {e}; run scripts/fetch-emoji.sh");
    }

    // A daemon rather than an application: closing the window to the tray
    // must not end the program (Wayland can't hide a window, only close it).
    iced::daemon(move || App::new(cfg.clone(), hidden), App::update, view::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .run()
}
