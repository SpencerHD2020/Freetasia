use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::{RecordingState, screen::ScreenRecorder, audio::AudioRecorder};

/// A completed recording session returned after stopping.
#[derive(Debug, Clone)]
pub struct RecordedSession {
    /// Path to the encoded video file (MP4).
    pub video_path: PathBuf,
    /// Path to the raw audio file (WAV), if audio recording was enabled.
    pub audio_path: Option<PathBuf>,
    /// Total wall-clock duration of the recording.
    pub duration: Duration,
}

/// Coordinates the screen and audio recorders and tracks recording time.
pub struct RecorderManager {
    state: RecordingState,
    screen: Option<ScreenRecorder>,
    audio: Option<AudioRecorder>,
    record_start: Option<Instant>,
    paused_accum: Duration,
    pause_start: Option<Instant>,
    /// Session settings remembered between calls.
    pub monitor_index: usize,
    pub fps: u32,
    pub record_audio: bool,
    /// Name of the microphone device to use (None = system default).
    pub mic_device_name: Option<String>,
    /// Base directory for temporary recording files.
    pub output_dir: PathBuf,
}

impl Default for RecorderManager {
    fn default() -> Self {
        Self {
            state: RecordingState::Idle,
            screen: None,
            audio: None,
            record_start: None,
            paused_accum: Duration::ZERO,
            pause_start: None,
            monitor_index: 0,
            fps: 60,
            record_audio: true,
            mic_device_name: None,
            output_dir: std::env::temp_dir().join("freetasia"),
        }
    }
}

impl RecorderManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> RecordingState {
        self.state
    }

    /// Elapsed recording time (paused intervals are excluded).
    pub fn elapsed(&self) -> Duration {
        match self.record_start {
            None => Duration::ZERO,
            Some(start) => {
                let total = start.elapsed();
                let paused = self.paused_accum
                    + self.pause_start.map(|ps| ps.elapsed()).unwrap_or_default();
                total.saturating_sub(paused)
            }
        }
    }

    /// Latest preview frame (if available).  Non-blocking.
    pub fn try_recv_frame(&self) -> Option<super::screen::FrameData> {
        self.screen.as_ref()?.preview_rx.try_recv().ok()
    }

    /// Width of the recorded screen (0 if no recording has started).
    pub fn frame_width(&self) -> u32 {
        self.screen.as_ref().map(|s| s.width).unwrap_or(0)
    }

    /// Height of the recorded screen (0 if no recording has started).
    pub fn frame_height(&self) -> u32 {
        self.screen.as_ref().map(|s| s.height).unwrap_or(0)
    }

    /// Paths used for the current (or most recent) session.
    pub fn current_video_path(&self) -> Option<&PathBuf> {
        self.screen.as_ref().map(|s| &s.output_path)
    }

    // ── State transitions ──────────────────────────────────────────────────

    /// Begin a new recording session.  Returns an error if already recording
    /// or if the screen/audio recorders fail to start.
    pub fn start_recording(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.state == RecordingState::Idle,
            "Already recording"
        );

        std::fs::create_dir_all(&self.output_dir)?;

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let video_path = self.output_dir.join(format!("screen_{timestamp}.mp4"));
        let audio_path = self.output_dir.join(format!("audio_{timestamp}.wav"));

        let screen = ScreenRecorder::start(self.monitor_index, self.fps, video_path)?;

        let audio = if self.record_audio {
            match AudioRecorder::start(audio_path, self.mic_device_name.as_deref()) {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("Audio recording unavailable: {e}");
                    None
                }
            }
        } else {
            None
        };

        self.screen = Some(screen);
        self.audio = audio;
        self.record_start = Some(Instant::now());
        self.paused_accum = Duration::ZERO;
        self.pause_start = None;
        self.state = RecordingState::Recording;
        Ok(())
    }

    /// Pause the current recording.
    pub fn pause_recording(&mut self) {
        if self.state != RecordingState::Recording {
            return;
        }
        if let Some(ref s) = self.screen {
            s.pause();
        }
        if let Some(ref a) = self.audio {
            a.pause();
        }
        self.pause_start = Some(Instant::now());
        self.state = RecordingState::Paused;
    }

    /// Resume a paused recording.
    pub fn resume_recording(&mut self) {
        if self.state != RecordingState::Paused {
            return;
        }
        if let Some(ref s) = self.screen {
            s.resume();
        }
        if let Some(ref a) = self.audio {
            a.resume();
        }
        if let Some(ps) = self.pause_start.take() {
            self.paused_accum += ps.elapsed();
        }
        self.state = RecordingState::Recording;
    }

    /// Stop recording and return session metadata.
    pub fn stop_recording(&mut self) -> Option<RecordedSession> {
        if self.state == RecordingState::Idle {
            return None;
        }

        // Accumulate any remaining pause time.
        if let Some(ps) = self.pause_start.take() {
            self.paused_accum += ps.elapsed();
        }

        let duration = self.elapsed();

        let video_path = self.screen.as_ref().map(|s| s.output_path.clone());
        let audio_path = self.audio.as_ref().map(|a| a.output_path.clone());

        // Stop recorders (finalises files).
        if let Some(mut s) = self.screen.take() {
            s.stop();
        }
        if let Some(mut a) = self.audio.take() {
            a.stop();
        }

        self.record_start = None;
        self.state = RecordingState::Idle;

        video_path.map(|vp| RecordedSession {
            video_path: vp,
            audio_path,
            duration,
        })
    }
}
