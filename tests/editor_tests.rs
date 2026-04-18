use freetasia::editor::{clip::Clip, export, project::Project, timeline::Timeline};
use std::path::PathBuf;

// ── Clip tests ──────────────────────────────────────────────────────────────

#[test]
fn clip_full_duration() {
    let clip = Clip::new(1, PathBuf::from("a.mp4"), 30.0, "Clip A");
    assert!((clip.duration() - 30.0).abs() < 1e-9);
}

#[test]
fn clip_trimmed_duration() {
    let mut clip = Clip::new(1, PathBuf::from("a.mp4"), 30.0, "Clip A");
    clip.trim_start = 5.0;
    clip.trim_end = 20.0;
    assert!((clip.duration() - 15.0).abs() < 1e-9);
}

#[test]
fn clip_timeline_placement() {
    let mut clip = Clip::new(1, PathBuf::from("a.mp4"), 10.0, "A");
    clip.timeline_start = 3.0;
    assert!((clip.timeline_end() - 13.0).abs() < 1e-9);
}

// ── Timeline tests ──────────────────────────────────────────────────────────

#[test]
fn timeline_clips_auto_placed() {
    let mut tl = Timeline::new();
    tl.add_clip(Clip::new(0, PathBuf::from("a.mp4"), 10.0, "A"));
    tl.add_clip(Clip::new(0, PathBuf::from("b.mp4"), 5.0, "B"));

    let clips = tl.clips();
    assert_eq!(clips.len(), 2);
    // Second clip placed at the end of the first.
    assert!((clips[1].timeline_start - 10.0).abs() < 1e-9);
    assert!((tl.total_duration() - 15.0).abs() < 1e-9);
}

#[test]
fn timeline_remove_by_id() {
    let mut tl = Timeline::new();
    let id = tl.add_clip(Clip::new(0, PathBuf::from("a.mp4"), 10.0, "A"));
    assert!(tl.remove_clip(id));
    assert!(tl.is_empty());
}

#[test]
fn timeline_playhead_clamped_to_zero() {
    let mut tl = Timeline::new();
    tl.set_playhead(-99.0);
    assert_eq!(tl.playhead, 0.0);
}

#[test]
fn timeline_playhead_clamped_to_duration() {
    let mut tl = Timeline::new();
    tl.add_clip(Clip::new(0, PathBuf::from("a.mp4"), 10.0, "A"));
    tl.set_playhead(9999.0);
    assert!((tl.playhead - 10.0).abs() < 1e-9);
}

// ── Project tests ───────────────────────────────────────────────────────────

#[test]
fn project_default_name() {
    let p = Project::default();
    assert!(!p.name.is_empty());
}

#[test]
fn project_save_load_roundtrip() {
    let dir = std::env::temp_dir().join("freetasia_integration_tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roundtrip.json");

    let mut p = Project::new("Integration Test");
    p.output_fps = 24;
    p.timeline
        .add_clip(Clip::new(0, PathBuf::from("clip.mp4"), 8.0, "Test"));

    p.save(&path).unwrap();

    let loaded = Project::load(&path).unwrap();
    assert_eq!(loaded.name, "Integration Test");
    assert_eq!(loaded.output_fps, 24);
    assert_eq!(loaded.timeline.clips().len(), 1);
    assert!((loaded.timeline.clips()[0].duration() - 8.0).abs() < 1e-9);

    std::fs::remove_file(&path).ok();
}

#[test]
fn project_default_output_name_replaces_spaces() {
    let p = Project::new("My Cool Project");
    assert_eq!(
        p.default_output_name(),
        PathBuf::from("My_Cool_Project.mp4")
    );
}

// ── Export tests ────────────────────────────────────────────────────────────

#[test]
fn export_empty_timeline_errors() {
    let tl = Timeline::new();
    let result = export::export_timeline(&tl, std::path::Path::new("/tmp/test_out.mp4"));
    assert!(result.is_err());
}

#[test]
fn ffmpeg_availability_check_does_not_panic() {
    let _ = export::ffmpeg_available();
}
