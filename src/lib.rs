pub mod app;
pub mod editor;
pub mod recorder;

use anyhow::Result;

/// Initialise logging and launch the eframe event-loop.
pub fn run() -> Result<()> {
    env_logger::init();

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
