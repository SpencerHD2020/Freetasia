use serde::{Deserialize, Serialize};

/// The kind-specific data for each overlay effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OverlayKind {
    Text {
        text: String,
        /// Font size in pixels (at 1080p; scaled proportionally for other resolutions).
        font_size: f32,
        /// Text colour as [R, G, B, A] in 0–255.
        color: [u8; 4],
    },
    Blur {
        /// Width as a fraction of video width.
        width: f32,
        /// Height as a fraction of video height.
        height: f32,
    },
}

/// A video effect overlay that appears at a specific time range and position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overlay {
    /// Unique identifier (shared id-space with clips).
    pub id: u64,
    /// Start time on the timeline (seconds).
    pub start: f64,
    /// End time on the timeline (seconds).
    pub end: f64,
    /// Horizontal position as a fraction of video width (0.0 = left, 1.0 = right).
    /// For Text this is the centre; for Blur this is the left edge.
    pub x: f32,
    /// Vertical position as a fraction of video height (0.0 = top, 1.0 = bottom).
    /// For Text this is the centre; for Blur this is the top edge.
    pub y: f32,
    /// The effect-specific data.
    pub kind: OverlayKind,
}

impl Overlay {
    /// Create a new text overlay.
    pub fn new_text(id: u64, text: impl Into<String>, start: f64, end: f64) -> Self {
        Self {
            id,
            start,
            end,
            x: 0.5,
            y: 0.5,
            kind: OverlayKind::Text {
                text: text.into(),
                font_size: 48.0,
                color: [255, 255, 255, 255],
            },
        }
    }

    /// Create a new blur overlay.
    pub fn new_blur(id: u64, start: f64, end: f64) -> Self {
        Self {
            id,
            start,
            end,
            x: 0.35,
            y: 0.35,
            kind: OverlayKind::Blur {
                width: 0.3,
                height: 0.3,
            },
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

    pub fn is_text(&self) -> bool {
        matches!(self.kind, OverlayKind::Text { .. })
    }

    pub fn is_blur(&self) -> bool {
        matches!(self.kind, OverlayKind::Blur { .. })
    }

    /// Short display label for the timeline track.
    pub fn label(&self) -> String {
        match &self.kind {
            OverlayKind::Text { text, .. } => {
                if text.len() > 20 {
                    format!("{}…", &text[..19])
                } else {
                    text.clone()
                }
            }
            OverlayKind::Blur { .. } => "Blur".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_overlay_duration() {
        let o = Overlay::new_text(1, "Hello", 2.0, 5.0);
        assert!((o.duration() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn text_overlay_visible_at() {
        let o = Overlay::new_text(1, "Hello", 2.0, 5.0);
        assert!(!o.visible_at(1.0));
        assert!(o.visible_at(2.0));
        assert!(o.visible_at(3.5));
        assert!(!o.visible_at(5.0));
    }

    #[test]
    fn text_overlay_defaults() {
        let o = Overlay::new_text(1, "Test", 0.0, 1.0);
        assert_eq!(o.x, 0.5);
        assert_eq!(o.y, 0.5);
        assert!(o.is_text());
        if let OverlayKind::Text { font_size, color, .. } = &o.kind {
            assert_eq!(*font_size, 48.0);
            assert_eq!(*color, [255, 255, 255, 255]);
        }
    }

    #[test]
    fn blur_overlay_duration() {
        let b = Overlay::new_blur(1, 2.0, 5.0);
        assert!((b.duration() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn blur_overlay_visible_at() {
        let b = Overlay::new_blur(1, 2.0, 5.0);
        assert!(!b.visible_at(1.0));
        assert!(b.visible_at(2.0));
        assert!(b.visible_at(3.5));
        assert!(!b.visible_at(5.0));
    }

    #[test]
    fn blur_overlay_defaults() {
        let b = Overlay::new_blur(1, 0.0, 1.0);
        assert!(b.is_blur());
        if let OverlayKind::Blur { width, height } = &b.kind {
            assert_eq!(*width, 0.3);
            assert_eq!(*height, 0.3);
        }
    }

    #[test]
    fn label_text_short() {
        let o = Overlay::new_text(1, "Hi", 0.0, 1.0);
        assert_eq!(o.label(), "Hi");
    }

    #[test]
    fn label_text_truncated() {
        let o = Overlay::new_text(1, "A very long text that should be truncated", 0.0, 1.0);
        assert!(o.label().ends_with('…'));
        assert!(o.label().len() <= 22); // 19 chars + "…" (up to 3 bytes)
    }

    #[test]
    fn label_blur() {
        let b = Overlay::new_blur(1, 0.0, 1.0);
        assert_eq!(b.label(), "Blur");
    }
}
