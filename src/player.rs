//! Playback with adjustable speed. Both voice messages and videos run
//! through `scaletempo`, so speeding up keeps the pitch of voices.

use std::sync::Arc;
use std::time::Duration;

use futures::executor::block_on;
use gstreamer as gst;
use gstreamer::prelude::*;
use iced_video_player::Video;

use crate::{stream, tg};

/// `[` / `]` change the speed by this, between the two below.
const SPEED_STEP: f64 = 0.25;
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 5.0;

/// The next step up; a speed between steps (`:speed 1.6`) goes to the
/// nearest one above.
pub fn faster(speed: f64) -> f64 {
    (((speed / SPEED_STEP + 1e-6).floor() + 1.0) * SPEED_STEP).min(MAX_SPEED)
}

pub fn slower(speed: f64) -> f64 {
    (((speed / SPEED_STEP - 1e-6).ceil() - 1.0) * SPEED_STEP).max(MIN_SPEED)
}

pub fn fmt_speed(speed: f64) -> String {
    format!("{}x", (speed * 100.0).round() / 100.0)
}

/// Volume of videos and voice messages, 0.0–1.0, in steps of 10%.
pub fn louder(volume: f64) -> f64 {
    ((volume * 10.0).round() + 1.0).min(10.0) / 10.0
}

pub fn quieter(volume: f64) -> f64 {
    ((volume * 10.0).round() - 1.0).max(0.0) / 10.0
}

pub fn fmt_volume(volume: f64) -> String {
    format!("{}%", (volume * 100.0).round())
}

fn volume_path() -> std::path::PathBuf {
    crate::config::dirs().data_dir().join("volume")
}

/// The volume last set, in percent on disk; full when never set.
pub fn load_volume() -> f64 {
    std::fs::read_to_string(volume_path())
        .ok()
        .and_then(|t| t.trim().parse::<f64>().ok())
        .filter(|v| (0.0..=100.0).contains(v))
        .map_or(1.0, |v| v / 100.0)
}

pub fn save_volume(volume: f64) {
    let path = volume_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{}\n", (volume * 100.0).round()));
}

/// Media kinds that remember their own playback speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedKind {
    Voice,
    Video,
    /// Round video messages.
    Round,
    Music,
}

impl SpeedKind {
    const ALL: [SpeedKind; 4] = [SpeedKind::Voice, SpeedKind::Video, SpeedKind::Round, SpeedKind::Music];

    fn name(self) -> &'static str {
        match self {
            SpeedKind::Voice => "voice",
            SpeedKind::Video => "video",
            SpeedKind::Round => "round",
            SpeedKind::Music => "music",
        }
    }
}

/// Last speed per kind, kept across restarts.
#[derive(Debug, Clone, PartialEq)]
pub struct Speeds([f64; 4]);

impl Default for Speeds {
    fn default() -> Self {
        Speeds([1.0; 4])
    }
}

impl Speeds {
    pub fn get(&self, kind: SpeedKind) -> f64 {
        self.0[kind as usize]
    }

    pub fn set(&mut self, kind: SpeedKind, speed: f64) {
        self.0[kind as usize] = speed;
    }

    /// "voice 1.5" per line.
    pub fn parse(text: &str) -> Self {
        let mut speeds = Speeds::default();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(value)) = (parts.next(), parts.next()) else { continue };
            if let (Some(kind), Ok(speed)) =
                (SpeedKind::ALL.into_iter().find(|k| k.name() == name), value.parse::<f64>())
                && (MIN_SPEED..=MAX_SPEED).contains(&speed)
            {
                speeds.set(kind, speed);
            }
        }
        speeds
    }

    pub fn to_text(&self) -> String {
        SpeedKind::ALL.iter().map(|k| format!("{} {}\n", k.name(), self.get(*k))).collect()
    }

    fn path() -> std::path::PathBuf {
        crate::config::dirs().data_dir().join("speeds")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

fn scaletempo() -> Result<gst::Element, String> {
    gst::ElementFactory::make("scaletempo")
        .build()
        .map_err(|e| format!("scaletempo: {e}"))
}

fn uri(path: &str) -> Result<url::Url, String> {
    url::Url::from_file_path(path).map_err(|()| format!("bad path: {path}"))
}

/// `caps` with a frame rate, when they say none (0/1) or lack it.
fn with_framerate(caps: &gst::CapsRef) -> Option<gst::Caps> {
    let rate = caps.structure(0)?.get::<gst::Fraction>("framerate").ok();
    if rate.is_some_and(|r| r.numer() > 0 && r.denom() > 0) {
        return None;
    }
    let mut caps = caps.to_owned();
    caps.make_mut().set("framerate", gst::Fraction::new(30, 1));
    Some(caps)
}

/// A video being played, and the stream feeding it.
pub struct Movie {
    pub video: Video,
    speed: f64,
    _stream: Option<stream::Handle>,
}

impl Movie {
    pub fn set_volume(&mut self, volume: f64) {
        self.video.set_volume(volume);
    }

    /// Jump to `to`, keeping the speed.
    pub fn seek(&mut self, to: Duration) -> Result<(), String> {
        self.seek_at(to, self.speed, gst::SeekFlags::empty())
    }

    /// Play at `speed` from where the video is now. Not `Video::set_speed`:
    /// its seek runs to "the end", which qtdemux reading a stream (push
    /// mode) doesn't know yet and takes as 0, so the video stops dead.
    pub fn set_speed(&mut self, speed: f64) -> Result<(), String> {
        let position = self.video.pipeline().query_position::<gst::ClockTime>();
        let position = position.ok_or("video: position unknown")?;
        self.seek_at(Duration::from_nanos(position.nseconds()), speed, gst::SeekFlags::ACCURATE)?;
        self.speed = speed;
        Ok(())
    }

    /// `Video::seek` would reset the speed to its own (1x): the speed is
    /// changed here, not through it.
    fn seek_at(&self, to: Duration, speed: f64, flags: gst::SeekFlags) -> Result<(), String> {
        self.video
            .pipeline()
            .seek(
                speed,
                gst::SeekFlags::FLUSH | flags,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds(to.as_nanos() as u64),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .map_err(|e| e.to_string())
    }
}

/// A video with sound for the full-screen viewer, streamed while it
/// downloads. Blocks until playback can start, so call it off the UI thread.
pub fn open_video(file_id: i32, speed: f64, volume: f64) -> Result<Movie, String> {
    gst::init().map_err(|e| e.to_string())?;
    let size = match block_on(tg::file_source(file_id))? {
        tg::FileSource::Local(path) => return build(uri(&path)?.as_str(), None, speed, volume),
        tg::FileSource::Remote(size) => size,
    };
    if size > 0 {
        stream::wait_for_start(file_id)?;
        return open_stream(Arc::new(stream::Telegram(file_id)), size, speed, volume);
    }
    // Without a known size the appsrc cannot seek; download the whole file.
    let path = block_on(tg::download(file_id, 32))?;
    build(uri(&path)?.as_str(), None, speed, volume)
}

/// Play `size` bytes of video from `source`.
pub fn open_stream(source: Arc<dyn stream::Source>, size: u64, speed: f64, volume: f64) -> Result<Movie, String> {
    build(stream::URI, Some((source, size)), speed, volume)
}

/// Same pipeline as `Video::new`, plus a pitch-preserving audio filter.
fn build(uri: &str, source: Option<(Arc<dyn stream::Source>, u64)>, speed: f64, volume: f64) -> Result<Movie, String> {
    gst::init().map_err(|e| e.to_string())?;
    let description = format!(
        "playbin uri=\"{uri}\" video-sink=\"videoscale ! videoconvert ! appsink name=iced_video \
         drop=true caps=video/x-raw,format=NV12,pixel-aspect-ratio=1/1\""
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| e.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "video: not a pipeline".to_string())?;
    pipeline.set_property("audio-filter", scaletempo()?);
    // Before it starts playing, so it never starts loud.
    pipeline.set_property("volume", volume);
    let stream = source.map(|(source, size)| stream::attach(pipeline.upcast_ref(), source, size));

    let sink_bin: gst::Element = pipeline.property("video-sink");
    let video_sink = sink_bin
        .downcast::<gst::Bin>()
        .ok()
        .and_then(|bin| bin.by_name("iced_video"))
        .and_then(|sink| sink.downcast::<gstreamer_app::AppSink>().ok())
        .ok_or_else(|| "video: no appsink".to_string())?;
    // Variable frame rate videos (phones, screen recordings) say 0/1, which
    // iced_video_player refuses; it only keeps the number, frames follow
    // their timestamps anyway.
    if let Some(pad) = video_sink.static_pad("sink") {
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, |_, info| {
            if let Some(gst::PadProbeData::Event(event)) = &mut info.data
                && let gst::EventView::Caps(c) = event.view()
                && let Some(caps) = with_framerate(c.caps())
            {
                *event = gst::event::Caps::new(&caps);
            }
            gst::PadProbeReturn::Ok
        });
    }

    let video = Video::from_gst_pipeline(pipeline, video_sink, None).map_err(|e| e.to_string())?;
    let mut movie = Movie {
        video,
        speed: 1.0,
        _stream: stream,
    };
    if (speed - 1.0).abs() > 1e-6 {
        let _ = movie.set_speed(speed);
    }
    Ok(movie)
}

/// Audio-only playback (voice messages).
pub struct Audio {
    pipeline: gst::Pipeline,
    speed: f64,
    /// The rate seek needs a known position, so it waits for preroll.
    rate_pending: bool,
    /// Where to start, once it can seek there.
    start: Option<Duration>,
    finished: bool,
}

impl Audio {
    /// Play from `start` (from the beginning when zero).
    pub fn open(path: &str, speed: f64, volume: f64, start: Duration) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri(path)?.as_str())
            .property("audio-filter", scaletempo()?)
            .property("volume", volume)
            .build()
            .map_err(|e| e.to_string())?;
        let pipeline = playbin
            .downcast::<gst::Pipeline>()
            .map_err(|_| "audio: not a pipeline".to_string())?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("cannot play: {e}"))?;
        Ok(Audio {
            pipeline,
            speed,
            rate_pending: (speed - 1.0).abs() > 1e-6,
            start: (start > Duration::ZERO).then_some(start),
            finished: false,
        })
    }

    /// Handle bus messages; call regularly while playing.
    pub fn poll(&mut self) -> Result<(), String> {
        if let Some(bus) = self.pipeline.bus() {
            while let Some(msg) = bus.pop() {
                match msg.view() {
                    gst::MessageView::Eos(_) => self.finished = true,
                    gst::MessageView::Error(e) => return Err(format!("playback: {}", e.error())),
                    _ => {}
                }
            }
        }
        if (self.rate_pending || self.start.is_some()) && self.seek_to(self.start.unwrap_or(self.position())).is_ok() {
            self.rate_pending = false;
            self.start = None;
        }
        Ok(())
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn paused(&self) -> bool {
        self.pipeline.current_state() == gst::State::Paused
    }

    pub fn toggle_pause(&mut self) {
        let state = if self.paused() { gst::State::Playing } else { gst::State::Paused };
        let _ = self.pipeline.set_state(state);
    }

    pub fn position(&self) -> Duration {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map_or(Duration::ZERO, |t| Duration::from_nanos(t.nseconds()))
    }

    pub fn duration(&self) -> Option<Duration> {
        self.pipeline
            .query_duration::<gst::ClockTime>()
            .map(|t| Duration::from_nanos(t.nseconds()))
    }

    fn seek_to(&self, position: Duration) -> Result<(), String> {
        self.pipeline
            .seek(
                self.speed,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds(position.as_nanos() as u64),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .map_err(|e| e.to_string())
    }

    pub fn set_volume(&self, volume: f64) {
        self.pipeline.set_property("volume", volume);
    }

    /// Go to `at`; before it can seek, start there instead.
    pub fn seek(&mut self, at: Duration) {
        if self.start.is_some() || self.seek_to(at).is_err() {
            self.start = Some(at);
        }
    }

    pub fn seek_by(&mut self, delta: f64) {
        let target = (self.position().as_secs_f64() + delta).max(0.0);
        let target = match self.duration() {
            Some(d) => target.min(d.as_secs_f64()),
            None => target,
        };
        let _ = self.seek_to(Duration::from_secs_f64(target));
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
        self.rate_pending = self.seek_to(self.position()).is_err();
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Length and picture of a local video, for sending it: Telegram takes
/// these from the sender and shows 0:00 otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoInfo {
    /// Whole seconds, rounded.
    pub duration: i32,
    /// As displayed, i.e. after the rotation phones record in the metadata.
    pub width: i32,
    pub height: i32,
}

/// Read a video's duration and size from the file (blocking, quick: only
/// headers are parsed).
pub fn probe_video(path: &std::path::Path) -> Option<VideoInfo> {
    use gstreamer_pbutils::prelude::*;
    gst::init().ok()?;
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(15)).ok()?;
    let uri = url::Url::from_file_path(path).ok()?;
    let info = discoverer.discover_uri(uri.as_str()).ok()?;
    let duration = info.duration().map_or(0, |d| (d.mseconds() as f64 / 1000.0).round() as i32);
    let (mut width, mut height) = (0, 0);
    if let Some(video) = info.video_streams().into_iter().next() {
        (width, height) = (video.width() as i32, video.height() as i32);
        let rotated = video
            .tags()
            .and_then(|tags| tags.get::<gst::tags::ImageOrientation>().map(|t| t.get().to_owned()))
            .is_some_and(|o| o.contains("90") || o.contains("270"));
        if rotated {
            (width, height) = (height, width);
        }
    }
    Some(VideoInfo { duration, width, height })
}

/// What a music file says about itself, sent along with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioInfo {
    /// Seconds.
    pub duration: i32,
    pub title: String,
    pub performer: String,
}

/// Read a music file's length, title and artist (blocking, quick: only
/// headers are parsed).
pub fn probe_audio(path: &std::path::Path) -> Option<AudioInfo> {
    use gstreamer_pbutils::prelude::*;
    gst::init().ok()?;
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(15)).ok()?;
    let uri = url::Url::from_file_path(path).ok()?;
    let info = discoverer.discover_uri(uri.as_str()).ok()?;
    let duration = info.duration().map_or(0, |d| (d.mseconds() as f64 / 1000.0).round() as i32);
    // Container tags, else the audio stream's.
    let tags = info
        .tags()
        .or_else(|| info.audio_streams().into_iter().next().and_then(|s| s.tags()));
    let title = tags.as_ref().and_then(|t| t.get::<gst::tags::Title>()).map(|t| t.get().to_owned());
    let performer = tags.as_ref().and_then(|t| t.get::<gst::tags::Artist>()).map(|t| t.get().to_owned());
    Some(AudioInfo {
        duration,
        title: title.unwrap_or_default(),
        performer: performer.unwrap_or_default(),
    })
}

/// Longest side of a video preview: Telegram ignores bigger ones.
const THUMBNAIL_SIDE: i32 = 320;

/// A JPEG preview of a video to send with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub path: std::path::PathBuf,
    pub width: i32,
    pub height: i32,
}

/// A frame from about a second in (blocking). Telegram shows no picture for
/// a sent video unless the sender attaches one.
pub fn video_thumbnail(path: &std::path::Path, info: VideoInfo) -> Result<Thumbnail, String> {
    if info.width <= 0 || info.height <= 0 {
        return Err("no video stream".into());
    }
    let scale = (THUMBNAIL_SIDE as f64 / info.width.max(info.height) as f64).min(1.0);
    // Even sizes keep videoscale and the encoder happy.
    let even = |v: i32| ((v as f64 * scale).round() as i32).max(2) & !1;
    let (width, height) = (even(info.width), even(info.height));

    gst::init().map_err(|e| e.to_string())?;
    // `videoflip video-direction=auto` applies the rotation phones record.
    let description = format!(
        "uridecodebin uri=\"{}\" ! videoconvert ! videoflip video-direction=auto ! videoscale ! \
         video/x-raw,format=RGBx,width={width},height={height},pixel-aspect-ratio=1/1 ! \
         appsink name=sink sync=false max-buffers=1 drop=true",
        uri(&path.to_string_lossy())?
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| e.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "not a pipeline".to_string())?;
    let sink = pipeline
        .by_name("sink")
        .and_then(|s| s.downcast::<gstreamer_app::AppSink>().ok())
        .ok_or("no appsink")?;
    let result = (|| {
        let timeout = gst::ClockTime::from_seconds(10);
        pipeline.set_state(gst::State::Paused).map_err(|e| e.to_string())?;
        pipeline.state(timeout).0.map_err(|e| e.to_string())?;
        if info.duration >= 2 {
            pipeline
                .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, gst::ClockTime::from_seconds(1))
                .map_err(|e| e.to_string())?;
            pipeline.state(timeout).0.map_err(|e| e.to_string())?;
        }
        let sample = sink.try_pull_preroll(timeout).ok_or("no frame")?;
        let buffer = sample.buffer().ok_or("empty frame")?.map_readable().map_err(|e| e.to_string())?;
        // RGBx has no row padding: drop the x of every pixel.
        let rgb: Vec<u8> = buffer.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]).collect();
        if rgb.len() != (width * height * 3) as usize {
            return Err(format!("frame of {} bytes for {width}x{height}", rgb.len()));
        }
        let path = crate::clipboard::paste_path("jpg")?;
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 85)
            .encode(&rgb, width as u32, height as u32, image::ExtendedColorType::Rgb8)
            .map_err(|e| e.to_string())?;
        Ok(Thumbnail { path, width, height })
    })();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn probes_video_length_and_size() {
        let Some(sample) = crate::stream::tests::sample_mp4() else {
            eprintln!("skipped: cannot encode a sample video");
            return;
        };
        let info = probe_video(&sample).expect("probe");
        assert_eq!((info.width, info.height), (320, 240));
        assert_eq!(info.duration, 3);
    }

    #[test]
    fn makes_video_thumbnails() {
        let Some(sample) = crate::stream::tests::sample_mp4() else {
            eprintln!("skipped: cannot encode a sample video");
            return;
        };
        let info = probe_video(&sample).expect("probe");
        let thumb = video_thumbnail(&sample, info).expect("thumbnail");
        assert_eq!((thumb.width, thumb.height), (320, 240));
        let decoded = image::open(&thumb.path).expect("a JPEG");
        assert_eq!((decoded.width(), decoded.height()), (320, 240));
        let _ = std::fs::remove_file(&thumb.path);
        // Bigger videos are scaled down to 320 px on the long side.
        let big = VideoInfo { width: 1920, height: 1080, ..info };
        let thumb = video_thumbnail(&sample, big).expect("thumbnail");
        assert_eq!((thumb.width, thumb.height), (320, 180));
        let _ = std::fs::remove_file(&thumb.path);
    }

    #[test]
    fn variable_frame_rate_gets_one() {
        gst::init().unwrap();
        let caps = |rate: &str| gst::Caps::from_str(&format!("video/x-raw,format=NV12,framerate={rate}")).unwrap();
        let fixed = with_framerate(&caps("0/1")).expect("replaced");
        let rate = fixed.structure(0).unwrap().get::<gst::Fraction>("framerate").unwrap();
        assert_eq!(rate, gst::Fraction::new(30, 1));
        assert!(with_framerate(&caps("25/1")).is_none(), "real rates stay");
        let none = gst::Caps::from_str("video/x-raw,format=NV12").unwrap();
        assert!(with_framerate(&none).is_some());
    }

    #[test]
    fn speeds_round_trip() {
        let mut speeds = Speeds::default();
        speeds.set(SpeedKind::Voice, 1.75);
        speeds.set(SpeedKind::Round, 2.0);
        let back = Speeds::parse(&speeds.to_text());
        assert_eq!(back, speeds);
        assert_eq!(back.get(SpeedKind::Video), 1.0);
        // A file from before music had its own speed.
        let old = Speeds::parse("voice 1.75\nvideo 1\nround 2\n");
        assert_eq!((old.get(SpeedKind::Voice), old.get(SpeedKind::Music)), (1.75, 1.0));
        // Garbage and out-of-range values fall back to 1x.
        assert_eq!(Speeds::parse("voice 99\nvideo x\nnope 2"), Speeds::default());
    }

    #[test]
    fn speed_steps() {
        assert_eq!(faster(1.0), 1.25);
        assert_eq!(slower(1.0), 0.75);
        assert_eq!(faster(5.0), 5.0);
        assert_eq!(slower(0.25), 0.25);
        assert_eq!(faster(2.0), 2.25);
        assert_eq!(slower(3.0), 2.75);
        assert_eq!(faster(4.75), 5.0);
        assert_eq!(faster(1.6), 1.75);
        assert_eq!(slower(1.6), 1.5);
        assert_eq!(fmt_speed(1.5), "1.5x");
        assert_eq!(fmt_speed(0.25), "0.25x");
    }

    /// Counts the frames reaching the video sink from now on.
    fn count_frames(movie: &Movie) -> Arc<std::sync::atomic::AtomicUsize> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let sink: gst::Element = movie.video.pipeline().property("video-sink");
        let pad = sink
            .downcast::<gst::Bin>()
            .ok()
            .and_then(|bin| bin.by_name("iced_video"))
            .and_then(|sink| sink.static_pad("sink"))
            .unwrap();
        let counter = count.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            gst::PadProbeReturn::Ok
        });
        count
    }

    /// Frames keep coming: a few within 3 seconds. A frozen video shows
    /// one at most; a busy CI runner may take a while after a seek.
    fn keeps_playing(movie: &Movie) -> bool {
        let count = count_frames(movie);
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if count.load(std::sync::atomic::Ordering::SeqCst) > 3 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn plays_streamed_video_with_seek_and_speed() {
        let Some(path) = stream::tests::sample_mp4() else {
            eprintln!("skipping: cannot encode a sample video");
            return;
        };
        let size = std::fs::metadata(&path).unwrap().len();
        // Opening at another speed froze streamed videos on their first frame.
        let mut movie = open_stream(Arc::new(stream::tests::FileSource(path)), size, 1.5, 1.0).unwrap();
        assert_eq!(movie.video.size(), (320, 240));
        assert!(movie.video.duration() > Duration::from_secs(2), "{:?}", movie.video.duration());
        assert!(keeps_playing(&movie));
        movie.seek(Duration::from_millis(1000)).unwrap();
        // The rate change needs the position, which is unknown mid-seek.
        let _ = movie.video.pipeline().state(gst::ClockTime::from_seconds(5));
        assert!(keeps_playing(&movie));
        movie.set_speed(0.5).unwrap();
        assert!(keeps_playing(&movie));
        // Still at half speed after a seek.
        movie.seek(Duration::from_millis(1500)).unwrap();
        let _ = movie.video.pipeline().state(gst::ClockTime::from_seconds(5));
        let from = movie.video.position();
        std::thread::sleep(Duration::from_millis(400));
        let played = movie.video.position().saturating_sub(from);
        assert!(played < Duration::from_millis(330), "{played:?} in 400 ms at 0.5x");
    }

    #[test]
    fn reads_music_tags() {
        gst::init().unwrap();
        let path = std::env::temp_dir().join(format!("vimgram-test-{}.ogg", std::process::id()));
        let description = format!(
            "audiotestsrc num-buffers=100 ! audioconvert ! taginject tags=\"title=Song,artist=Band\" ! \
             vorbisenc ! oggmux ! filesink location={}",
            path.display()
        );
        // Without the encoder plugins there is nothing to probe.
        let Ok(pipeline) = gst::parse::launch(&description) else {
            return;
        };
        pipeline.set_state(gst::State::Playing).unwrap();
        let done = pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        assert_eq!(done.map(|m| m.type_()), Some(gst::MessageType::Eos));
        let info = probe_audio(&path);
        let _ = std::fs::remove_file(&path);
        let info = info.unwrap();
        assert_eq!((info.title.as_str(), info.performer.as_str()), ("Song", "Band"));
        assert_eq!(info.duration, 2);
    }

    #[test]
    fn plays_audio_with_speed() {
        let Some(path) = stream::tests::sample_mp4() else {
            return;
        };
        let mut audio = Audio::open(path.to_str().unwrap(), 2.0, 0.5, Duration::from_secs(1)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while audio.duration().is_none() && std::time::Instant::now() < deadline {
            audio.poll().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(audio.duration().is_some());
        // Started from 1s, not from the beginning: right after the seek
        // it is there already.
        while audio.start.is_some() && std::time::Instant::now() < deadline {
            audio.poll().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let soon = std::time::Instant::now() + Duration::from_millis(150);
        while audio.position() < Duration::from_millis(900) && std::time::Instant::now() < soon {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(audio.position() >= Duration::from_millis(900), "at {:?}", audio.position());
        // Seeking to a point (a click on the waveform, a dragged slider).
        audio.seek(Duration::from_secs(2));
        let soon = std::time::Instant::now() + Duration::from_millis(300);
        while audio.position() < Duration::from_millis(1900) && std::time::Instant::now() < soon {
            audio.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(audio.position() >= Duration::from_millis(1900), "at {:?}", audio.position());
        audio.set_speed(4.0);
        audio.seek_by(1.0);
        audio.toggle_pause();
        audio.poll().unwrap();
    }
}

