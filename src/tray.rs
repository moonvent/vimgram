//! Tray icon (StatusNotifierItem, shown by waybar's `tray` module). Other
//! platforms have none yet: `run` reports that, and the app keeps its window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Left click: show or hide the window.
    Toggle,
    Show,
    Quit,
}

/// Messages from the tray to the app.
#[derive(Debug, Clone)]
pub enum Output {
    Ready(Handle),
    Failed(String),
    Event(Event),
}

#[cfg(target_os = "linux")]
mod imp {
    use futures::channel::mpsc;
    use futures::{SinkExt, Stream};
    use ksni::TrayMethods;

    use super::{Event, Output};
    use crate::icon;

    struct Tray {
        unread: i32,
        events: mpsc::UnboundedSender<Event>,
    }

    impl Tray {
        fn send(&self, event: Event) {
            let _ = self.events.unbounded_send(event);
        }
    }

    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "vimgram".into()
        }

        fn title(&self) -> String {
            "vimgram".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            [22, 32, 48, 64]
                .into_iter()
                .map(|size| {
                    let pixmap = icon::render_tray(size, self.unread > 0);
                    ksni::Icon {
                        width: size as i32,
                        height: size as i32,
                        data: icon::argb(&pixmap),
                    }
                })
                .collect()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            let description = match self.unread {
                0 => "No unread chats".into(),
                1 => "1 unread chat".into(),
                n => format!("{n} unread chats"),
            };
            ksni::ToolTip {
                title: "vimgram".into(),
                description,
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(Event::Toggle);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            vec![
                StandardItem {
                    label: "Open vimgram".into(),
                    activate: Box::new(|tray: &mut Self| tray.send(Event::Show)),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|tray: &mut Self| tray.send(Event::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    #[derive(Clone)]
    pub struct Handle(ksni::Handle<Tray>);

    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("tray::Handle")
        }
    }

    impl Handle {
        pub async fn set_unread(self, unread: i32) {
            self.0.update(|tray| tray.unread = unread).await;
        }
    }

    /// Registers the tray icon and yields its clicks. Fails when nothing shows
    /// tray icons (no waybar `tray` module or similar).
    pub fn run() -> impl Stream<Item = Output> {
        iced::stream::channel(16, async |mut out| {
            let (events, mut clicks) = mpsc::unbounded();
            let tray = Tray { unread: 0, events };
            match tray.spawn().await {
                Ok(handle) => {
                    let _ = out.send(Output::Ready(Handle(handle))).await;
                }
                Err(e) => {
                    let _ = out.send(Output::Failed(e.to_string())).await;
                    return;
                }
            }
            use futures::StreamExt;
            while let Some(event) = clicks.next().await {
                if out.send(Output::Event(event)).await.is_err() {
                    break;
                }
            }
        })
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use futures::Stream;

    use super::Output;

    #[derive(Clone, Debug)]
    pub struct Handle;

    impl Handle {
        pub async fn set_unread(self, _unread: i32) {}
    }

    pub fn run() -> impl Stream<Item = Output> {
        futures::stream::once(async { Output::Failed("not supported on this platform yet".into()) })
    }
}

pub use imp::{Handle, run};
