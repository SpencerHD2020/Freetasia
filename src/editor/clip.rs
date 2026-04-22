use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single media segment placed on the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    /// Unique identifier (incrementing integer).
    pub id: u64,
    /// Path to the source media file (video or audio).
    pub source_path: PathBuf,
    /// Start of the used portion within the source file, in seconds.
    pub trim_start: f64,
    /// End of the used portion within the source file, in seconds.
    pub trim_end: f64,
    /// Position of this clip on the project timeline, in seconds.
    pub timeline_start: f64,
    /// Human-readable label shown in the timeline.
    pub label: String,
    /// Playback speed multiplier (1.0 = normal, 2.0 = 2× fast, 0.5 = half speed).
    #[serde(default = "default_speed")]
    pub speed: f64,
    /// Optional path to a paired audio file (WAV) to mux into the export.
    #[serde(default)]
    pub audio_path: Option<PathBuf>,
}

fn default_speed() -> f64 {
    1.0
}

impl Clip {
    /// Create a new clip that uses the full duration of a source file.
    pub fn new(id: u64, source_path: PathBuf, duration: f64, label: impl Into<String>) -> Self {
        Self {
            id,
            source_path,
            trim_start: 0.0,
            trim_end: duration,
            timeline_start: 0.0,
            label: label.into(),
            speed: 1.0,
            audio_path: None,
        }
    }

    /// Duration of the source segment before speed adjustment, in seconds.
    pub fn source_duration(&self) -> f64 {
        (self.trim_end - self.trim_start).max(0.0)
    }

    /// Duration of the clip on the timeline after speed adjustment, in seconds.
    pub fn duration(&self) -> f64 {
        (self.source_duration() / self.speed).max(0.0)
    }

    /// End position of this clip on the project timeline, in seconds.
    pub fn timeline_end(&self) -> f64 {
        self.timeline_start + self.duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_duration_full() {
        let clip = Clip::new(1, PathBuf::from("test.mp4"), 10.0, "Test");
        assert!((clip.duration() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn clip_duration_trimmed() {
        let mut clip = Clip::new(1, PathBuf::from("test.mp4"), 10.0, "Test");
        clip.trim_start = 2.0;
        clip.trim_end = 7.0;
        assert!((clip.duration() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn clip_timeline_end() {
        let mut clip = Clip::new(1, PathBuf::from("test.mp4"), 10.0, "Test");
        clip.timeline_start = 5.0;
        assert!((clip.timeline_end() - 15.0).abs() < 1e-9);
    }

    #[test]
    fn clip_duration_never_negative() {
        let mut clip = Clip::new(1, PathBuf::from("test.mp4"), 10.0, "Test");
        clip.trim_start = 9.0;
        clip.trim_end = 5.0; // reversed – should clamp to 0
        assert_eq!(clip.duration(), 0.0);
    }
}
