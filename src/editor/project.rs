use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::timeline::Timeline;

/// Top-level project file that can be saved/loaded as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub timeline: Timeline,
    /// Desired output frame rate for export.
    pub output_fps: u32,
    /// Desired output resolution (width, height).
    pub output_resolution: (u32, u32),
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: "Untitled Project".into(),
            timeline: Timeline::new(),
            output_fps: 30,
            output_resolution: (1920, 1080),
        }
    }
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Serialise the project to a JSON file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .context("Failed to serialise project")?;
        fs::write(path, json).with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Deserialise a project from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let json =
            fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
        let project: Self =
            serde_json::from_str(&json).context("Failed to deserialise project")?;
        Ok(project)
    }

    /// Suggested output file name derived from the project name.
    pub fn default_output_name(&self) -> PathBuf {
        PathBuf::from(format!("{}.mp4", self.name.replace(' ', "_")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::editor::clip::Clip;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("freetasia_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_project.json");

        let mut project = Project::new("My Test");
        project.output_fps = 24;
        project.timeline.add_clip(Clip::new(
            0,
            PathBuf::from("clip.mp4"),
            10.0,
            "Test Clip",
        ));

        project.save(&path).unwrap();
        let loaded = Project::load(&path).unwrap();

        assert_eq!(loaded.name, "My Test");
        assert_eq!(loaded.output_fps, 24);
        assert_eq!(loaded.timeline.clips().len(), 1);
        assert_eq!(loaded.timeline.clips()[0].label, "Test Clip");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn default_output_name() {
        let p = Project::new("My Cool Project");
        assert_eq!(
            p.default_output_name(),
            PathBuf::from("My_Cool_Project.mp4")
        );
    }
}
