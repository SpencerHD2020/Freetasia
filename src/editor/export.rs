use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use super::timeline::Timeline;

/// Build and run an ffmpeg command that realises the timeline as a single
/// output video file.
///
/// # Requirements
/// `ffmpeg` must be available on `PATH`.  The function checks for this first
/// and returns a helpful error if it is missing.
///
/// # Filter graph
/// For each clip the function emits an `[i:v]trim / setpts` filter and then
/// concatenates them all.  Audio from the first clip's paired WAV file (same
/// stem with `.wav` extension) is mixed in when present.
pub fn export_timeline(timeline: &Timeline, output_path: &Path) -> Result<()> {
    anyhow::ensure!(
        !timeline.is_empty(),
        "Cannot export an empty timeline"
    );

    // Verify ffmpeg is reachable.
    let ffmpeg = find_ffmpeg()?;

    let clips = timeline.clips();
    let n = clips.len();

    // ── Build the ffmpeg argument list ────────────────────────────────────
    let mut args: Vec<String> = Vec::new();

    // Input files: one per clip.
    for clip in clips {
        args.push("-i".into());
        args.push(clip.source_path.to_string_lossy().into_owned());
    }

    // Filter complex: trim each video stream, then concatenate.
    let mut filter = String::new();
    for (i, clip) in clips.iter().enumerate() {
        filter.push_str(&format!(
            "[{i}:v]trim=start={start}:end={end},setpts=PTS-STARTPTS[v{i}];",
            start = clip.trim_start,
            end = clip.trim_end,
        ));
    }
    // Concat video segments.
    for i in 0..n {
        filter.push_str(&format!("[v{i}]"));
    }
    filter.push_str(&format!("concat=n={n}:v=1:a=0[outv]"));

    args.push("-filter_complex".into());
    args.push(filter);
    args.push("-map".into());
    args.push("[outv]".into());

    // Optionally include audio from the first clip's paired WAV.
    let audio_path = clips[0]
        .source_path
        .with_extension("wav");
    if audio_path.exists() {
        args.push("-i".into());
        args.push(audio_path.to_string_lossy().into_owned());
        args.push("-map".into());
        args.push(format!("{}:a", n)); // index after video inputs
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-shortest".into());
    }

    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push("-y".into()); // overwrite output without asking
    args.push(output_path.to_string_lossy().into_owned());

    log::info!("Running: {} {}", ffmpeg, args.join(" "));

    let status = Command::new(&ffmpeg)
        .args(&args)
        .status()
        .context("Failed to spawn ffmpeg")?;

    anyhow::ensure!(status.success(), "ffmpeg exited with status {status}");
    Ok(())
}

/// Return the path to the ffmpeg executable, searching common locations.
pub fn find_ffmpeg() -> Result<String> {
    // Try plain name first (on PATH).
    if Command::new("ffmpeg").arg("-version").output().is_ok() {
        return Ok("ffmpeg".into());
    }
    // Common Windows install location.
    let win_path = r"C:\ffmpeg\bin\ffmpeg.exe";
    if Path::new(win_path).exists() {
        return Ok(win_path.into());
    }
    anyhow::bail!(
        "ffmpeg not found. Please install ffmpeg and add it to your PATH. \
         See https://ffmpeg.org/download.html"
    )
}

/// Return `true` if ffmpeg is available on this machine.
pub fn ffmpeg_available() -> bool {
    find_ffmpeg().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::timeline::Timeline;

    #[test]
    fn export_empty_timeline_errors() {
        let tl = Timeline::new();
        let result = export_timeline(&tl, Path::new("/tmp/out.mp4"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn ffmpeg_available_does_not_panic() {
        // We only check it doesn't panic; ffmpeg may or may not be installed.
        let _ = ffmpeg_available();
    }
}
