pub mod app;
pub mod editor;
pub mod recorder;

use anyhow::Result;

/// Initialise logging and launch the eframe event-loop.
pub fn run() -> Result<()> {
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
