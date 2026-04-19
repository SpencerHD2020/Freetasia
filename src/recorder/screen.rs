use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};

use crate::editor::export::find_ffmpeg;

/// Apply platform-specific flags to hide the console window on Windows.
#[cfg(target_os = "windows")]
fn hide_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_console_window(_cmd: &mut Command) {}

/// A single captured video frame together with its presentation timestamp.
#[derive(Clone)]
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    /// Raw RGBA pixel data (row-major, top-to-bottom).
    pub rgba: Vec<u8>,
    /// Milliseconds since the recording session started.
    pub timestamp_ms: u64,
}

/// Handle to a running screen-capture session.
pub struct ScreenRecorder {
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Receive the most-recent frame for live preview.
    pub preview_rx: Receiver<FrameData>,
    /// Path where the captured video is being written.
    pub output_path: PathBuf,
    pub width: u32,
    pub height: u32,
}

impl ScreenRecorder {
    /// Start recording the given monitor index at `fps` frames per second.
    ///
    /// Video is encoded by piping raw RGBA frames to an `ffmpeg` process that
    /// writes to `output_path` (MP4 / libx264).  If ffmpeg is unavailable the
    /// function returns an error before spawning any threads.
    pub fn start(
        monitor_index: usize,
        fps: u32,
        output_path: PathBuf,
    ) -> Result<Self> {
        // Probe screen dimensions before spawning threads.
        let (width, height) = probe_monitor_size(monitor_index)?;

        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let (preview_tx, preview_rx) = bounded::<FrameData>(2);

        let running_clone = running.clone();
        let paused_clone = paused.clone();
        let out_path_clone = output_path.clone();

        let thread = thread::Builder::new()
            .name("screen-capture".into())
            .spawn(move || {
                if let Err(e) = capture_loop(
                    running_clone,
                    paused_clone,
                    preview_tx,
                    monitor_index,
                    fps,
                    width,
                    height,
                    out_path_clone,
                ) {
                    log::error!("Screen capture thread error: {e}");
                }
            })
            .context("Failed to spawn screen-capture thread")?;

        Ok(Self {
            running,
            paused,
            thread: Some(thread),
            preview_rx,
            output_path,
            width,
            height,
        })
    }

    /// Pause capture (frames are skipped but the thread keeps running).
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume a paused capture.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Signal the capture thread to stop and wait for it to finish.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ScreenRecorder {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Internals ──────────────────────────────────────────────────────────────

/// Attempt to find the pixel dimensions of a monitor by briefly capturing it.
fn probe_monitor_size(monitor_index: usize) -> Result<(u32, u32)> {
    use screenshots::Screen;
    let screens = Screen::all().map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(!screens.is_empty(), "No screens found");
    let screen = &screens[monitor_index.min(screens.len() - 1)];
    let image = screen.capture().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((image.width(), image.height()))
}

/// The main loop running on the capture thread.
fn capture_loop(
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    preview_tx: Sender<FrameData>,
    monitor_index: usize,
    fps: u32,
    width: u32,
    height: u32,
    output_path: PathBuf,
) -> Result<()> {
    use screenshots::Screen;

    let frame_interval = Duration::from_secs(1) / fps.max(1);
    let start = Instant::now();

    // Spawn ffmpeg to encode frames written to its stdin.
    let mut ffmpeg = spawn_ffmpeg_encoder(width, height, fps, &output_path)?;
    let mut stdin: Option<ChildStdin> = ffmpeg.stdin.take();

    let screens = Screen::all().map_err(|e| anyhow::anyhow!("{e}"))?;
    let screen = screens
        .into_iter()
        .nth(monitor_index)
        .unwrap_or_else(|| {
            // Safety: probe_monitor_size already verified at least one screen exists.
            Screen::all().unwrap().remove(0)
        });

    while running.load(Ordering::SeqCst) {
        let loop_start = Instant::now();

        if !paused.load(Ordering::SeqCst) {
            match screen.capture() {
                Ok(image) => {
                    let rgba = image.into_raw();
                    let timestamp_ms = start.elapsed().as_millis() as u64;

                    // Write raw RGBA bytes to ffmpeg stdin (non-blocking on failure).
                    if let Some(ref mut s) = stdin {
                        if s.write_all(&rgba).is_err() {
                            log::warn!("ffmpeg stdin closed; stopping capture");
                            break;
                        }
                    }

                    // Best-effort preview update (drop old frame if UI is slow).
                    let frame = FrameData {
                        width,
                        height,
                        rgba,
                        timestamp_ms,
                    };
                    let _ = preview_tx.try_send(frame);
                }
                Err(e) => log::warn!("Capture error: {e}"),
            }
        }

        let elapsed = loop_start.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }

    // Close stdin so ffmpeg flushes and exits cleanly.
    drop(stdin);
    let _ = ffmpeg.wait();

    Ok(())
}

/// Spawn an `ffmpeg` process that reads raw RGBA frames on stdin and encodes
/// them to `output_path` using libx264.
fn spawn_ffmpeg_encoder(
    width: u32,
    height: u32,
    fps: u32,
    output_path: &Path,
) -> Result<Child> {
    let size = format!("{width}x{height}");
    let fps_str = fps.to_string();
    let out = output_path.to_string_lossy();

    let ffmpeg_path = find_ffmpeg()?;
    let mut cmd = Command::new(&ffmpeg_path);
    cmd.args([
            "-y",
            "-f", "rawvideo",
            "-pixel_format", "rgba",
            "-video_size", &size,
            "-framerate", &fps_str,
            "-i", "pipe:0",
            "-c:v", "libx264",
            "-pix_fmt", "yuv420p",
            "-preset", "ultrafast",
            out.as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut cmd);
    cmd.spawn()
        .context("Failed to spawn ffmpeg — is it installed and on PATH?")
}
