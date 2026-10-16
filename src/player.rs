//! Playback with adjustable speed. Both voice messages and videos run
//! through `scaletempo`, so speeding up keeps the pitch of voices.

use std::sync::Arc;
use std::time::Duration;

use futures::executor::block_on;
use gstreamer as gst;
use gstreamer::prelude::*;
use iced_video_player::Video;

use crate::{stream, tg};

/// Speeds reachable with `[` / `]`.
pub const SPEEDS: [f64; 12] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

pub fn faster(speed: f64) -> f64 {
    SPEEDS.into_iter().find(|&s| s > speed + 1e-6).unwrap_or(SPEEDS[SPEEDS.len() - 1])
}

pub fn slower(speed: f64) -> f64 {
    SPEEDS.into_iter().rev().find(|&s| s < speed - 1e-6).unwrap_or(SPEEDS[0])
}

pub fn fmt_speed(speed: f64) -> String {
    format!("{}x", (speed * 100.0).round() / 100.0)
}

fn scaletempo() -> Result<gst::Element, String> {
    gst::ElementFactory::make("scaletempo")
        .build()
        .map_err(|e| format!("scaletempo: {e}"))
}

fn uri(path: &str) -> Result<url::Url, String> {
    url::Url::from_file_path(path).map_err(|()| format!("bad path: {path}"))
}

/// A video being played, and the stream feeding it.
pub struct Movie {
    pub video: Video,
    _stream: Option<stream::Handle>,
}

/// A video with sound for the full-screen viewer, streamed while it
/// downloads. Blocks until playback can start, so call it off the UI thread.
pub fn open_video(file_id: i32, speed: f64) -> Result<Movie, String> {
    gst::init().map_err(|e| e.to_string())?;
    let size = block_on(tg::file_size(file_id))?;
    if size > 0 {
        stream::wait_for_start(file_id)?;
        return open_stream(Arc::new(stream::Telegram(file_id)), size, speed);
    }
    // Without a known size the appsrc cannot seek; download the whole file.
    let path = block_on(tg::download(file_id, 32))?;
    build(uri(&path)?.as_str(), None, speed)
}

/// Play `size` bytes of video from `source`.
pub fn open_stream(source: Arc<dyn stream::Source>, size: u64, speed: f64) -> Result<Movie, String> {
    build(stream::URI, Some((source, size)), speed)
}

/// Same pipeline as `Video::new`, plus a pitch-preserving audio filter.
fn build(uri: &str, source: Option<(Arc<dyn stream::Source>, u64)>, speed: f64) -> Result<Movie, String> {
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
    let stream = source.map(|(source, size)| stream::attach(pipeline.upcast_ref(), source, size));

    let sink_bin: gst::Element = pipeline.property("video-sink");
    let video_sink = sink_bin
        .downcast::<gst::Bin>()
        .ok()
        .and_then(|bin| bin.by_name("iced_video"))
        .and_then(|sink| sink.downcast::<gstreamer_app::AppSink>().ok())
        .ok_or_else(|| "video: no appsink".to_string())?;

    let mut video = Video::from_gst_pipeline(pipeline, video_sink, None).map_err(|e| e.to_string())?;
    if (speed - 1.0).abs() > 1e-6 {
        let _ = video.set_speed(speed);
    }
    Ok(Movie {
        video,
        _stream: stream,
    })
}

/// Audio-only playback (voice messages).
pub struct Audio {
    pipeline: gst::Pipeline,
    speed: f64,
    /// The rate seek needs a known position, so it waits for preroll.
    rate_pending: bool,
    finished: bool,
}

impl Audio {
    pub fn open(path: &str, speed: f64) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri(path)?.as_str())
            .property("audio-filter", scaletempo()?)
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
        if self.rate_pending && self.seek_to(self.position()).is_ok() {
            self.rate_pending = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_steps() {
        assert_eq!(faster(1.0), 1.25);
        assert_eq!(slower(1.0), 0.75);
        assert_eq!(faster(5.0), 5.0);
        assert_eq!(slower(0.25), 0.25);
        assert_eq!(fmt_speed(1.5), "1.5x");
        assert_eq!(fmt_speed(0.25), "0.25x");
    }

    #[test]
    fn plays_streamed_video_with_seek_and_speed() {
        let Some(path) = stream::tests::sample_mp4() else {
            eprintln!("skipping: cannot encode a sample video");
            return;
        };
        let size = std::fs::metadata(&path).unwrap().len();
        let mut movie = open_stream(Arc::new(stream::tests::FileSource(path)), size, 2.0).unwrap();
        let video = &mut movie.video;
        assert_eq!(video.size(), (320, 240));
        assert!(video.duration() > Duration::from_secs(2), "{:?}", video.duration());
        video.seek(Duration::from_millis(1500), true).unwrap();
        // The rate change needs the position, which is unknown mid-seek.
        let _ = video.pipeline().state(gst::ClockTime::from_seconds(5));
        video.set_speed(5.0).unwrap();
        assert_eq!(video.speed(), 5.0);
        // A speed change is a seek too.
        let _ = video.pipeline().state(gst::ClockTime::from_seconds(5));
        video.set_speed(0.25).unwrap();
        assert_eq!(video.speed(), 0.25);
    }

    #[test]
    fn plays_audio_with_speed() {
        let Some(path) = stream::tests::sample_mp4() else {
            return;
        };
        let mut audio = Audio::open(path.to_str().unwrap(), 2.0).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while audio.duration().is_none() && std::time::Instant::now() < deadline {
            audio.poll().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(audio.duration().is_some());
        audio.set_speed(4.0);
        audio.seek_by(1.0);
        audio.toggle_pause();
        audio.poll().unwrap();
    }
}

