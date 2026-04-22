use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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
    /// Active audio output for the current playback session.
    audio_playback: Option<AudioPlayback>,
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
            audio_playback: None,
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
    /// `audio_paths` is parallel to `segments` — the optional WAV path for each clip.
    pub fn play(
        &mut self,
        segments: Vec<(PathBuf, f64, f64, f64, f64, f64)>,
        audio_paths: Vec<Option<PathBuf>>,
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

        // Keep a copy of segment metadata for audio (before segments is moved into the thread).
        let segments_for_audio: Vec<PlaySegment> = segments.iter().map(|s| PlaySegment {
            source_path: s.source_path.clone(),
            trim_start: s.trim_start,
            trim_duration: s.trim_duration,
            speed: s.speed,
            timeline_start: s.timeline_start,
            timeline_duration: s.timeline_duration,
        }).collect();

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

        // Buffer 4 frames so the decode thread can stay ahead of the UI repaint
        // cycle without blocking on every single frame.
        let (tx, rx) = bounded::<DecodedFrame>(4);
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

        // Start audio output for any segments that have an associated WAV file.
        let audio_segs: Vec<(PathBuf, f64, f64, f64, f64, f64)> = audio_paths
            .into_iter()
            .zip(segments_for_audio.iter())
            .filter_map(|(ap, seg)| {
                ap.filter(|p| p.exists()).map(|p| {
                    (p, seg.trim_start, seg.trim_duration, seg.speed, seg.timeline_start, seg.timeline_duration)
                })
            })
            .collect();
        self.audio_playback = if audio_segs.is_empty() {
            None
        } else {
            AudioPlayback::start(&audio_segs, start_pos)
        };

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
            if let Some(ref ap) = self.audio_playback {
                ap.pause();
            }
            self.state = PlaybackState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == PlaybackState::Paused {
            if let Some(clock) = &self.playback_clock {
                clock.resume();
            }
            if let Some(ref ap) = self.audio_playback {
                ap.resume();
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
        if let Some(mut ap) = self.audio_playback.take() {
            ap.stop();
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

            // Move the buffer into the frame and allocate a fresh one for the
            // next iteration – this avoids an 8 MB clone on every frame.
            let new_buf = vec![0u8; frame_size];
            let rgba = std::mem::replace(&mut buf, new_buf);
            let frame = DecodedFrame {
                width,
                height,
                rgba,
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

// ── Audio playback ──────────────────────────────────────────────────────────

/// Plays back the audio track(s) matching the video segments in real time.
pub struct AudioPlayback {
    /// Keeps the cpal stream alive.
    _stream: cpal::Stream,
    paused: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

impl AudioPlayback {
    /// Build the interleaved sample buffer from all audio segments, then start
    /// a cpal output stream from `play_start_pos`.
    ///
    /// `audio_segs`: `(audio_path, trim_start, source_dur, speed, tl_start, tl_dur)`
    pub fn start(
        audio_segs: &[(PathBuf, f64, f64, f64, f64, f64)],
        play_start_pos: f64,
    ) -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let out_sr = config.sample_rate().0 as f64;
        let out_ch = config.channels() as usize;

        // Total timeline end across all audio segments.
        let tl_end = audio_segs
            .iter()
            .map(|(_, _, _, _, tls, tld)| tls + tld)
            .fold(play_start_pos, f64::max);

        if tl_end <= play_start_pos {
            return None;
        }

        let total_frames = ((tl_end - play_start_pos) * out_sr).ceil() as usize + out_sr as usize;
        let total_samples = total_frames * out_ch;
        let mut samples = vec![0.0f32; total_samples];

        for (path, trim_start, _source_dur, speed, tl_start, tl_dur) in audio_segs {
            let seg_tl_start = tl_start.max(play_start_pos);
            let seg_tl_end = (tl_start + tl_dur).min(tl_end);
            if seg_tl_start >= seg_tl_end {
                continue;
            }

            let Ok(mut reader) = hound::WavReader::open(path) else {
                continue;
            };
            let spec = reader.spec();
            let wav_sr = spec.sample_rate as f64;
            let wav_ch = spec.channels as usize;

            let wav_samples: Vec<f32> = match spec.sample_format {
                hound::SampleFormat::Float => {
                    reader.samples::<f32>().filter_map(|s| s.ok()).collect()
                }
                hound::SampleFormat::Int => {
                    let scale = (1i64 << (spec.bits_per_sample as u32 - 1)) as f32;
                    reader.samples::<i32>()
                        .filter_map(|s| s.ok())
                        .map(|s| s as f32 / scale)
                        .collect()
                }
            };

            let out_frame_start = ((seg_tl_start - play_start_pos) * out_sr) as usize;
            let out_frames = ((seg_tl_end - seg_tl_start) * out_sr).ceil() as usize;

            // Offset into the segment caused by seeking past its start.
            let seg_seek_offset = (play_start_pos - tl_start).max(0.0);
            let src_start_secs = trim_start + seg_seek_offset * speed;

            for out_frame in 0..out_frames {
                let src_secs = src_start_secs + (out_frame as f64 / out_sr) * speed;
                let src_frame = (src_secs * wav_sr) as usize;
                let wav_frames = wav_samples.len() / wav_ch.max(1);
                if src_frame >= wav_frames {
                    break;
                }

                for ch in 0..out_ch {
                    let out_idx = (out_frame_start + out_frame) * out_ch + ch;
                    if out_idx >= total_samples {
                        break;
                    }
                    // Down-mix if WAV has fewer channels than output.
                    let src_ch = ch.min(wav_ch.saturating_sub(1));
                    let src_idx = src_frame * wav_ch + src_ch;
                    if src_idx < wav_samples.len() {
                        samples[out_idx] = wav_samples[src_idx];
                    }
                }
            }
        }

        let samples = Arc::new(samples);
        let cursor = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));

        let err_fn = |e: cpal::StreamError| log::error!("Audio output error: {e}");

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let (s, c, r, p) = (samples.clone(), cursor.clone(), running.clone(), paused.clone());
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [f32], _| audio_fill_f32(data, &s, &c, &r, &p),
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let (s, c, r, p) = (samples.clone(), cursor.clone(), running.clone(), paused.clone());
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [i16], _| audio_fill_i16(data, &s, &c, &r, &p),
                    err_fn,
                    None,
                )
            }
            _ => return None,
        }
        .ok()?;

        stream.play().ok()?;

        Some(Self {
            _stream: stream,
            paused,
            running,
        })
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

fn audio_fill_f32(
    data: &mut [f32],
    samples: &[f32],
    cursor: &AtomicUsize,
    running: &AtomicBool,
    paused: &AtomicBool,
) {
    if !running.load(Ordering::Relaxed) || paused.load(Ordering::Relaxed) {
        data.fill(0.0);
        return;
    }
    let pos = cursor.fetch_add(data.len(), Ordering::Relaxed);
    for (i, s) in data.iter_mut().enumerate() {
        *s = samples.get(pos + i).copied().unwrap_or(0.0);
    }
}

fn audio_fill_i16(
    data: &mut [i16],
    samples: &[f32],
    cursor: &AtomicUsize,
    running: &AtomicBool,
    paused: &AtomicBool,
) {
    if !running.load(Ordering::Relaxed) || paused.load(Ordering::Relaxed) {
        data.fill(0);
        return;
    }
    let pos = cursor.fetch_add(data.len(), Ordering::Relaxed);
    for (i, s) in data.iter_mut().enumerate() {
        let f = samples.get(pos + i).copied().unwrap_or(0.0);
        *s = (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
    }
}
