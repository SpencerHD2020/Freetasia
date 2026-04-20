use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use super::timeline::Timeline;

/// Apply platform-specific flags to hide the console window on Windows.
#[cfg(target_os = "windows")]
fn hide_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_console_window(_cmd: &mut Command) {}

/// Progress state shared between the export thread and the UI.
#[derive(Clone, Debug)]
pub enum ExportProgress {
    /// Fraction in 0.0..=1.0.
    Progress(f32),
    /// Export finished successfully.
    Done,
    /// Export failed with an error message.
    Error(String),
}

/// Build the ffmpeg argument list for a timeline export (internal helper).
fn build_ffmpeg_args(timeline: &Timeline, output_path: &Path) -> Result<(String, Vec<String>)> {
    anyhow::ensure!(
        !timeline.is_empty(),
        "Cannot export an empty timeline"
    );

    let ffmpeg = find_ffmpeg()?;
    let clips = timeline.clips();
    let n = clips.len();

    let mut args: Vec<String> = Vec::new();

    // Input files: one per clip.
    for clip in clips {
        args.push("-i".into());
        args.push(clip.source_path.to_string_lossy().into_owned());
    }

    // Filter complex: trim each video stream, then concatenate.
    let mut filter = String::new();
    for (i, clip) in clips.iter().enumerate() {
        let speed_factor = 1.0 / clip.speed;
        filter.push_str(&format!(
            "[{i}:v]trim=start={start}:end={end},setpts={speed_factor}*(PTS-STARTPTS)[v{i}];",
            start = clip.trim_start,
            end = clip.trim_end,
        ));
    }
    for i in 0..n {
        filter.push_str(&format!("[v{i}]"));
    }
    filter.push_str(&format!("concat=n={n}:v=1:a=0[concatv]"));

    // Apply overlays: text (drawtext), then blur (split+crop+boxblur+overlay).
    use super::overlay::OverlayKind;
    let all_overlays = timeline.overlays();
    let text_overlays: Vec<_> = all_overlays.iter().filter(|o| o.is_text()).collect();
    let blur_overlays: Vec<_> = all_overlays.iter().filter(|o| o.is_blur()).collect();
    let after_text_label = if blur_overlays.is_empty() { "outv" } else { "textv" };

    if text_overlays.is_empty() {
        filter.push_str(&format!(";[concatv]null[{after_text_label}]"));
    } else {
        let mut prev_label = "concatv".to_string();
        for (i, overlay) in text_overlays.iter().enumerate() {
            let next_label = if i == text_overlays.len() - 1 {
                after_text_label.to_string()
            } else {
                format!("txt{i}")
            };
            if let OverlayKind::Text { ref text, font_size, color } = overlay.kind {
                let escaped_text = text
                    .replace('\\', "\\\\\\\\")
                    .replace('\'', "'\\\\\\''")
                    .replace(':', "\\\\:");
                let r = color[0];
                let g = color[1];
                let b = color[2];
                let a = color[3];
                let fontcolor = format!("#{r:02x}{g:02x}{b:02x}{a:02x}");
                let x_expr = format!("w*{}-tw/2", overlay.x);
                let y_expr = format!("h*{}-th/2", overlay.y);
                filter.push_str(&format!(
                    ";[{prev_label}]drawtext=text='{escaped_text}'\
                     :fontsize={fs}:fontcolor={fontcolor}\
                     :x='{x_expr}':y='{y_expr}'\
                     :enable='between(t,{start},{end})'[{next_label}]",
                    fs = font_size as u32,
                    start = overlay.start,
                    end = overlay.end,
                ));
            }
            prev_label = next_label;
        }
    }

    // Apply blur overlays using split → crop → boxblur → overlay.
    if !blur_overlays.is_empty() {
        let mut prev_label = after_text_label.to_string();
        for (i, blur) in blur_overlays.iter().enumerate() {
            let next_label = if i == blur_overlays.len() - 1 {
                "outv".to_string()
            } else {
                format!("blr{i}")
            };
            if let OverlayKind::Blur { width, height } = &blur.kind {
                filter.push_str(&format!(
                    ";[{prev_label}]split=2[bbase{i}][bsrc{i}]\
                     ;[bsrc{i}]crop=\
                     floor(iw*{w}/2)*2:floor(ih*{h}/2)*2:\
                     floor(iw*{x}):floor(ih*{y})\
                     ,boxblur=8:5\
                     :enable='between(t,{start},{end})'\
                     ,format=yuv420p\
                     [bblur{i}]\
                     ;[bbase{i}][bblur{i}]overlay=\
                     floor(W*{x}):floor(H*{y})\
                     :shortest=1[{next_label}]",
                    w = width,
                    h = height,
                    x = blur.x,
                    y = blur.y,
                    start = blur.start,
                    end = blur.end,
                ));
            }
            prev_label = next_label;
        }
    }

    args.push("-filter_complex".into());
    args.push(filter);
    args.push("-map".into());
    args.push("[outv]".into());

    // Optionally include audio from the first clip's paired WAV.
    let audio_path = clips[0].source_path.with_extension("wav");
    if audio_path.exists() {
        args.push("-i".into());
        args.push(audio_path.to_string_lossy().into_owned());
        args.push("-map".into());
        args.push(format!("{}:a", n));
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-shortest".into());
    }

    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    // Force constant frame rate output so that sped-up segments don't
    // cause the container to report an incorrect duration / frame rate.
    args.push("-r".into());
    args.push("60".into());
    args.push("-vsync".into());
    args.push("cfr".into());
    // Request progress output on stderr.
    args.push("-progress".into());
    args.push("pipe:2".into());
    args.push("-y".into());
    args.push(output_path.to_string_lossy().into_owned());

    Ok((ffmpeg, args))
}

/// Build and run an ffmpeg command that realises the timeline as a single
/// output video file (synchronous / blocking).
pub fn export_timeline(timeline: &Timeline, output_path: &Path) -> Result<()> {
    let (ffmpeg, args) = build_ffmpeg_args(timeline, output_path)?;

    log::info!("Running: {} {}", ffmpeg, args.join(" "));

    let mut cmd = Command::new(&ffmpeg);
    cmd.args(&args);
    hide_console_window(&mut cmd);
    let status = cmd
        .status()
        .context("Failed to spawn ffmpeg")?;

    anyhow::ensure!(status.success(), "ffmpeg exited with status {status}");
    Ok(())
}

/// Spawn the export in a background thread, sending progress updates through
/// a `crossbeam_channel::Sender<ExportProgress>`.
pub fn export_timeline_async(
    timeline: &Timeline,
    output_path: &Path,
    progress_tx: crossbeam_channel::Sender<ExportProgress>,
) -> Result<()> {
    let total_duration = timeline.total_duration();
    let (ffmpeg, args) = build_ffmpeg_args(timeline, output_path)?;

    log::info!("Running (async): {} {}", ffmpeg, args.join(" "));

    let ffmpeg_owned = ffmpeg.clone();
    let args_owned = args.clone();

    std::thread::spawn(move || {
        let mut cmd = Command::new(&ffmpeg_owned);
        cmd.args(&args_owned);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        hide_console_window(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = progress_tx.send(ExportProgress::Error(format!("Failed to spawn ffmpeg: {e}")));
                return;
            }
        };

        let stderr = child.stderr.take().unwrap();
        let reader = BufReader::new(stderr);

        let mut last_error_lines: Vec<String> = Vec::new();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            // ffmpeg -progress outputs lines like: out_time_us=12345678
            if let Some(time_us_str) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = time_us_str.trim().parse::<i64>() {
                    if us >= 0 && total_duration > 0.0 {
                        let frac = (us as f64 / 1_000_000.0 / total_duration) as f32;
                        let _ = progress_tx.send(ExportProgress::Progress(frac.clamp(0.0, 1.0)));
                    }
                }
            } else if !line.trim().is_empty() {
                // Keep last few non-progress lines for error reporting.
                last_error_lines.push(line);
                if last_error_lines.len() > 20 {
                    last_error_lines.remove(0);
                }
            }
        }

        match child.wait() {
            Ok(status) if status.success() => {
                let _ = progress_tx.send(ExportProgress::Done);
            }
            Ok(status) => {
                let detail = if last_error_lines.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", last_error_lines.join("\n"))
                };
                let _ = progress_tx.send(ExportProgress::Error(
                    format!("ffmpeg exited with status {status}{detail}"),
                ));
            }
            Err(e) => {
                let _ = progress_tx.send(ExportProgress::Error(format!("ffmpeg wait error: {e}")));
            }
        }
    });

    Ok(())
}

/// Return the path to the ffmpeg executable, searching common locations.
///
/// Search order:
/// 1. Bundled `ffmpeg.exe` next to the running executable (for distribution).
/// 2. System `PATH`.
/// 3. Common Windows install location `C:\ffmpeg\bin\`.
pub fn find_ffmpeg() -> Result<String> {
    // 1. Check next to our own executable (bundled distribution).
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let bundled = exe_dir.join("ffmpeg.exe");
            if bundled.exists() {
                return Ok(bundled.to_string_lossy().into_owned());
            }
            // Also check an `ffmpeg/` subdirectory next to the exe.
            let subdir = exe_dir.join("ffmpeg").join("ffmpeg.exe");
            if subdir.exists() {
                return Ok(subdir.to_string_lossy().into_owned());
            }
        }
    }

    // 2. Try plain name (on PATH).
    let mut probe = Command::new("ffmpeg");
    probe.arg("-version");
    hide_console_window(&mut probe);
    if probe.output().is_ok() {
        return Ok("ffmpeg".into());
    }

    // 3. Common Windows install location.
    let win_path = r"C:\ffmpeg\bin\ffmpeg.exe";
    if Path::new(win_path).exists() {
        return Ok(win_path.into());
    }

    anyhow::bail!(
        "ffmpeg not found. Place ffmpeg.exe next to the Freetasia executable, \
         or install ffmpeg and add it to your PATH. \
         See https://ffmpeg.org/download.html"
    )
}

/// Return the path to the ffprobe executable, searching common locations.
///
/// Uses the same search strategy as [`find_ffmpeg`].
pub fn find_ffprobe() -> Result<String> {
    // 1. Check next to our own executable (bundled distribution).
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let bundled = exe_dir.join("ffprobe.exe");
            if bundled.exists() {
                return Ok(bundled.to_string_lossy().into_owned());
            }
            let subdir = exe_dir.join("ffmpeg").join("ffprobe.exe");
            if subdir.exists() {
                return Ok(subdir.to_string_lossy().into_owned());
            }
        }
    }

    // 2. Try plain name (on PATH).
    let mut probe = Command::new("ffprobe");
    probe.arg("-version");
    hide_console_window(&mut probe);
    if probe.output().is_ok() {
        return Ok("ffprobe".into());
    }

    // 3. Common Windows install location.
    let win_path = r"C:\ffmpeg\bin\ffprobe.exe";
    if Path::new(win_path).exists() {
        return Ok(win_path.into());
    }

    anyhow::bail!(
        "ffprobe not found. Place ffprobe.exe next to the Freetasia executable, \
         or install ffmpeg and add it to your PATH. \
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
