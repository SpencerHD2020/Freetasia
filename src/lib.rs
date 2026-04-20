pub mod app;
pub mod editor;
pub mod recorder;

use anyhow::Result;

// On Windows the default multimedia timer resolution is ~15 ms, which means
// thread::sleep(10ms) actually sleeps ~15 ms.  Requesting 1 ms resolution
// brings the OS scheduler in line with what the decode thread expects and
// gives much more accurate per-frame pacing.
#[cfg(target_os = "windows")]
mod win_timer {
    #[link(name = "winmm")]
    extern "system" {
        pub fn timeBeginPeriod(uPeriod: u32) -> u32;
    }
}

/// Initialise logging and launch the eframe event-loop.
pub fn run() -> Result<()> {
    // Enable 1 ms timer resolution on Windows for accurate frame pacing.
    #[cfg(target_os = "windows")]
    unsafe { win_timer::timeBeginPeriod(1); }
    // Write logs to a file so we can inspect timeline state.
    use std::io::Write;
    let log_path = std::env::temp_dir().join("freetasia-debug.log");
    let target = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("Cannot open log file");
    let target = std::sync::Mutex::new(target);
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .format(move |_buf, record| {
            let now = chrono::Local::now().format("%H:%M:%S%.3f");
            let line = format!("[{now} {} {}] {}\n", record.level(), record.target(), record.args());
            let _ = target.lock().unwrap().write_all(line.as_bytes());
            Ok(())
        })
        .init();
    log::info!("Log file: {}", log_path.display());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Freetasia – Screen Recorder & Video Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "Freetasia",
        options,
        Box::new(|cc| {
            Box::new(app::FreetasiaApp::new(cc)) as Box<dyn eframe::App>
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}
