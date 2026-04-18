use freetasia::recorder::RecordingState;

#[test]
fn recording_state_display() {
    assert_eq!(RecordingState::Idle.to_string(), "Idle");
    assert_eq!(RecordingState::Recording.to_string(), "Recording");
    assert_eq!(RecordingState::Paused.to_string(), "Paused");
}

#[test]
fn recording_state_default_is_idle() {
    assert_eq!(RecordingState::default(), RecordingState::Idle);
}

/// RecorderManager starts in the Idle state and elapsed time is zero.
#[test]
fn manager_initial_state() {
    use freetasia::recorder::manager::RecorderManager;
    let mgr = RecorderManager::new();
    assert_eq!(mgr.state(), RecordingState::Idle);
    assert_eq!(mgr.elapsed().as_secs(), 0);
    assert!(mgr.try_recv_frame().is_none());
}

/// Stopping without a recording returns None.
#[test]
fn manager_stop_when_idle_returns_none() {
    use freetasia::recorder::manager::RecorderManager;
    let mut mgr = RecorderManager::new();
    assert!(mgr.stop_recording().is_none());
}

/// Pausing / resuming when idle is a no-op (does not panic).
#[test]
fn manager_pause_resume_when_idle() {
    use freetasia::recorder::manager::RecorderManager;
    let mut mgr = RecorderManager::new();
    mgr.pause_recording();
    assert_eq!(mgr.state(), RecordingState::Idle);
    mgr.resume_recording();
    assert_eq!(mgr.state(), RecordingState::Idle);
}

/// Screen capture requires a live display and ffmpeg; mark as ignored for CI.
#[test]
#[ignore = "requires a display and ffmpeg"]
fn screen_recorder_starts_and_stops() {
    use freetasia::recorder::screen::ScreenRecorder;
    use std::time::Duration;

    let tmp = std::env::temp_dir().join("freetasia_test_screen.mp4");
    let mut rec = ScreenRecorder::start(0, 10, tmp.clone()).expect("ScreenRecorder::start");
    std::thread::sleep(Duration::from_millis(500));
    rec.stop();
}

/// Audio capture requires a microphone; mark as ignored for CI.
#[test]
#[ignore = "requires an audio input device"]
fn audio_recorder_starts_and_stops() {
    use freetasia::recorder::audio::AudioRecorder;
    use std::time::Duration;

    let tmp = std::env::temp_dir().join("freetasia_test_audio.wav");
    let mut rec = AudioRecorder::start(tmp.clone()).expect("AudioRecorder::start");
    std::thread::sleep(Duration::from_millis(500));
    rec.stop();
    assert!(tmp.exists(), "WAV file should have been created");
}
