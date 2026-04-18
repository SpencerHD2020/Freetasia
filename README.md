# 🎬 Freetasia

A **free, open-source screen recorder and video editor for Windows**, inspired by Camtasia.  
Built entirely in **Rust** using [egui](https://github.com/emilk/egui) for the UI.

---

## Features

| Feature | Status |
|---|---|
| Screen capture (any monitor) | ✅ |
| Microphone audio recording | ✅ |
| Live preview during recording | ✅ |
| Pause / Resume recording | ✅ |
| Multi-clip timeline with scrubbing | ✅ |
| Per-clip trim controls | ✅ |
| Project save / load (JSON) | ✅ |
| Export to MP4 via ffmpeg | ✅ |
| Configurable FPS & output directory | ✅ |

---

## Prerequisites

| Dependency | Purpose |
|---|---|
| [Rust ≥ 1.75](https://rustup.rs/) | Build toolchain |
| [ffmpeg](https://ffmpeg.org/download.html) on `PATH` | Video encoding & export |

> On Windows, download an ffmpeg release build, extract it, and add the `bin/` folder to your `PATH`.

---

## Building & Running

```powershell
# Clone
git clone https://github.com/SpencerHD2020/Freetasia.git
cd Freetasia

# Debug build (shows console for logging)
cargo run

# Optimised release build
cargo build --release
.\target\release\freetasia.exe
```

---

## Usage

1. **Select a monitor** and **FPS** in the *Recording Controls* panel (right side).
2. Toggle **Record audio** if you want microphone input.
3. Press **⏺ Record** to start. The live preview updates in real time.
4. Press **⏸ Pause** to pause without stopping.
5. Press **⏹ Stop** — the clip is automatically added to the **Timeline**.
6. **Trim** the clip using the drag-value controls below the timeline, or drag the playhead to preview different positions.
7. Click **🚀 Export** to render the final video via ffmpeg.
8. Save / reopen your work with **💾 Save** / **📂 Open** (JSON project files).

---

## Architecture

```
src/
├── lib.rs              Entry point; wires up eframe
├── main.rs             Thin binary wrapper
├── app.rs              Main egui App — UI layout & user interactions
├── recorder/
│   ├── mod.rs          RecordingState enum
│   ├── screen.rs       Screen capture → ffmpeg pipe (per-frame RGBA)
│   ├── audio.rs        Microphone capture → WAV via cpal + hound
│   └── manager.rs      Coordinates screen + audio; tracks elapsed time
└── editor/
    ├── mod.rs
    ├── clip.rs         Clip data model (trim, timeline placement)
    ├── timeline.rs     Ordered clip list + playhead
    ├── project.rs      JSON-serialisable project file
    └── export.rs       Builds & runs the ffmpeg filter-graph command
```

---

## Running Tests

```powershell
cargo test
```

Hardware-dependent tests (screen capture, audio) are marked `#[ignore]` and can be run explicitly on a machine with a display and microphone:

```powershell
cargo test -- --ignored
```

---

## License

MIT
