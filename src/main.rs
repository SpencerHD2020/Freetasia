//! Freetasia – a free, open-source Camtasia-style screen recorder and video
//! editor for Windows.
//!
//! On Windows builds the console window is hidden in release mode.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() {
    if let Err(e) = freetasia::run() {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}
