mod app;
mod archive;
mod config;
mod media;
mod player;
mod stream;
mod tg;
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

    iced::application(move || App::new(cfg.clone()), App::update, view::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .exit_on_close_request(false)
        .window_size((1200.0, 800.0))
        .run()
}
