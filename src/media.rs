//! Downloaded media, ready to render.
//!
//! Photos and static stickers are decoded lazily by the renderer. Animated
//! stickers (Lottie and VP9-with-alpha WebM) are short, so all their frames
//! are decoded up front into RGBA at display size. GIFs are really MP4s of any
//! length; they play through a GStreamer video pipeline instead.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io::Read;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use iced::Size;
use iced::widget::image::Handle;
use iced_video_player::Video;

use crate::tg::{Media, StickerKind};

/// How a downloaded file is turned into something drawable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// JPEG/PNG/WebP, decoded lazily by the renderer.
    Image,
    Tgs,
    Webm,
    Video,
}

impl Kind {
    pub fn of(media: &Media) -> Self {
        match media {
            // Videos show their still thumbnail inline.
            Media::Photo { .. } | Media::Video { .. } | Media::Voice { .. } => Kind::Image,
            Media::Sticker { kind: StickerKind::Webp, .. } => Kind::Image,
            Media::Sticker { kind: StickerKind::Tgs, .. } => Kind::Tgs,
            Media::Sticker { kind: StickerKind::Webm, .. } => Kind::Webm,
            Media::Animation { .. } => Kind::Video,
        }
    }
}

/// Pre-decoded animation frames.
pub struct Frames {
    frames: Vec<Handle>,
    frame_time: Duration,
}

impl Frames {
    pub fn at(&self, elapsed: Duration) -> &Handle {
        let index = (elapsed.as_nanos() / self.frame_time.as_nanos().max(1)) as usize;
        &self.frames[index % self.frames.len()]
    }
}

pub enum Loaded {
    Image(Handle),
    Frames(Frames),
    Video(Video),
}

/// A non-`Clone` value travelling through a `Message`, which must be `Clone`.
pub struct Shared<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(self.0.clone())
    }
}

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Shared(Arc::new(Mutex::new(Some(value))))
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

impl<T> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared(..)")
    }
}

/// Download priorities, 1..=32; higher is fetched first.
pub const PRIORITY_AVATAR: i32 = 8;
pub const PRIORITY_INLINE: i32 = 16;
pub const PRIORITY_VIEWER: i32 = 32;

/// Decoded animations kept around after scrolling out of view.
const MAX_HIDDEN_FRAMES: usize = 12;
const MAX_ANIMATION_FRAMES: usize = 180;
/// A sticker that takes longer than this to decode is broken.
const WEBM_TIMEOUT: Duration = Duration::from_secs(10);

/// What to decode once a file is downloaded.
pub struct Job {
    pub kind: Kind,
    pub path: String,
    pub size: Size,
}

#[derive(Default)]
pub struct Store {
    loaded: HashMap<i32, Loaded>,
    /// Files requested from TDLib (in flight, done or failed).
    requested: HashSet<i32>,
    pending: HashMap<i32, (Kind, Size)>,
}

impl Store {
    pub fn get(&self, file_id: i32) -> Option<&Loaded> {
        self.loaded.get(&file_id)
    }

    pub fn image(&self, file_id: i32) -> Option<&Handle> {
        match self.loaded.get(&file_id)? {
            Loaded::Image(handle) => Some(handle),
            _ => None,
        }
    }

    /// Mark `file_id` as requested; returns false if it already was.
    pub fn request(&mut self, file_id: i32, kind: Kind, size: Size) -> bool {
        let fresh = self.requested.insert(file_id);
        if fresh {
            self.pending.insert(file_id, (kind, size));
        }
        fresh
    }

    /// Images are ready right away; everything else needs a decode job.
    pub fn downloaded(&mut self, file_id: i32, path: String) -> Option<Job> {
        let (kind, size) = self.pending.remove(&file_id)?;
        match kind {
            Kind::Image => {
                self.loaded.insert(file_id, Loaded::Image(Handle::from_path(path)));
                None
            }
            _ => Some(Job { kind, path, size }),
        }
    }

    pub fn decoded(&mut self, file_id: i32, shared: &Shared<Loaded>) {
        if let Some(loaded) = shared.take() {
            self.loaded.insert(file_id, loaded);
        }
    }

    /// Stop videos that scrolled out of view and cap hidden animations.
    /// Dropped files are re-requested (from TDLib's cache) when visible again.
    pub fn retain_visible(&mut self, visible: &HashSet<i32>) {
        let mut hidden_frames = Vec::new();
        let mut dropped = Vec::new();
        for (id, loaded) in &self.loaded {
            if visible.contains(id) {
                continue;
            }
            match loaded {
                Loaded::Video(_) => dropped.push(*id),
                Loaded::Frames(_) => hidden_frames.push(*id),
                Loaded::Image(_) => {}
            }
        }
        dropped.extend(hidden_frames.into_iter().skip(MAX_HIDDEN_FRAMES));
        for id in dropped {
            self.loaded.remove(&id);
            self.requested.remove(&id);
        }
    }
}

/// Decode a downloaded file off the UI thread.
pub async fn decode(job: Job) -> Result<Shared<Loaded>, String> {
    tokio::task::spawn_blocking(move || {
        let loaded = match job.kind {
            Kind::Image => Loaded::Image(Handle::from_path(&job.path)),
            Kind::Tgs => Loaded::Frames(decode_tgs(&job.path, job.size)?),
            Kind::Webm => Loaded::Frames(decode_webm(&job.path, job.size)?),
            Kind::Video => Loaded::Video(open_video(&job.path)?),
        };
        Ok(Shared::new(loaded))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn decode_tgs(path: &str, size: Size) -> Result<Frames, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut json = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut json)
        .map_err(|e| format!("tgs: {e}"))?;
    let mut animation =
        Lottie::from_data(json, path).ok_or_else(|| "tgs: invalid lottie data".to_string())?;

    let (w, h) = (size.width.max(1.0) as usize, size.height.max(1.0) as usize);
    let mut pixels = vec![0u32; w * h];
    let total = animation.totalframe().max(1);
    let fps = animation.framerate().max(1.0);
    // Sample at most 30 fps: plenty for stickers, half the memory of 60.
    let step = (fps / 30.0).ceil().max(1.0) as usize;
    let frames: Vec<Handle> = (0..total)
        .step_by(step)
        .take(MAX_ANIMATION_FRAMES)
        .map(|frame| {
            animation.render(frame, &mut pixels, w, h);
            Handle::from_rgba(w as u32, h as u32, argb_premultiplied_to_rgba(&pixels))
        })
        .collect();
    Ok(Frames {
        frames,
        frame_time: Duration::from_secs_f64(step as f64 / fps),
    })
}

/// The few calls of rlottie's C API that stickers need. Not the `rlottie`
/// crate: that one only builds on unix.
struct Lottie(NonNull<rlottie_sys::Lottie_Animation>);

impl Lottie {
    /// `key` caches the parsed animation inside rlottie.
    fn from_data(json: Vec<u8>, key: &str) -> Option<Self> {
        let json = CString::new(json).ok()?;
        let key = CString::new(key).ok()?;
        // SAFETY: all three are valid nul-terminated strings; rlottie copies them.
        let ptr = unsafe {
            rlottie_sys::lottie_animation_from_data(json.as_ptr(), key.as_ptr(), c"".as_ptr())
        };
        NonNull::new(ptr).map(Self)
    }

    fn totalframe(&self) -> usize {
        // SAFETY: the pointer is a live animation until drop.
        unsafe { rlottie_sys::lottie_animation_get_totalframe(self.0.as_ptr()) }
    }

    fn framerate(&self) -> f64 {
        // SAFETY: as above.
        unsafe { rlottie_sys::lottie_animation_get_framerate(self.0.as_ptr()) }
    }

    /// Draws `frame` into `pixels`, `width` × `height` of them.
    fn render(&mut self, frame: usize, pixels: &mut [u32], width: usize, height: usize) {
        assert_eq!(pixels.len(), width * height);
        // SAFETY: `pixels` holds exactly `height` rows of `width * 4` bytes.
        unsafe {
            rlottie_sys::lottie_animation_render(
                self.0.as_ptr(),
                frame,
                pixels.as_mut_ptr(),
                width,
                height,
                width * 4,
            );
        }
    }
}

impl Drop for Lottie {
    fn drop(&mut self) {
        // SAFETY: created by lottie_animation_from_data, destroyed only here.
        unsafe { rlottie_sys::lottie_animation_destroy(self.0.as_ptr()) }
    }
}

/// rlottie renders premultiplied ARGB pixels; iced wants straight RGBA.
fn argb_premultiplied_to_rgba(pixels: &[u32]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for px in pixels {
        let [a, r, g, b] = px.to_be_bytes();
        let unmul = |c: u8| {
            if a == 0 {
                0
            } else {
                ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[unmul(r), unmul(g), unmul(b), a]);
    }
    rgba
}

fn decode_webm(path: &str, size: Size) -> Result<Frames, String> {
    let (w, h) = (size.width.max(1.0) as u32, size.height.max(1.0) as u32);
    // GStreamer's alpha decoder rejects stickers whose first frame lacks
    // the alpha layer; ffmpeg copes with those.
    let (frames, fps) = match decode_webm_gst(path, w, h) {
        Ok(decoded) => decoded,
        Err(gst_error) => decode_webm_ffmpeg(path, w, h).map_err(|e| format!("{gst_error}; {e}"))?,
    };
    Ok(Frames {
        frames,
        frame_time: Duration::from_secs_f64(1.0 / fps),
    })
}

fn decode_webm_gst(path: &str, w: u32, h: u32) -> Result<(Vec<Handle>, f64), String> {
    gst::init().map_err(|e| e.to_string())?;
    // decodebin picks vp9alphadecodebin, which keeps the alpha channel.
    let description = format!(
        "filesrc location=\"{}\" ! decodebin ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,width={w},height={h},pixel-aspect-ratio=1/1 ! \
         appsink name=sink sync=false",
        path.replace('"', "\\\"")
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| e.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "webm: not a pipeline".to_string())?;
    let sink = pipeline
        .by_name("sink")
        .and_then(|e| e.downcast::<AppSink>().ok())
        .ok_or_else(|| "webm: no appsink".to_string())?;
    let bus = pipeline.bus().ok_or_else(|| "webm: no bus".to_string())?;

    pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + WEBM_TIMEOUT;
    let mut frames = Vec::new();
    let mut fps = 30.0;
    let mut error = None;
    // A failing pipeline never reaches EOS, so poll the bus between samples.
    while frames.len() < MAX_ANIMATION_FRAMES {
        if let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(e) = msg.view() {
                error = Some(e.error().to_string());
            }
            break;
        }
        if std::time::Instant::now() > deadline {
            error = Some("timed out".into());
            break;
        }
        let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(50)) else {
            if sink.is_eos() {
                break;
            }
            continue;
        };
        if let Some(rate) = sample
            .caps()
            .and_then(|caps| caps.structure(0)?.get::<gst::Fraction>("framerate").ok())
            .filter(|rate| rate.numer() > 0 && rate.denom() > 0)
        {
            fps = f64::from(rate.numer()) / f64::from(rate.denom());
        }
        let Some(buffer) = sample.buffer() else { continue };
        let Ok(map) = buffer.map_readable() else { continue };
        frames.push(Handle::from_rgba(w, h, map.as_slice().to_vec()));
    }
    let _ = pipeline.set_state(gst::State::Null);

    match error {
        Some(e) => Err(format!("webm: {e}")),
        None if frames.is_empty() => Err(format!("webm: no frames decoded from {path}")),
        None => Ok((frames, fps)),
    }
}

/// Fallback through the ffmpeg binary: libvpx decodes the alpha layer
/// whenever a frame has one.
fn decode_webm_ffmpeg(path: &str, w: u32, h: u32) -> Result<(Vec<Handle>, f64), String> {
    use std::process::{Command, Stdio};
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=r_frame_rate", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe: {e}"))?;
    let fps = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .split_once('/')
        .and_then(|(n, d)| Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok()?))
        .filter(|fps| fps.is_finite() && *fps > 0.0)
        .unwrap_or(30.0);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-c:v", "libvpx-vp9", "-i"])
        .arg(path)
        .args(["-frames:v", &MAX_ANIMATION_FRAMES.to_string()])
        .args(["-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let frame_len = (w * h * 4) as usize;
    let frames: Vec<Handle> = output
        .stdout
        .chunks_exact(frame_len)
        .map(|frame| Handle::from_rgba(w, h, frame.to_vec()))
        .collect();
    if frames.is_empty() {
        return Err(format!("ffmpeg: no frames decoded from {path}"));
    }
    Ok((frames, fps))
}

fn open_video(path: &str) -> Result<Video, String> {
    let uri = url::Url::from_file_path(path).map_err(|()| format!("bad path: {path}"))?;
    let mut video = Video::new(&uri).map_err(|e| e.to_string())?;
    video.set_muted(true);
    video.set_looping(true);
    Ok(video)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes every sticker TDLib has cached; run with `--ignored`.
    #[test]
    #[ignore]
    fn decodes_cached_stickers() {
        let dir = crate::config::dirs().data_dir().join("db/stickers");
        let mut failed = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let p = path.to_string_lossy().into_owned();
            let size = Size::new(160.0, 160.0);
            let result = match path.extension().and_then(|e| e.to_str()) {
                Some("tgs") => decode_tgs(&p, size).map(|f| f.frames.len()),
                Some("webm") => decode_webm(&p, size).map(|f| f.frames.len()),
                Some("webp") => image::open(&path).map(|_| 1).map_err(|e| e.to_string()),
                _ => continue,
            };
            println!("{p}: {result:?}");
            if result.is_err() {
                failed.push(p);
            }
        }
        assert!(failed.is_empty(), "{failed:?}");
    }
}
