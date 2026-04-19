use serde::{Deserialize, Serialize};

/// A text callout that appears on the video at a specific time range and position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextOverlay {
    /// Unique identifier.
    pub id: u64,
    /// The text content to display.
    pub text: String,
    /// Start time on the timeline (seconds).
    pub start: f64,
    /// End time on the timeline (seconds).
    pub end: f64,
    /// Horizontal position as a fraction of video width (0.0 = left, 1.0 = right).
    pub x: f32,
    /// Vertical position as a fraction of video height (0.0 = top, 1.0 = bottom).
    pub y: f32,
    /// Font size in pixels (at 1080p; scaled proportionally for other resolutions).
    pub font_size: f32,
    /// Text colour as [R, G, B, A] in 0–255.
    pub color: [u8; 4],
}

impl TextOverlay {
    pub fn new(id: u64, text: impl Into<String>, start: f64, end: f64) -> Self {
        Self {
            id,
            text: text.into(),
            start,
            end,
            x: 0.5,
            y: 0.5,
            font_size: 48.0,
            color: [255, 255, 255, 255],
        }
    }

    /// Duration of the overlay in seconds.
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    /// Is the given timeline position inside this overlay's time range?
    pub fn visible_at(&self, t: f64) -> bool {
        t >= self.start && t < self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_duration() {
        let o = TextOverlay::new(1, "Hello", 2.0, 5.0);
        assert!((o.duration() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn overlay_visible_at() {
        let o = TextOverlay::new(1, "Hello", 2.0, 5.0);
        assert!(!o.visible_at(1.0));
        assert!(o.visible_at(2.0));
        assert!(o.visible_at(3.5));
        assert!(!o.visible_at(5.0));
    }

    #[test]
    fn overlay_defaults() {
        let o = TextOverlay::new(1, "Test", 0.0, 1.0);
        assert_eq!(o.x, 0.5);
        assert_eq!(o.y, 0.5);
        assert_eq!(o.font_size, 48.0);
        assert_eq!(o.color, [255, 255, 255, 255]);
    }
}
