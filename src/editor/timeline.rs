use serde::{Deserialize, Serialize};

use super::clip::Clip;

/// Manages the ordered collection of clips and the playhead position.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Timeline {
    clips: Vec<Clip>,
    next_id: u64,
    /// Current playhead position in seconds.
    pub playhead: f64,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a clip at the end of the timeline and return its assigned id.
    pub fn add_clip(&mut self, mut clip: Clip) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        clip.id = id;
        // Auto-place after the last clip.
        clip.timeline_start = self.total_duration();
        self.clips.push(clip);
        id
    }

    /// Remove the clip with the given id. Returns `true` if found.
    pub fn remove_clip(&mut self, id: u64) -> bool {
        let before = self.clips.len();
        self.clips.retain(|c| c.id != id);
        self.clips.len() < before
    }

    /// Return an immutable slice of all clips ordered by timeline position.
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// Return a mutable reference to a clip by id.
    pub fn clip_mut(&mut self, id: u64) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }

    /// Total duration of the timeline (end of the last clip), in seconds.
    pub fn total_duration(&self) -> f64 {
        self.clips
            .iter()
            .map(|c| c.timeline_end())
            .fold(0.0_f64, f64::max)
    }

    /// Move the playhead, clamped to [0, total_duration].
    pub fn set_playhead(&mut self, pos: f64) {
        self.playhead = pos.clamp(0.0, self.total_duration().max(0.0));
    }

    /// Is the timeline empty?
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dummy_clip(duration: f64) -> Clip {
        Clip::new(0, PathBuf::from("dummy.mp4"), duration, "Test")
    }

    #[test]
    fn add_and_remove() {
        let mut tl = Timeline::new();
        let id = tl.add_clip(dummy_clip(5.0));
        assert_eq!(tl.clips().len(), 1);
        assert!(tl.remove_clip(id));
        assert!(tl.clips().is_empty());
    }

    #[test]
    fn total_duration_accumulates() {
        let mut tl = Timeline::new();
        tl.add_clip(dummy_clip(10.0));
        tl.add_clip(dummy_clip(5.0));
        assert!((tl.total_duration() - 15.0).abs() < 1e-9);
    }

    #[test]
    fn playhead_clamped() {
        let mut tl = Timeline::new();
        tl.add_clip(dummy_clip(10.0));
        tl.set_playhead(99.0);
        assert!((tl.playhead - 10.0).abs() < 1e-9);
        tl.set_playhead(-5.0);
        assert!((tl.playhead).abs() < 1e-9);
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut tl = Timeline::new();
        assert!(!tl.remove_clip(42));
    }
}
