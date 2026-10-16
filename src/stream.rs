//! Stream a Telegram file into GStreamer while TDLib downloads it.
//!
//! `playbin uri=appsrc://` gets a seekable `appsrc`. A worker thread feeds
//! it one buffer per `need-data`: whatever TDLib already has at the current
//! offset, or it moves TDLib's download there and waits. The appsrc callbacks
//! never block, so seeks and shutdown stay instant even while data is missing.
//!
//! The stream type must be `Seekable` (push mode), not `RandomAccess`: in
//! random-access mode demuxers pull exact byte ranges, and buffers pushed
//! ahead of time get handed out for the wrong ranges.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::executor::block_on;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSrc, AppStreamType};

use crate::tg;

/// Bytes pushed per buffer, at most.
const CHUNK: u64 = 256 * 1024;
/// How long to wait for missing data before giving up.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(40);

pub const URI: &str = "appsrc://";

/// Where the bytes come from.
pub trait Source: Send + Sync + 'static {
    /// Up to `want` bytes at `offset`, waiting for them if necessary.
    /// `Ok(None)` when `stale()` says the request is no longer wanted.
    fn fetch(&self, offset: u64, want: u64, stale: &dyn Fn() -> bool) -> Result<Option<Vec<u8>>, String>;
}

/// A Telegram file, downloaded on demand by TDLib.
pub struct Telegram(pub i32);

impl Source for Telegram {
    fn fetch(&self, offset: u64, want: u64, stale: &dyn Fn() -> bool) -> Result<Option<Vec<u8>>, String> {
        fetch(self.0, offset, want, stale)
    }
}

#[derive(Default)]
struct State {
    offset: u64,
    /// The appsrc asked for data and has not got it yet.
    wanted: bool,
    /// Bumped on every seek, so data fetched for an old offset is dropped.
    generation: u64,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    cancelled: AtomicBool,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut State)) {
        if let Ok(mut state) = self.state.lock() {
            f(&mut state);
        }
        self.wake.notify_all();
    }
}

/// Stops the worker when dropped.
pub struct Handle(Arc<Shared>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.wake.notify_all();
    }
}

/// Make `playbin` (with `uri=appsrc://`) read `size` bytes from `source`.
pub fn attach(playbin: &gst::Element, source: Arc<dyn Source>, size: u64) -> Handle {
    let shared = Arc::new(Shared::default());
    let setup = shared.clone();
    playbin.connect("source-setup", false, move |args| {
        let src = args
            .get(1)
            .and_then(|v| v.get::<gst::Element>().ok())
            .and_then(|e| e.downcast::<AppSrc>().ok());
        if let Some(src) = src {
            configure(&src, source.clone(), size, setup.clone());
        }
        None
    });
    Handle(shared)
}

fn configure(src: &AppSrc, source: Arc<dyn Source>, size: u64, shared: Arc<Shared>) {
    src.set_stream_type(AppStreamType::Seekable);
    src.set_format(gst::Format::Bytes);
    src.set_size(size as i64);

    // TDLib keeps downloading ahead on its own, so no read-ahead here.
    let (need, seek) = (shared.clone(), shared.clone());
    src.set_callbacks(
        gstreamer_app::AppSrcCallbacks::builder()
            .need_data(move |_, _| need.update(|s| s.wanted = true))
            .seek_data(move |_, to| {
                seek.update(|s| {
                    s.offset = to;
                    s.generation += 1;
                });
                true
            })
            .build(),
    );

    let weak = src.downgrade();
    std::thread::Builder::new()
        .name("media-stream".into())
        .spawn(move || worker(&weak, source.as_ref(), size, &shared))
        .expect("failed to spawn stream thread");
}

fn worker(src: &gst::glib::WeakRef<AppSrc>, source: &dyn Source, size: u64, shared: &Shared) {
    loop {
        // Wait until the appsrc wants data.
        let (offset, generation) = {
            let Ok(mut state) = shared.state.lock() else { return };
            while !state.wanted && !shared.cancelled.load(Ordering::SeqCst) {
                let Ok((next, _)) = shared.wake.wait_timeout(state, Duration::from_millis(500)) else {
                    return;
                };
                state = next;
                if src.upgrade().is_none() {
                    return;
                }
            }
            (state.offset, state.generation)
        };
        if shared.cancelled.load(Ordering::SeqCst) {
            return;
        }
        let Some(appsrc) = src.upgrade() else { return };

        if offset >= size {
            shared.update(|s| s.wanted = false);
            let _ = appsrc.end_of_stream();
            continue;
        }

        let stale = || {
            shared.cancelled.load(Ordering::SeqCst)
                || shared.state.lock().map_or(true, |s| s.generation != generation)
        };
        match source.fetch(offset, CHUNK.min(size - offset), &stale) {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                // Push under the lock, so a seek cannot slip in between the
                // generation check and the push.
                let Ok(mut state) = shared.state.lock() else { return };
                if state.generation == generation {
                    state.offset += bytes.len() as u64;
                    state.wanted = false;
                    let mut buffer = gst::Buffer::from_mut_slice(bytes);
                    if let Some(buffer) = buffer.get_mut() {
                        buffer.set_offset(offset);
                    }
                    let _ = appsrc.push_buffer(buffer);
                }
            }
            // Seeked away or cancelled meanwhile: start over.
            Ok(_) => {}
            Err(_) => {
                let _ = appsrc.end_of_stream();
                return;
            }
        }
    }
}

/// Read up to `want` bytes at `offset`, downloading them first if needed.
/// Returns `Ok(None)` when `stale()` says the request is no longer wanted.
fn fetch(file_id: i32, offset: u64, want: u64, stale: &dyn Fn() -> bool) -> Result<Option<Vec<u8>>, String> {
    block_on(async {
        let deadline = Instant::now() + STALL_TIMEOUT;
        let mut requested = false;
        loop {
            if stale() {
                return Ok(None);
            }
            let ready = tg::downloaded_prefix(file_id, offset as i64).await?;
            if ready > 0 {
                let count = ready.min(want as i64);
                return tg::read_part(file_id, offset as i64, count).await.map(Some);
            }
            if !requested {
                tg::start_download(file_id, offset as i64).await?;
                requested = true;
            }
            if Instant::now() > deadline {
                return Err(format!("file {file_id}: no data at offset {offset}"));
            }
            std::thread::sleep(POLL);
        }
    })
}

/// Block until the start of the file is available, so the pipeline can
/// preroll within `iced_video_player`'s timeout.
pub fn wait_for_start(file_id: i32) -> Result<(), String> {
    match fetch(file_id, 0, 1, &|| false)? {
        Some(_) => Ok(()),
        None => Err("cancelled".into()),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::{Path, PathBuf};

    /// A local file standing in for a Telegram download.
    pub struct FileSource(pub PathBuf);

    impl Source for FileSource {
        fn fetch(&self, offset: u64, want: u64, _: &dyn Fn() -> bool) -> Result<Option<Vec<u8>>, String> {
            let mut file = std::fs::File::open(&self.0).map_err(|e| e.to_string())?;
            file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            let mut buf = vec![0; want as usize];
            let n = file.read(&mut buf).map_err(|e| e.to_string())?;
            buf.truncate(n);
            Ok(Some(buf))
        }
    }

    /// A short MP4 with H.264 video and AAC audio. GStreamer's mp4mux puts
    /// the index at the end, the hardest case for streaming.
    pub fn sample_mp4() -> Option<PathBuf> {
        // Tests run in parallel: encode once, and only publish a finished file.
        static SAMPLE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
        SAMPLE.get_or_init(encode_sample).clone()
    }

    fn encode_sample() -> Option<PathBuf> {
        gst::init().ok()?;
        let path = std::env::temp_dir().join(format!("vimgram-test-{}.mp4", std::process::id()));
        let partial = path.with_extension("part");
        let description = format!(
            "videotestsrc num-buffers=90 ! video/x-raw,width=320,height=240,framerate=30/1 ! \
             x264enc ! h264parse ! mp4mux name=m ! filesink location={} \
             audiotestsrc num-buffers=130 ! audioconvert ! avenc_aac ! m.",
            partial.display()
        );
        let pipeline = gst::parse::launch(&description).ok()?;
        pipeline.set_state(gst::State::Playing).ok()?;
        let bus = pipeline.bus()?;
        let done = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        let _ = pipeline.set_state(gst::State::Null);
        if !matches!(done.map(|m| m.type_()), Some(gst::MessageType::Eos)) {
            return None;
        }
        std::fs::rename(&partial, &path).ok()?;
        Some(path)
    }

    fn size_of(path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }

    /// Play `playbin` until EOS, failing on errors.
    fn play_to_end(pipeline: &gst::Element) -> Result<(), String> {
        pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        let bus = pipeline.bus().ok_or("no bus")?;
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(30),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .ok_or("timeout")?;
        let _ = pipeline.set_state(gst::State::Null);
        match msg.view() {
            gst::MessageView::Error(e) => Err(format!("{} ({:?})", e.error(), e.debug())),
            _ => Ok(()),
        }
    }

    #[test]
    fn streams_mp4_through_appsrc() {
        let Some(path) = sample_mp4() else {
            eprintln!("skipping: cannot encode a sample video");
            return;
        };
        let size = size_of(&path);
        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", URI)
            .property("video-sink", gst::ElementFactory::make("fakesink").build().unwrap())
            .property("audio-sink", gst::ElementFactory::make("fakesink").build().unwrap())
            .build()
            .unwrap();
        let _handle = attach(&pipeline, Arc::new(FileSource(path)), size);
        play_to_end(&pipeline).unwrap();
    }
}

