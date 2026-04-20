use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};

use super::export::{find_ffmpeg, find_ffprobe};

/// Apply platform-specific flags to hide the console window on Windows.
#[cfg(target_os = "windows")]
fn hide_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_console_window(_cmd: &mut Command) {}

/// A decoded video frame for UI display.
#[derive(Clone)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Position on the timeline in seconds.
    pub timeline_pos: f64,
}

/// Playback state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self::Stopped
    }
}

/// Segment descriptor passed to the decode thread.
#[derive(Clone)]
struct PlaySegment {
    source_path: PathBuf,
    trim_start: f64,
    trim_duration: f64,
    speed: f64,
    timeline_start: f64,
    timeline_duration: f64,
}

/// Buffered scrub request for when a decode is already in flight.
#[derive(Clone)]
struct ScrubRequest {
    segments: Vec<(PathBuf, f64, f64, f64, f64, f64)>,
    position: f64,
    width: u32,
    height: u32,
}

struct PlaybackClock {
    start_instant: Instant,
    paused: AtomicBool,
    paused_accum: Mutex<Duration>,
    pause_instant: Mutex<Option<Instant>>,
}

impl PlaybackClock {
    fn new() -> Self {
        Self {
            start_instant: Instant::now(),
            paused: AtomicBool::new(false),
            paused_accum: Mutex::new(Duration::ZERO),
            pause_instant: Mutex::new(None),
        }
    }

    fn lock_paused_accum(&self) -> std::sync::MutexGuard<'_, Duration> {
        self.paused_accum.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_pause_instant(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.pause_instant.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn active_elapsed(&self) -> Duration {
        let paused_accum = *self.lock_paused_accum();
        let current_pause = self
            .lock_pause_instant()
            .as_ref()
            .map(|instant| instant.elapsed())
            .unwrap_or_default();
        self.start_instant
            .elapsed()
            .saturating_sub(paused_accum + current_pause)
    }

    fn current_position(&self, start_pos: f64, end_pos: f64) -> f64 {
        (start_pos + self.active_elapsed().as_secs_f64()).min(end_pos)
    }

    fn pause(&self) {
        if !self.paused.swap(true, Ordering::SeqCst) {
            *self.lock_pause_instant() = Some(Instant::now());
        }
    }

    fn resume(&self) {
        if self.paused.swap(false, Ordering::SeqCst) {
            let mut pause_instant = self.lock_pause_instant();
            if let Some(instant) = pause_instant.take() {
                *self.lock_paused_accum() += instant.elapsed();
            }
        }
    }

    fn wait_for_timeline_pos(
        &self,
        running: &AtomicBool,
        start_pos: f64,
        timeline_pos: f64,
    ) -> bool {
        let target_elapsed = Duration::from_secs_f64((timeline_pos - start_pos).max(0.0));

        while running.load(Ordering::Relaxed) {
            if self.paused.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(10));
                continue;
            }

            let actual = self.active_elapsed();
            if actual >= target_elapsed {
                return true;
            }

            thread::sleep((target_elapsed - actual).min(Duration::from_millis(10)));
        }

        false
    }
}

/// Video playback engine – decodes clips via ffmpeg and streams RGBA frames.
pub struct VideoPlayer {
    state: PlaybackState,
    decode_thread: Option<JoinHandle<()>>,
    frame_rx: Option<Receiver<DecodedFrame>>,
    running: Arc<AtomicBool>,
    playback_clock: Option<Arc<PlaybackClock>>,
    play_start_position: f64,
    end_position: f64,
    /// Timeline position of the most recently received decoded frame.
    /// Drives the playhead so it stays in sync with actual video output.
    pub last_frame_pos: Option<f64>,
    /// Receiver for single-frame scrub results.
    scrub_rx: Option<Receiver<DecodedFrame>>,
    scrub_thread: Option<JoinHandle<()>>,
    scrub_cancel: Arc<AtomicBool>,
    /// True while a scrub decode is in flight.
    scrub_busy: bool,
    /// Pending scrub request queued while a decode was in flight.
    scrub_pending: Option<ScrubRequest>,
}

impl VideoPlayer {
    /// Returns `true` when a scrub decode is in flight or pending.
    pub fn is_scrub_busy(&self) -> bool {
        self.scrub_busy || self.scrub_pending.is_some()
    }
}

impl Default for VideoPlayer {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            decode_thread: None,
            frame_rx: None,
            running: Arc::new(AtomicBool::new(false)),
            playback_clock: None,
            play_start_position: 0.0,
            end_position: 0.0,
            last_frame_pos: None,
            scrub_rx: None,
            scrub_thread: None,
            scrub_cancel: Arc::new(AtomicBool::new(false)),
            scrub_busy: false,
            scrub_pending: None,
        }
    }
}

impl VideoPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    /// Start playback from `start_pos` on the timeline.
    ///
    /// Each tuple: `(source_path, trim_start, source_duration, speed,
    ///               timeline_start, timeline_duration)`.
    pub fn play(
        &mut self,
        segments: Vec<(PathBuf, f64, f64, f64, f64, f64)>,
        start_pos: f64,
        fps: u32,
        frame_width: u32,
        frame_height: u32,
    ) {
        self.stop();

        if segments.is_empty() || frame_width == 0 || frame_height == 0 {
            return;
        }

        let segments: Vec<PlaySegment> = segments
            .into_iter()
            .map(|(source, ts, td, sp, tls, tld)| PlaySegment {
                source_path: source,
                trim_start: ts,
                trim_duration: td,
                speed: sp,
                timeline_start: tls,
                timeline_duration: tld,
            })
            .collect();

        let end_pos = segments
            .iter()
            .map(|s| s.timeline_start + s.timeline_duration)
            .fold(0.0f64, f64::max);

        self.end_position = end_pos;
        self.play_start_position = start_pos;
        self.last_frame_pos = None;
        let playback_clock = Arc::new(PlaybackClock::new());
        self.playback_clock = Some(playback_clock.clone());

        let running = Arc::new(AtomicBool::new(true));
        self.running = running.clone();

        let (tx, rx) = bounded::<DecodedFrame>(1);
        self.frame_rx = Some(rx);

        let thread = thread::Builder::new()
            .name("video-playback".into())
            .spawn(move || {
                decode_segments(
                    running,
                    tx,
                    segments,
                    start_pos,
                    fps,
                    frame_width,
                    frame_height,
                    playback_clock,
                );
            })
            .ok();

        self.decode_thread = thread;
        self.state = PlaybackState::Playing;
    }

    /// Current timeline position derived from wall-clock time.
    pub fn current_position(&self) -> f64 {
        self.playback_clock
            .as_ref()
            .map(|clock| clock.current_position(self.play_start_position, self.end_position))
            .unwrap_or(self.play_start_position)
    }

    /// Returns `true` when playback has reached the end of the timeline.
    pub fn is_finished(&self) -> bool {
        self.state == PlaybackState::Playing
            && self.last_frame_pos.unwrap_or(0.0) >= self.end_position - 0.1
    }

    /// Non-blocking: grab the next decoded frame (if available).
    pub fn try_recv_frame(&mut self) -> Option<DecodedFrame> {
        let rx = self.frame_rx.as_ref()?;
        let mut latest = None;
        while let Ok(frame) = rx.try_recv() {
            latest = Some(frame);
        }
        if let Some(ref f) = latest {
            self.last_frame_pos = Some(f.timeline_pos);
        }
        latest
    }

    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            if let Some(clock) = &self.playback_clock {
                clock.pause();
            }
            self.state = PlaybackState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == PlaybackState::Paused {
            if let Some(clock) = &self.playback_clock {
                clock.resume();
            }
            self.state = PlaybackState::Playing;
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Drop the receiver first so the decode thread's send() unblocks.
        self.frame_rx = None;
        if let Some(h) = self.decode_thread.take() {
            let _ = h.join();
        }
        self.state = PlaybackState::Stopped;
        self.playback_clock = None;
    }

    /// Request a single frame at `position` on the timeline for scrub preview.
    ///
    /// If a scrub decode is already in flight, the request is queued and will
    /// be dispatched automatically once the current decode finishes (via
    /// `try_recv_scrub_frame`). This prevents spawning an ffmpeg process per
    /// drag event while still always converging on the latest position.
    pub fn seek_frame(
        &mut self,
        segments: Vec<(PathBuf, f64, f64, f64, f64, f64)>,
        position: f64,
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 || segments.is_empty() {
            return;
        }

        // Check if the previous scrub thread has finished.
        self.check_scrub_done();

        if self.scrub_busy {
            // A decode is running — just remember the latest request.
            self.scrub_pending = Some(ScrubRequest {
                segments,
                position,
                width,
                height,
            });
            return;
        }

        self.dispatch_scrub(segments, position, width, height);
    }

    /// Actually spawn the ffmpeg scrub thread.
    fn dispatch_scrub(
        &mut self,
        segments: Vec<(PathBuf, f64, f64, f64, f64, f64)>,
        position: f64,
        width: u32,
        height: u32,
    ) {
        // Cancel any leftover (shouldn't happen, but be safe).
        self.scrub_cancel.store(true, Ordering::SeqCst);
        self.scrub_thread = None;

        let cancel = Arc::new(AtomicBool::new(false));
        self.scrub_cancel = cancel.clone();

        let (tx, rx) = bounded::<DecodedFrame>(1);
        self.scrub_rx = Some(rx);

        let thread = thread::Builder::new()
            .name("scrub-seek".into())
            .spawn(move || {
                decode_single_frame(cancel, tx, segments, position, width, height);
            })
            .ok();

        self.scrub_thread = thread;
        self.scrub_busy = true;
    }

    /// Check whether the scrub thread has finished.
    fn check_scrub_done(&mut self) {
        if !self.scrub_busy {
            return;
        }
        // The thread is done if it has been joined or is no longer alive.
        if let Some(ref h) = self.scrub_thread {
            if !h.is_finished() {
                return;
            }
        }
        // Thread done — clean up.
        if let Some(h) = self.scrub_thread.take() {
            let _ = h.join();
        }
        self.scrub_busy = false;
    }

    /// Non-blocking: grab the scrub preview frame (if ready).
    ///
    /// When a frame arrives and a newer scrub request is pending, this
    /// automatically dispatches the queued request so the preview converges.
    pub fn try_recv_scrub_frame(&mut self) -> Option<DecodedFrame> {
        let frame = self.scrub_rx.as_ref()?.try_recv().ok();

        if frame.is_some() {
            // The decode finished — mark as not busy and dispatch pending.
            self.scrub_busy = false;
            if let Some(h) = self.scrub_thread.take() {
                let _ = h.join();
            }
            if let Some(req) = self.scrub_pending.take() {
                self.dispatch_scrub(req.segments, req.position, req.width, req.height);
            }
        } else {
            // No frame yet — but check if thread died without sending.
            self.check_scrub_done();
            if !self.scrub_busy {
                if let Some(req) = self.scrub_pending.take() {
                    self.dispatch_scrub(req.segments, req.position, req.width, req.height);
                }
            }
        }

        frame
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Background decode ──────────────────────────────────────────────────────

fn decode_segments(
    running: Arc<AtomicBool>,
    tx: Sender<DecodedFrame>,
    segments: Vec<PlaySegment>,
    start_pos: f64,
    fps: u32,
    width: u32,
    height: u32,
    playback_clock: Arc<PlaybackClock>,
) {
    let frame_size = (width * height * 4) as usize;
    let frame_dur = 1.0 / fps.max(1) as f64;

    for seg in &segments {
        if !running.load(Ordering::SeqCst) {
            break;
        }

        let seg_end = seg.timeline_start + seg.timeline_duration;

        // Skip segments entirely before start_pos.
        if seg_end <= start_pos {
            continue;
        }

        // Calculate seek offset if start_pos falls within this segment.
        // For segments that begin after start_pos the offset is 0 — we always
        // start from trim_start and let wait_for_timeline_pos pace the frames
        // through any inter-segment gap. This ensures the first frame of every
        // clip matches the scrub preview at the same position.
        let offset_in_seg = (start_pos - seg.timeline_start).max(0.0);
        let source_offset = offset_in_seg * seg.speed;
        let seek_pos = seg.trim_start + source_offset;
        let remaining_source_dur = seg.trim_duration - source_offset;

        if remaining_source_dur <= 0.0 {
            continue;
        }

        // Instead of using setpts to manipulate timestamps (which
        // interacts badly with -t and -r, causing wrong frame counts
        // and hyperspeed output), we control speed purely through the
        // output frame rate.  For a clip at speed S played back at F
        // fps, we ask ffmpeg to output at F/S fps from the source.
        // This naturally samples every S-th frame from the source,
        // producing exactly the right number of frames for the
        // timeline duration.
        let decode_fps = fps as f64 / seg.speed;
        let filter = format!(
            "scale={width}:{height}:flags=bilinear"
        );

        log::debug!(
            "DECODE seg file={} seek={:.3} remaining_src_dur={:.3} speed={:.2}x decode_fps={:.2} tl={:.3}..{:.3}",
            seg.source_path.display(), seek_pos, remaining_source_dur,
            seg.speed, decode_fps, seg.timeline_start + offset_in_seg, seg_end,
        );

        let ffmpeg_path = match find_ffmpeg() {
            Ok(p) => p,
            Err(e) => {
                log::error!("Cannot find ffmpeg for decode: {e}");
                return;
            }
        };
        let mut cmd = Command::new(&ffmpeg_path);
        cmd.args(["-ss", &format!("{seek_pos:.6}")])
            .args(["-t", &format!("{remaining_source_dur:.6}")])
            .arg("-i")
            .arg(&seg.source_path)
            .args([
                "-vf",
                &filter,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-r",
                &format!("{decode_fps:.4}"),
                "-v",
                "quiet",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console_window(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to spawn ffmpeg for playback: {e}");
                break;
            }
        };

        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => break,
        };

        let mut buf = vec![0u8; frame_size];
        let mut timeline_pos = seg.timeline_start + offset_in_seg;

        loop {
            if !running.load(Ordering::SeqCst) {
                break;
            }

            match read_exact_or_eof(&mut stdout, &mut buf) {
                Ok(true) => {}
                _ => break,
            }

            let frame = DecodedFrame {
                width,
                height,
                rgba: buf.clone(),
                timeline_pos,
            };

            if !playback_clock.wait_for_timeline_pos(&running, start_pos, timeline_pos) {
                break;
            }

            // Use send (blocking) so we wait for the consumer instead of
            // dropping frames when the UI is slightly behind.
            if tx.send(frame).is_err() {
                break;
            }

            timeline_pos += frame_dur;
            if timeline_pos >= seg_end {
                break;
            }
        }

        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Decode a single frame at a specific timeline position for scrub preview.
fn decode_single_frame(
    cancel: Arc<AtomicBool>,
    tx: Sender<DecodedFrame>,
    segments: Vec<(PathBuf, f64, f64, f64, f64, f64)>,
    position: f64,
    width: u32,
    height: u32,
) {
    let frame_size = (width * height * 4) as usize;

    // Find the segment that contains this position.
    for (source_path, trim_start, source_dur, speed, tl_start, tl_dur) in &segments {
        if cancel.load(Ordering::SeqCst) {
            return;
        }

        let tl_end = tl_start + tl_dur;
        if position < *tl_start || position >= tl_end {
            continue;
        }

        let offset_in_seg = position - tl_start;
        let source_offset = offset_in_seg * speed;
        let seek_pos = trim_start + source_offset;

        if source_offset >= *source_dur {
            continue;
        }

        let ffmpeg_path = match find_ffmpeg() {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut cmd = Command::new(&ffmpeg_path);
        // Use accurate_seek: ffmpeg fast-seeks to the keyframe before seek_pos,
        // then decodes forward to the exact target frame. This is the correct
        // balance of speed vs. accuracy — much faster than decoding from the
        // start of the file, and frame-accurate unlike noaccurate_seek (which
        // just returns whatever keyframe is nearby, potentially seconds off).
        cmd.args(["-accurate_seek", "-ss", &format!("{seek_pos:.6}")])
            .arg("-i")
            .arg(source_path)
            .args([
                "-frames:v",
                "1",
                "-vf",
                &format!("scale={width}:{height}:flags=fast_bilinear"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-v",
                "quiet",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console_window(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return,
        };

        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => return,
        };

        let mut buf = vec![0u8; frame_size];
        if let Ok(true) = read_exact_or_eof(&mut stdout, &mut buf) {
            if !cancel.load(Ordering::SeqCst) {
                let _ = tx.send(DecodedFrame {
                    width,
                    height,
                    rgba: buf,
                    timeline_pos: position,
                });
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        return;
    }
}

/// Read exactly `buf.len()` bytes. Returns `Ok(true)` on success, `Ok(false)`
/// on EOF before any bytes could be read.
fn read_exact_or_eof(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Probe the resolution of a video file using ffprobe.
/// Probe the actual encoded duration of a video file using ffprobe.
/// Returns `None` if the file cannot be probed.
pub fn probe_video_duration(path: &std::path::Path) -> Option<f64> {
    let ffprobe_path = find_ffprobe().ok()?;
    let mut cmd = Command::new(&ffprobe_path);
    cmd.args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console_window(&mut cmd);
    let output = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse::<f64>().ok().filter(|&d| d > 0.0)
}

pub fn probe_video_resolution(path: &std::path::Path) -> Option<(u32, u32)> {
    let ffprobe_path = find_ffprobe().ok()?;
    let mut cmd = Command::new(&ffprobe_path);
    cmd.args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console_window(&mut cmd);
    let output = cmd.output().ok()?;

    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.trim().split(',').collect();
    if parts.len() >= 2 {
        let w = parts[0].parse().ok()?;
        let h = parts[1].parse().ok()?;
        Some((w, h))
    } else {
        None
    }
}
