pub mod audio;
pub mod manager;
pub mod screen;

/// Current recording state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Nothing is recording.
    Idle,
    /// Actively capturing screen and/or audio.
    Recording,
    /// Capture is temporarily suspended.
    Paused,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::Idle
    }
}

impl std::fmt::Display for RecordingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Recording => write!(f, "Recording"),
            Self::Paused => write!(f, "Paused"),
        }
    }
}
