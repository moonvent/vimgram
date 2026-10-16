//! Voice message recording: microphone -> Opus in OGG, the format Telegram
//! uses for voice notes, plus the 5-bit waveform shown next to them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;

/// Level meter updates per second while recording.
const LEVEL_INTERVAL: Duration = Duration::from_millis(50);
/// Telegram clients display 100 waveform bars.
const WAVEFORM_SAMPLES: usize = 100;
/// Quietest level that still shows as a bar, in dB.
const FLOOR_DB: f64 = -50.0;

pub struct Recorder {
    pipeline: gst::Pipeline,
    path: PathBuf,
    started: Instant,
    /// Loudness in 0.0..=1.0, one value per `LEVEL_INTERVAL`.
    levels: Vec<f32>,
}

pub struct Recording {
    pub path: PathBuf,
    pub duration: i32,
    /// Packed 5-bit waveform, ready for TDLib.
    pub waveform: Vec<u8>,
}

impl Recorder {
    pub fn start(dir: PathBuf) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("voice-{stamp}.ogg"));

        let description = format!(
            "autoaudiosrc ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1 ! \
             level interval={} post-messages=true ! \
             opusenc bitrate=32000 audio-type=voice ! oggmux ! filesink location=\"{}\"",
            LEVEL_INTERVAL.as_nanos(),
            path.display().to_string().replace('"', "\\\"")
        );
        let pipeline = gst::parse::launch(&description)
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "recorder: not a pipeline".to_string())?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("cannot start microphone: {e}"))?;

        Ok(Recorder {
            pipeline,
            path,
            started: Instant::now(),
            levels: Vec::new(),
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    /// Collect pending level messages; returns an error if the pipeline failed.
    pub fn poll(&mut self) -> Result<(), String> {
        let Some(bus) = self.pipeline.bus() else {
            return Ok(());
        };
        while let Some(msg) = bus.pop() {
            self.handle(&msg)?;
        }
        Ok(())
    }

    fn handle(&mut self, msg: &gst::Message) -> Result<(), String> {
        match msg.view() {
            gst::MessageView::Element(element) => {
                if let Some(s) = element.structure().filter(|s| s.name() == "level")
                    && let Some(db) = rms_db(s)
                {
                    let level = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
                    self.levels.push(level as f32);
                }
                Ok(())
            }
            gst::MessageView::Error(err) => Err(format!("recording failed: {}", err.error())),
            _ => Ok(()),
        }
    }

    /// Finish the file properly (EOS flushes the OGG container) and return it.
    pub fn stop(mut self) -> Result<Recording, String> {
        let duration = self.elapsed();
        self.pipeline.send_event(gst::event::Eos::new());
        let bus = self.pipeline.bus().ok_or("recorder: no bus")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_nseconds(remaining.as_nanos() as u64))
            else {
                break;
            };
            if matches!(msg.view(), gst::MessageView::Eos(_)) {
                break;
            }
            if let Err(e) = self.handle(&msg) {
                let _ = self.pipeline.set_state(gst::State::Null);
                return Err(e);
            }
        }
        let _ = self.pipeline.set_state(gst::State::Null);

        Ok(Recording {
            path: self.path.clone(),
            duration: duration.as_secs_f64().round().max(1.0) as i32,
            waveform: pack_waveform(&waveform(&self.levels)),
        })
    }

    pub fn cancel(self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// RMS of the first channel in dB. Depending on the GStreamer version the
/// field is a `GValueArray` or a `GstValueArray`.
fn rms_db(s: &gst::StructureRef) -> Option<f64> {
    if let Ok(values) = s.get::<gst::glib::ValueArray>("rms") {
        return values.iter().next()?.get::<f64>().ok();
    }
    let values = s.get::<gst::Array>("rms").ok()?;
    values.iter().next()?.get::<f64>().ok()
}

/// Resample levels to `WAVEFORM_SAMPLES` bars of 0..=31, normalized to the
/// loudest bar like official clients do.
fn waveform(levels: &[f32]) -> Vec<u8> {
    if levels.is_empty() {
        return vec![0; WAVEFORM_SAMPLES];
    }
    let bars: Vec<f32> = (0..WAVEFORM_SAMPLES)
        .map(|i| {
            let start = i * levels.len() / WAVEFORM_SAMPLES;
            let end = ((i + 1) * levels.len() / WAVEFORM_SAMPLES).max(start + 1).min(levels.len());
            levels[start.min(levels.len() - 1)..end].iter().copied().fold(0.0, f32::max)
        })
        .collect();
    let peak = bars.iter().copied().fold(0.0, f32::max).max(f32::EPSILON);
    bars.iter().map(|b| (b / peak * 31.0).round() as u8).collect()
}

/// Pack 5-bit values little-endian, as Telegram expects.
fn pack_waveform(samples: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; (samples.len() * 5).div_ceil(8)];
    for (i, &v) in samples.iter().enumerate() {
        let bit = i * 5;
        let value = u16::from(v & 0x1f) << (bit % 8);
        out[bit / 8] |= value as u8;
        if let Some(next) = out.get_mut(bit / 8 + 1) {
            *next |= (value >> 8) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_five_bit_values() {
        // 31 then 1: bits 0..5 = 11111, bits 5..10 = 00001 -> 0b0011_1111, 0b0000_0000
        assert_eq!(pack_waveform(&[31, 1]), vec![0b0011_1111, 0b0000_0000]);
        assert_eq!(pack_waveform(&[0; 100]).len(), 63);
        assert_eq!(pack_waveform(&[1, 2, 3, 4, 5, 6, 7, 8]).len(), 5);
    }

    #[test]
    fn waveform_is_normalized() {
        let w = waveform(&[0.1, 0.5, 0.25]);
        assert_eq!(w.len(), WAVEFORM_SAMPLES);
        assert_eq!(*w.iter().max().unwrap(), 31);
    }
}
