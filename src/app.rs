use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::path::PathBuf;

use crate::editor::{
    clip::Clip,
    export::{self, ExportProgress},
    player::{PlaybackState, VideoPlayer},
    project::Project,
    text_overlay::TextOverlay,
};
use crate::recorder::{
    manager::RecorderManager,
    RecordingState,
};

// ── Colour palette ──────────────────────────────────────────────────────────

const COLOR_RECORD: Color32 = Color32::from_rgb(220, 50, 50);
const COLOR_PAUSE: Color32 = Color32::from_rgb(240, 160, 30);
const COLOR_STOP: Color32 = Color32::from_rgb(80, 80, 80);
const COLOR_TIMELINE_BG: Color32 = Color32::from_rgb(30, 30, 30);
const COLOR_CLIP: Color32 = Color32::from_rgb(60, 120, 200);
const COLOR_CLIP_SELECTED: Color32 = Color32::from_rgb(90, 160, 240);
const COLOR_PLAYHEAD: Color32 = Color32::from_rgb(250, 60, 60);
const COLOR_RULER_TEXT: Color32 = Color32::from_rgb(180, 180, 180);
const COLOR_TRIM_HANDLE: Color32 = Color32::from_rgb(0, 200, 130);
const COLOR_TRIM_REGION: Color32 = Color32::from_rgba_premultiplied(0, 200, 130, 40);
const COLOR_TEXT_OVERLAY: Color32 = Color32::from_rgb(255, 200, 50);
const COLOR_TEXT_OVERLAY_SELECTED: Color32 = Color32::from_rgb(255, 230, 100);

// ── App state ───────────────────────────────────────────────────────────────

/// Root application state and egui `App` implementation.
pub struct FreetasiaApp {
    // ── Recorder ──
    recorder: RecorderManager,
    /// Available monitor names (populated once at startup).
    monitor_names: Vec<String>,

    // ── Editor ──
    project: Project,
    selected_clip_id: Option<u64>,

    // ── Preview ──
    preview_texture: Option<egui::TextureHandle>,

    // ── Playback ──
    player: VideoPlayer,

    // ── Timeline UI ──
    /// Pixels per second (zoom).
    zoom: f32,
    /// Clip being dragged on the timeline (id + offset from clip start to grab point).
    dragging_clip_id: Option<u64>,
    drag_offset: f64,
    /// Whether the playhead handle is being dragged.
    dragging_playhead: bool,

    // ── Trim heads ──
    /// Left trim head position on the timeline (None = not placed).
    trim_head_left: Option<f64>,
    /// Right trim head position on the timeline (None = not placed).
    trim_head_right: Option<f64>,
    /// Which trim head is currently being dragged.
    dragging_trim_left: bool,
    dragging_trim_right: bool,

    // ── Scrub resolution cache ──
    /// Cached native video resolution so we don't ffprobe on every scrub.
    cached_resolution: Option<(u32, u32)>,

    // ── Text overlays ──
    /// Currently selected text overlay id.
    selected_overlay_id: Option<u64>,
    /// Dragging a text overlay body on the timeline.
    dragging_overlay_id: Option<u64>,
    /// Dragging the left edge of a text overlay to resize.
    dragging_overlay_left_edge: Option<u64>,
    /// Dragging the right edge of a text overlay to resize.
    dragging_overlay_right_edge: Option<u64>,
    /// Dragging a text overlay in the preview to reposition.
    dragging_overlay_preview: bool,
    /// Drag offset when moving an overlay body.
    overlay_drag_offset: f64,

    // ── Dialogs / overlays ──
    show_export_dialog: bool,
    export_path: String,
    show_about: bool,
    status_msg: String,
    ffmpeg_ok: bool,

    // ── Export progress ──
    export_progress: Option<f32>,
    export_progress_rx: Option<crossbeam_channel::Receiver<ExportProgress>>,
    exporting: bool,
}

impl FreetasiaApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let monitor_names = detect_monitor_names();
        let ffmpeg_ok = export::ffmpeg_available();

        Self {
            recorder: RecorderManager::new(),
            monitor_names,
            project: Project::default(),
            selected_clip_id: None,
            preview_texture: None,
            player: VideoPlayer::new(),
            zoom: 80.0,
            dragging_clip_id: None,
            drag_offset: 0.0,
            dragging_playhead: false,
            trim_head_left: None,
            trim_head_right: None,
            dragging_trim_left: false,
            dragging_trim_right: false,
            cached_resolution: None,
            selected_overlay_id: None,
            dragging_overlay_id: None,
            dragging_overlay_left_edge: None,
            dragging_overlay_right_edge: None,
            dragging_overlay_preview: false,
            overlay_drag_offset: 0.0,
            show_export_dialog: false,
            export_path: String::new(),
            show_about: false,
            status_msg: if ffmpeg_ok {
                "Ready".into()
            } else {
                "⚠ ffmpeg not found — recording/export disabled".into()
            },
            ffmpeg_ok,
            export_progress: None,
            export_progress_rx: None,
            exporting: false,
        }
    }
}

impl eframe::App for FreetasiaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Pull latest preview frame from the recorder (non-blocking).
        self.refresh_preview(ctx);

        // Advance playback and pull decoded frames.
        self.refresh_playback(ctx);

        // Pull scrub preview frame when not playing.
        self.refresh_scrub_preview(ctx);

        // Poll export progress (non-blocking).
        self.poll_export_progress();

        // Keep repainting while recording, playing, exporting, or waiting for a scrub frame.
        if self.recorder.state() == RecordingState::Recording
            || self.player.state() == PlaybackState::Playing
            || self.player.is_scrub_busy()
            || self.exporting
        {
            ctx.request_repaint();
        }

        self.show_toolbar(ctx);
        self.show_status_bar(ctx);
        self.show_timeline_panel(ctx);
        self.show_central_panel(ctx);

        // Modal dialogs rendered last so they appear on top.
        if self.show_export_dialog {
            self.show_export_dialog(ctx);
        }
        if self.show_about {
            self.show_about_dialog(ctx);
        }
    }
}

// ── Panel renderers ─────────────────────────────────────────────────────────

impl FreetasiaApp {
    fn show_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🎬 Freetasia");
                ui.separator();

                if ui.button("📄 New").on_hover_text("New project").clicked() {
                    self.project = Project::default();
                    self.selected_clip_id = None;
                    self.invalidate_resolution_cache();
                    self.status("New project created");
                }

                if ui.button("📂 Open").on_hover_text("Open project file").clicked() {
                    self.open_project();
                }

                if ui.button("💾 Save").on_hover_text("Save project").clicked() {
                    self.save_project();
                }

                ui.separator();

                let can_export = !self.project.timeline.is_empty() && self.ffmpeg_ok;
                if ui
                    .add_enabled(can_export, egui::Button::new("🚀 Export"))
                    .on_hover_text("Export to video file")
                    .clicked()
                {
                    self.export_path = self
                        .project
                        .default_output_name()
                        .to_string_lossy()
                        .into_owned();
                    self.show_export_dialog = true;
                }

                ui.separator();

                // Project name editor.
                ui.label("Project:");
                ui.text_edit_singleline(&mut self.project.name);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("ℹ About").clicked() {
                        self.show_about = true;
                    }
                });
            });
        });
    }

    fn show_status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_msg);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let dur = self.project.timeline.total_duration();
                    ui.label(format!("Timeline: {}", fmt_duration(dur)));
                });
            });
        });
    }

    fn show_timeline_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("timeline_panel")
            .min_height(160.0)
            .max_height(260.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Timeline");
                ui.separator();
                self.draw_timeline(ui);
            });
    }

    fn show_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                // Left column: preview.
                cols[0].vertical(|ui| {
                    ui.heading("Preview");
                    ui.separator();
                    self.draw_preview(ui);
                });
                // Right column: recording controls.
                cols[1].vertical(|ui| {
                    ui.heading("Recording Controls");
                    ui.separator();
                    self.draw_recording_controls(ui);
                });
            });
        });
    }

    // ── Preview ──────────────────────────────────────────────────────────────

    fn draw_preview(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let preview_size = Vec2::new(available.x, (available.x * 9.0 / 16.0).min(available.y - 42.0));

        // Allocate the preview area with drag support (for text overlay positioning).
        let (preview_rect, preview_resp) =
            ui.allocate_exact_size(preview_size, Sense::click_and_drag());
        let painter = ui.painter_at(preview_rect);

        if let Some(ref tex) = self.preview_texture {
            painter.image(
                tex.id(),
                preview_rect,
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.rect_filled(preview_rect, 4.0, Color32::from_rgb(10, 10, 10));
            painter.text(
                preview_rect.center(),
                egui::Align2::CENTER_CENTER,
                "No preview",
                egui::FontId::proportional(18.0),
                Color32::from_gray(120),
            );
        }

        // ── Draw text overlays on preview ──
        let ph = self.project.timeline.playhead;
        let visible_overlays: Vec<(u64, String, f32, f32, f32, [u8; 4])> = self
            .project
            .timeline
            .text_overlays_at(ph)
            .iter()
            .map(|o| (o.id, o.text.clone(), o.x, o.y, o.font_size, o.color))
            .collect();

        for (oid, text, ox, oy, font_size, color) in &visible_overlays {
            let px = preview_rect.min.x + ox * preview_rect.width();
            let py = preview_rect.min.y + oy * preview_rect.height();
            // Scale font size relative to preview height (designed for 1080p).
            let scaled_size = font_size * preview_rect.height() / 1080.0;
            let text_color = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
            let selected = self.selected_overlay_id == Some(*oid);

            // Draw selection indicator.
            if selected {
                let galley = painter.layout_no_wrap(
                    text.clone(),
                    egui::FontId::proportional(scaled_size),
                    text_color,
                );
                let text_rect = Rect::from_min_size(
                    Pos2::new(px - galley.size().x * 0.5, py - galley.size().y * 0.5),
                    galley.size(),
                );
                painter.rect_stroke(
                    text_rect.expand(3.0),
                    2.0,
                    Stroke::new(1.5, Color32::from_rgb(255, 200, 50)),
                );
            }

            // Draw text with a shadow for readability.
            painter.text(
                Pos2::new(px + 1.5, py + 1.5),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(scaled_size),
                Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
            painter.text(
                Pos2::new(px, py),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(scaled_size),
                text_color,
            );
        }

        // ── Drag text overlay in preview to reposition ──
        if let Some(sel_id) = self.selected_overlay_id {
            if preview_resp.drag_started() {
                // Only start drag if the selected overlay is visible.
                if visible_overlays.iter().any(|(oid, ..)| *oid == sel_id) {
                    self.dragging_overlay_preview = true;
                }
            }
            if preview_resp.dragged() && self.dragging_overlay_preview {
                if let Some(pos) = preview_resp.interact_pointer_pos() {
                    let nx = ((pos.x - preview_rect.min.x) / preview_rect.width()).clamp(0.0, 1.0);
                    let ny = ((pos.y - preview_rect.min.y) / preview_rect.height()).clamp(0.0, 1.0);
                    if let Some(overlay) = self.project.timeline.text_overlay_mut(sel_id) {
                        overlay.x = nx;
                        overlay.y = ny;
                    }
                }
            }
            if preview_resp.drag_stopped() {
                self.dragging_overlay_preview = false;
            }
            // Click on preview to select overlay under cursor.
            if preview_resp.clicked() {
                if let Some(pos) = preview_resp.interact_pointer_pos() {
                    let nx = (pos.x - preview_rect.min.x) / preview_rect.width();
                    let ny = (pos.y - preview_rect.min.y) / preview_rect.height();
                    // Find the closest visible overlay to the click.
                    let mut best: Option<u64> = None;
                    let mut best_dist = 0.05_f32; // threshold in normalized coords
                    for (oid, _, ox, oy, ..) in &visible_overlays {
                        let d = ((nx - ox).powi(2) + (ny - oy).powi(2)).sqrt();
                        if d < best_dist {
                            best_dist = d;
                            best = Some(*oid);
                        }
                    }
                    if let Some(id) = best {
                        self.selected_overlay_id = Some(id);
                    }
                }
            }
        }

        // ── Playback transport ──
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let is_playing = self.player.state() == PlaybackState::Playing;
            let is_paused = self.player.state() == PlaybackState::Paused;
            let is_recording = self.recorder.state() != RecordingState::Idle;
            let can_play =
                !self.project.timeline.is_empty() && !is_recording && self.ffmpeg_ok;

            if is_playing {
                if ui.button("⏸  Pause").clicked() {
                    self.player.pause();
                }
                if ui.button("⏹  Stop").clicked() {
                    self.player.stop();
                    self.status("Playback stopped");
                }
            } else if is_paused {
                if ui.button("▶  Resume").clicked() {
                    self.player.resume();
                }
                if ui.button("⏹  Stop").clicked() {
                    self.player.stop();
                    self.status("Playback stopped");
                }
            } else if ui
                .add_enabled(can_play, egui::Button::new("▶  Play"))
                .on_hover_text("Play timeline from playhead")
                .clicked()
            {
                self.start_playback();
            }

            // Playback position display.
            if is_playing || is_paused {
                let pos = self.player.current_position();
                ui.label(
                    RichText::new(fmt_duration_hms(pos))
                        .monospace()
                        .size(14.0),
                );
            }
        });
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        if let Some(frame) = self.recorder.try_recv_frame() {
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            match self.preview_texture.as_mut() {
                Some(tex) => tex.set(color_image, egui::TextureOptions::default()),
                None => {
                    self.preview_texture = Some(ctx.load_texture(
                        "preview",
                        color_image,
                        egui::TextureOptions::default(),
                    ));
                }
            }
        }
    }

    // ── Playback ─────────────────────────────────────────────────────────────

    fn refresh_playback(&mut self, ctx: &egui::Context) {
        if self.player.state() == PlaybackState::Stopped {
            return;
        }

        let pos = self.player.current_position();
        self.project.timeline.set_playhead(pos);

        if let Some(frame) = self.player.try_recv_frame() {
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            match self.preview_texture.as_mut() {
                Some(tex) => tex.set(color_image, egui::TextureOptions::default()),
                None => {
                    self.preview_texture = Some(ctx.load_texture(
                        "preview",
                        color_image,
                        egui::TextureOptions::default(),
                    ));
                }
            }
        }

        if self.player.is_finished() {
            self.player.stop();
            self.status("Playback finished");
        }
    }

    fn start_playback(&mut self) {
        let clips = self.project.timeline.clips();
        if clips.is_empty() {
            return;
        }

        let (width, height) = clips
            .iter()
            .find_map(|c| crate::editor::player::probe_video_resolution(&c.source_path))
            .unwrap_or(self.project.output_resolution);

        let segments: Vec<_> = clips
            .iter()
            .map(|c| {
                (
                    c.source_path.clone(),
                    c.trim_start,
                    c.source_duration(),
                    c.speed,
                    c.timeline_start,
                    c.duration(),
                )
            })
            .collect();

        let start_pos = self.project.timeline.playhead;
        self.player
            .play(segments, start_pos, self.project.output_fps, width, height);
        self.status("Playing");
    }

    fn refresh_scrub_preview(&mut self, ctx: &egui::Context) {
        if let Some(frame) = self.player.try_recv_scrub_frame() {
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            match self.preview_texture.as_mut() {
                Some(tex) => tex.set(color_image, egui::TextureOptions::default()),
                None => {
                    self.preview_texture = Some(ctx.load_texture(
                        "preview",
                        color_image,
                        egui::TextureOptions::default(),
                    ));
                }
            }
        }
    }

    fn request_scrub_frame(&mut self) {
        let clips = self.project.timeline.clips();
        if clips.is_empty() || !self.ffmpeg_ok {
            return;
        }

        // Use cached resolution to avoid spawning ffprobe on every scrub event.
        let (native_w, native_h) = *self.cached_resolution.get_or_insert_with(|| {
            clips
                .iter()
                .find_map(|c| crate::editor::player::probe_video_resolution(&c.source_path))
                .unwrap_or(self.project.output_resolution)
        });
        // Halve resolution for scrub preview – 4× fewer pixels to decode.
        let width = (native_w / 2).max(2) & !1;  // keep even
        let height = (native_h / 2).max(2) & !1;

        let segments: Vec<_> = clips
            .iter()
            .map(|c| {
                (
                    c.source_path.clone(),
                    c.trim_start,
                    c.source_duration(),
                    c.speed,
                    c.timeline_start,
                    c.duration(),
                )
            })
            .collect();

        let position = self.project.timeline.playhead;
        self.player.seek_frame(segments, position, width, height);
    }

    /// Request a scrub preview at an arbitrary timeline position (for trim heads).
    fn request_scrub_frame_at(&mut self, position: f64) {
        let clips = self.project.timeline.clips();
        if clips.is_empty() || !self.ffmpeg_ok {
            return;
        }

        let (native_w, native_h) = *self.cached_resolution.get_or_insert_with(|| {
            clips
                .iter()
                .find_map(|c| crate::editor::player::probe_video_resolution(&c.source_path))
                .unwrap_or(self.project.output_resolution)
        });
        let width = (native_w / 2).max(2) & !1;
        let height = (native_h / 2).max(2) & !1;

        let segments: Vec<_> = clips
            .iter()
            .map(|c| {
                (
                    c.source_path.clone(),
                    c.trim_start,
                    c.source_duration(),
                    c.speed,
                    c.timeline_start,
                    c.duration(),
                )
            })
            .collect();

        self.player.seek_frame(segments, position, width, height);
    }

    /// Returns the effective trim range `(start, end)` or `None` if no trim heads are placed.
    fn trim_range(&self) -> Option<(f64, f64)> {
        let ph = self.project.timeline.playhead;
        match (self.trim_head_left, self.trim_head_right) {
            (Some(l), Some(r)) => Some((l.min(r), l.max(r))),
            (Some(l), None) => Some((l.min(ph), l.max(ph))),
            (None, Some(r)) => Some((r.min(ph), r.max(ph))),
            (None, None) => None,
        }
    }

    /// Perform the cut operation: remove the section between the trim heads.
    fn perform_cut(&mut self) {
        if let Some((start, end)) = self.trim_range() {
            if self.project.timeline.cut_range(start, end) {
                // Clear trim heads after a successful cut.
                self.trim_head_left = None;
                self.trim_head_right = None;
                self.selected_clip_id = None;
                // Move playhead to the cut point.
                self.project.timeline.set_playhead(start);
                self.request_scrub_frame();
                self.status(format!(
                    "Cut {:.1}s of timeline ({:.2}s – {:.2}s)",
                    end - start,
                    start,
                    end
                ));
            } else {
                self.status("Nothing to cut in selected range");
            }
        }
    }

    /// Invalidate cached video resolution (call when clips change).
    fn invalidate_resolution_cache(&mut self) {
        self.cached_resolution = None;
    }

    // ── Recording controls ──────────────────────────────────────────────────

    fn draw_recording_controls(&mut self, ui: &mut egui::Ui) {
        let state = self.recorder.state();
        let is_idle = state == RecordingState::Idle;
        let is_recording = state == RecordingState::Recording;
        let is_paused = state == RecordingState::Paused;

        // ── Large record / pause / stop buttons ───────────────────────────
        ui.horizontal(|ui| {
            // Record button.
            let rec_label = RichText::new("⏺  Record").size(16.0).color(Color32::WHITE);
            let rec_btn = egui::Button::new(rec_label)
                .fill(if is_recording { Color32::from_rgb(180, 30, 30) } else { COLOR_RECORD })
                .min_size(Vec2::new(110.0, 36.0));
            let not_playing = self.player.state() == PlaybackState::Stopped;
            if ui
                .add_enabled(is_idle && self.ffmpeg_ok && not_playing, rec_btn)
                .on_hover_text("Start recording")
                .clicked()
            {
                self.start_recording();
            }

            // Pause / Resume button.
            let (pause_label, pause_enabled) = if is_paused {
                ("▶  Resume", true)
            } else {
                ("⏸  Pause", is_recording)
            };
            let pause_btn = egui::Button::new(RichText::new(pause_label).size(16.0))
                .fill(COLOR_PAUSE)
                .min_size(Vec2::new(100.0, 36.0));
            if ui
                .add_enabled(pause_enabled, pause_btn)
                .clicked()
            {
                if is_paused {
                    self.recorder.resume_recording();
                    self.status("Recording resumed");
                } else {
                    self.recorder.pause_recording();
                    self.status("Recording paused");
                }
            }

            // Stop button.
            let stop_btn = egui::Button::new(
                RichText::new("⏹  Stop").size(16.0).color(Color32::WHITE),
            )
            .fill(COLOR_STOP)
            .min_size(Vec2::new(90.0, 36.0));
            if ui
                .add_enabled(is_recording || is_paused, stop_btn)
                .on_hover_text("Stop recording and add clip to timeline")
                .clicked()
            {
                self.stop_recording();
            }
        });

        ui.add_space(8.0);

        // ── Timer ─────────────────────────────────────────────────────────
        let elapsed = self.recorder.elapsed();
        let timer_text = RichText::new(fmt_duration_hms(elapsed.as_secs_f64()))
            .size(28.0)
            .monospace()
            .color(if is_recording { COLOR_RECORD } else { Color32::GRAY });
        ui.label(timer_text);

        // State badge.
        let badge_color = match state {
            RecordingState::Recording => COLOR_RECORD,
            RecordingState::Paused => COLOR_PAUSE,
            RecordingState::Idle => Color32::DARK_GRAY,
        };
        ui.label(RichText::new(format!("● {state}")).color(badge_color));

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);

        // ── Settings (only when idle) ─────────────────────────────────────
        ui.add_enabled_ui(is_idle, |ui| {
            egui::Grid::new("rec_settings")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    // Monitor selector.
                    ui.label("Monitor:");
                    egui::ComboBox::from_id_source("monitor_combo")
                        .selected_text(
                            self.monitor_names
                                .get(self.recorder.monitor_index)
                                .cloned()
                                .unwrap_or_else(|| "Monitor 0".into()),
                        )
                        .show_ui(ui, |ui| {
                            for (i, name) in self.monitor_names.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.recorder.monitor_index,
                                    i,
                                    name,
                                );
                            }
                        });
                    ui.end_row();

                    // FPS selector.
                    ui.label("FPS:");
                    egui::ComboBox::from_id_source("fps_combo")
                        .selected_text(self.recorder.fps.to_string())
                        .show_ui(ui, |ui| {
                            for &fps in &[10u32, 15, 20, 24, 30, 60] {
                                ui.selectable_value(
                                    &mut self.recorder.fps,
                                    fps,
                                    fps.to_string(),
                                );
                            }
                        });
                    ui.end_row();

                    // Record audio toggle.
                    ui.label("Record audio:");
                    ui.checkbox(&mut self.recorder.record_audio, "");
                    ui.end_row();

                    // Output directory.
                    ui.label("Output dir:");
                    let mut out = self.recorder.output_dir.to_string_lossy().into_owned();
                    if ui.text_edit_singleline(&mut out).changed() {
                        self.recorder.output_dir = PathBuf::from(&out);
                    }
                    ui.end_row();
                });
        });

        if !self.ffmpeg_ok {
            ui.add_space(8.0);
            ui.colored_label(
                Color32::YELLOW,
                "⚠ ffmpeg not found. Install ffmpeg and add it to PATH.",
            );
        }
    }

    // ── Timeline ─────────────────────────────────────────────────────────────

    fn draw_timeline(&mut self, ui: &mut egui::Ui) {
        let total_dur = self.project.timeline.total_duration().max(30.0);
        let track_h = 40.0;
        let text_track_h = 30.0;
        let track_gap = 2.0;
        let ruler_h = 20.0;
        let total_h = ruler_h + track_h + track_gap + text_track_h + 8.0;

        let available_w = ui.available_width();

        egui::ScrollArea::horizontal()
            .id_source("timeline_scroll")
            .show(ui, |ui| {
                let content_w = (total_dur as f32 * self.zoom).max(available_w);
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(content_w, total_h),
                    Sense::click_and_drag(),
                );

                let painter = ui.painter_at(rect);
                let origin = rect.min;

                // Background.
                painter.rect_filled(rect, 0.0, COLOR_TIMELINE_BG);

                // ── Ruler ────────────────────────────────────────────────
                let step_secs = ruler_step_secs(self.zoom);
                let mut t = 0.0f64;
                while t <= total_dur + step_secs {
                    let x = origin.x + t as f32 * self.zoom;
                    painter.line_segment(
                        [Pos2::new(x, origin.y), Pos2::new(x, origin.y + ruler_h)],
                        Stroke::new(1.0, Color32::from_gray(80)),
                    );
                    painter.text(
                        Pos2::new(x + 2.0, origin.y + 2.0),
                        egui::Align2::LEFT_TOP,
                        fmt_duration(t),
                        egui::FontId::monospace(11.0),
                        COLOR_RULER_TEXT,
                    );
                    t += step_secs;
                }

                // ── Clips ────────────────────────────────────────────────
                let track_top = origin.y + ruler_h + 4.0;
                for clip in self.project.timeline.clips() {
                    let x0 = origin.x + clip.timeline_start as f32 * self.zoom;
                    let x1 = origin.x + clip.timeline_end() as f32 * self.zoom;
                    let clip_rect = Rect::from_min_max(
                        Pos2::new(x0, track_top),
                        Pos2::new(x1.max(x0 + 2.0), track_top + track_h),
                    );

                    let selected = self.selected_clip_id == Some(clip.id);
                    let fill = if selected { COLOR_CLIP_SELECTED } else { COLOR_CLIP };
                    painter.rect_filled(clip_rect, 3.0, fill);
                    painter.rect_stroke(clip_rect, 3.0, Stroke::new(1.0, Color32::WHITE));

                    // Clip label.
                    let label_pos = Pos2::new(x0 + 4.0, track_top + 4.0);
                    painter.text(
                        label_pos,
                        egui::Align2::LEFT_TOP,
                        &clip.label,
                        egui::FontId::proportional(12.0),
                        Color32::WHITE,
                    );
                }

                // ── Text overlay track ───────────────────────────────
                let text_track_top = track_top + track_h + track_gap;
                // Track label.
                painter.text(
                    Pos2::new(origin.x + 4.0, text_track_top + 2.0),
                    egui::Align2::LEFT_TOP,
                    "T",
                    egui::FontId::monospace(10.0),
                    Color32::from_gray(90),
                );
                // Separator line between tracks.
                painter.line_segment(
                    [
                        Pos2::new(origin.x, text_track_top - 1.0),
                        Pos2::new(origin.x + content_w, text_track_top - 1.0),
                    ],
                    Stroke::new(1.0, Color32::from_gray(50)),
                );

                let edge_grab_w = 12.0_f32;
                for overlay in self.project.timeline.text_overlays() {
                    let ox0 = origin.x + overlay.start as f32 * self.zoom;
                    let ox1 = origin.x + overlay.end as f32 * self.zoom;
                    let overlay_rect = Rect::from_min_max(
                        Pos2::new(ox0, text_track_top),
                        Pos2::new(ox1.max(ox0 + 4.0), text_track_top + text_track_h),
                    );

                    let selected = self.selected_overlay_id == Some(overlay.id);
                    let fill = if selected { COLOR_TEXT_OVERLAY_SELECTED } else { COLOR_TEXT_OVERLAY };
                    painter.rect_filled(overlay_rect, 3.0, fill);
                    painter.rect_stroke(overlay_rect, 3.0, Stroke::new(1.0, Color32::from_gray(180)));

                    // Draw resize handles on edges — visible grab bars.
                    let handle_visual_w = edge_grab_w.min((ox1 - ox0) * 0.35);
                    let handle_color = if selected {
                        Color32::from_rgb(200, 140, 0)
                    } else {
                        Color32::from_rgb(140, 100, 20)
                    };
                    // Left handle.
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(ox0, text_track_top),
                            Vec2::new(handle_visual_w, text_track_h),
                        ),
                        2.0,
                        handle_color,
                    );
                    // Left grip lines.
                    let grip_x = ox0 + handle_visual_w * 0.5;
                    for dy in [text_track_h * 0.3, text_track_h * 0.5, text_track_h * 0.7] {
                        painter.line_segment(
                            [
                                Pos2::new(grip_x - 2.0, text_track_top + dy),
                                Pos2::new(grip_x + 2.0, text_track_top + dy),
                            ],
                            Stroke::new(1.0, Color32::from_gray(40)),
                        );
                    }
                    // Right handle.
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(ox1 - handle_visual_w, text_track_top),
                            Vec2::new(handle_visual_w, text_track_h),
                        ),
                        2.0,
                        handle_color,
                    );
                    // Right grip lines.
                    let grip_x = ox1 - handle_visual_w * 0.5;
                    for dy in [text_track_h * 0.3, text_track_h * 0.5, text_track_h * 0.7] {
                        painter.line_segment(
                            [
                                Pos2::new(grip_x - 2.0, text_track_top + dy),
                                Pos2::new(grip_x + 2.0, text_track_top + dy),
                            ],
                            Stroke::new(1.0, Color32::from_gray(40)),
                        );
                    }

                    // Text label inside the block.
                    let clip_w = ox1 - ox0;
                    if clip_w > 30.0 {
                        let label = if overlay.text.len() > 20 {
                            format!("{}…", &overlay.text[..19])
                        } else {
                            overlay.text.clone()
                        };
                        painter.text(
                            Pos2::new(ox0 + handle_visual_w + 2.0, text_track_top + 3.0),
                            egui::Align2::LEFT_TOP,
                            label,
                            egui::FontId::proportional(11.0),
                            Color32::BLACK,
                        );
                    }
                }

                // ── Trim region highlight ─────────────────────────────
                let handle_size = 10.0_f32;
                let trim_handle_w = 12.0_f32;
                let ph = self.project.timeline.playhead;
                // Effective positions: if not set, they sit at the playhead.
                let left_pos = self.trim_head_left.unwrap_or(ph);
                let right_pos = self.trim_head_right.unwrap_or(ph);
                let region_left = left_pos.min(right_pos);
                let region_right = left_pos.max(right_pos);
                {
                    // Only highlight when at least one handle has been dragged away.
                    if self.trim_head_left.is_some() || self.trim_head_right.is_some() {
                        let rx0 = origin.x + region_left as f32 * self.zoom;
                        let rx1 = origin.x + region_right as f32 * self.zoom;
                        if (rx1 - rx0).abs() > 1.0 {
                            let region_rect = Rect::from_min_max(
                                Pos2::new(rx0, origin.y),
                                Pos2::new(rx1, origin.y + total_h),
                            );
                            painter.rect_filled(region_rect, 0.0, COLOR_TRIM_REGION);
                        }
                    }
                }

                // ── Playhead ─────────────────────────────────────────────
                let ph_x = origin.x + ph as f32 * self.zoom;
                // Line.
                painter.line_segment(
                    [
                        Pos2::new(ph_x, origin.y),
                        Pos2::new(ph_x, origin.y + total_h),
                    ],
                    Stroke::new(2.0, COLOR_PLAYHEAD),
                );
                // Handle: square with a small downward point.
                let handle_top = origin.y;
                let sq = handle_size * 0.8;
                painter.rect_filled(
                    Rect::from_center_size(
                        Pos2::new(ph_x, handle_top + sq * 0.5),
                        Vec2::new(sq * 2.0, sq),
                    ),
                    2.0,
                    COLOR_PLAYHEAD,
                );
                // Small downward notch.
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        Pos2::new(ph_x - 3.0, handle_top + sq),
                        Pos2::new(ph_x + 3.0, handle_top + sq),
                        Pos2::new(ph_x, handle_top + sq + 5.0),
                    ],
                    COLOR_PLAYHEAD,
                    Stroke::NONE,
                ));

                // ── Trim handles (always visible) ────────────────────
                let timeline_bottom = origin.y + total_h;
                let trim_h = handle_size * 1.5;
                let trim_y_top = handle_top;

                // Left trim handle: triangle pointing left, positioned so its
                // right (flat) edge butts against the left edge of the center square.
                {
                    let lx = origin.x + left_pos as f32 * self.zoom;
                    if self.trim_head_left.is_some() {
                        painter.line_segment(
                            [Pos2::new(lx, origin.y), Pos2::new(lx, timeline_bottom)],
                            Stroke::new(1.5, COLOR_TRIM_HANDLE),
                        );
                    }
                    // Flat edge at lx - sq so it never overlaps the center square.
                    let flat_x = lx.min(ph_x - sq);
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            Pos2::new(flat_x, trim_y_top),                              // top-right
                            Pos2::new(flat_x, trim_y_top + trim_h),                     // bottom-right
                            Pos2::new(flat_x - trim_handle_w, trim_y_top + trim_h * 0.5), // left point
                        ],
                        COLOR_TRIM_HANDLE,
                        Stroke::new(1.0, Color32::WHITE),
                    ));
                }

                // Right trim handle: triangle pointing right, positioned so its
                // left (flat) edge butts against the right edge of the center square.
                {
                    let rx = origin.x + right_pos as f32 * self.zoom;
                    if self.trim_head_right.is_some() {
                        painter.line_segment(
                            [Pos2::new(rx, origin.y), Pos2::new(rx, timeline_bottom)],
                            Stroke::new(1.5, COLOR_TRIM_HANDLE),
                        );
                    }
                    let flat_x = rx.max(ph_x + sq);
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            Pos2::new(flat_x, trim_y_top),                               // top-left
                            Pos2::new(flat_x, trim_y_top + trim_h),                      // bottom-left
                            Pos2::new(flat_x + trim_handle_w, trim_y_top + trim_h * 0.5), // right point
                        ],
                        COLOR_TRIM_HANDLE,
                        Stroke::new(1.0, Color32::WHITE),
                    ));
                }

                // ── Mouse interaction ────────────────────────────────────
                let playhead_not_playing = self.player.state() != PlaybackState::Playing;
                let snap_threshold = 0.3_f64; // seconds

                // Detect drag start: trim handles, playhead handle, clip, or empty.
                if resp.drag_started() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        let py = pos.y - origin.y;
                        let in_handle_y = py >= 0.0 && py <= trim_h + 4.0;

                        let sq_sec = sq as f64 / self.zoom as f64;
                        let tw_sec = trim_handle_w as f64 / self.zoom as f64;

                        // Left triangle: flat edge at min(left_pos, ph - sq_sec),
                        //   extends tw_sec further left.
                        let l_flat = left_pos.min(ph - sq_sec);
                        let hit_left = in_handle_y
                            && t >= l_flat - tw_sec
                            && t <= l_flat;
                        // Right triangle: flat edge at max(right_pos, ph + sq_sec),
                        //   extends tw_sec further right.
                        let r_flat = right_pos.max(ph + sq_sec);
                        let hit_right = in_handle_y
                            && t >= r_flat
                            && t <= r_flat + tw_sec;
                        // Playhead square occupies [ph - sq_sec, ph + sq_sec].
                        let hit_playhead = in_handle_y
                            && t >= ph - sq_sec
                            && t <= ph + sq_sec;

                        if hit_left {
                            self.dragging_trim_left = true;
                        } else if hit_right {
                            self.dragging_trim_right = true;
                        } else if hit_playhead {
                            self.dragging_playhead = true;
                        // Check playhead line area below the handles.
                        } else {
                        let ph_hit_half = (handle_size + 4.0) / self.zoom;
                        if py < ruler_h + 4.0
                            && (t - self.project.timeline.playhead).abs() < ph_hit_half as f64
                        {
                            self.dragging_playhead = true;
                        } else {
                            let clip_py = pos.y - (origin.y + ruler_h);
                            if clip_py >= 0.0 && clip_py <= track_h {
                                if let Some(clip) = self
                                    .project
                                    .timeline
                                    .clips()
                                    .iter()
                                    .find(|c| t >= c.timeline_start && t <= c.timeline_end())
                                {
                                    self.dragging_clip_id = Some(clip.id);
                                    self.drag_offset = t - clip.timeline_start;
                                    self.selected_clip_id = Some(clip.id);
                                }
                            }
                            // Text overlay track hit testing.
                            let text_py = pos.y - (origin.y + ruler_h + 4.0 + track_h + track_gap);
                            if text_py >= 0.0 && text_py <= text_track_h {
                                let edge_sec = edge_grab_w as f64 / self.zoom as f64;
                                if let Some(overlay) = self
                                    .project
                                    .timeline
                                    .text_overlays()
                                    .iter()
                                    .find(|o| t >= o.start && t <= o.end)
                                {
                                    let oid = overlay.id;
                                    // Check if clicking on left or right resize edge.
                                    if t <= overlay.start + edge_sec {
                                        self.dragging_overlay_left_edge = Some(oid);
                                    } else if t >= overlay.end - edge_sec {
                                        self.dragging_overlay_right_edge = Some(oid);
                                    } else {
                                        self.dragging_overlay_id = Some(oid);
                                        self.overlay_drag_offset = t - overlay.start;
                                    }
                                    self.selected_overlay_id = Some(oid);
                                }
                            }
                        }
                        }
                    }
                }

                // Dragging a trim handle.
                if resp.dragged() && self.dragging_trim_left {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        // Left handle cannot pass the playhead.
                        let clamped = t.clamp(0.0, self.project.timeline.playhead);
                        self.trim_head_left = Some(clamped);
                        if playhead_not_playing {
                            self.request_scrub_frame_at(clamped);
                        }
                    }
                } else if resp.dragged() && self.dragging_trim_right {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        // Right handle cannot pass the playhead.
                        let clamped = t.clamp(
                            self.project.timeline.playhead,
                            self.project.timeline.total_duration().max(0.0),
                        );
                        self.trim_head_right = Some(clamped);
                        if playhead_not_playing {
                            self.request_scrub_frame_at(clamped);
                        }
                    }
                // Dragging the playhead handle — snap trim heads back.
                } else if resp.dragged() && self.dragging_playhead {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        self.project.timeline.set_playhead(t);
                        // Reset trim heads so they follow the playhead.
                        self.trim_head_left = None;
                        self.trim_head_right = None;
                        if playhead_not_playing {
                            self.request_scrub_frame();
                        }
                    }
                // Dragging a clip: snap + no-overlap.
                } else if resp.dragged() && self.dragging_clip_id.is_some() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        let mut new_start = (t - self.drag_offset).max(0.0);
                        let drag_id = self.dragging_clip_id.unwrap();

                        // Get this clip's duration for overlap computation.
                        let clip_dur = self
                            .project
                            .timeline
                            .clips()
                            .iter()
                            .find(|c| c.id == drag_id)
                            .map(|c| c.duration())
                            .unwrap_or(0.0);
                        let new_end = new_start + clip_dur;

                        // Collect edges of other clips for snapping.
                        let other_edges: Vec<(f64, f64)> = self
                            .project
                            .timeline
                            .clips()
                            .iter()
                            .filter(|c| c.id != drag_id)
                            .map(|c| (c.timeline_start, c.timeline_end()))
                            .collect();

                        // Snap: this clip's start → other clip's end (and vice versa).
                        let mut best_snap: Option<f64> = None;
                        let mut best_dist = snap_threshold;
                        for &(os, oe) in &other_edges {
                            // My start → their end.
                            let d = (new_start - oe).abs();
                            if d < best_dist {
                                best_dist = d;
                                best_snap = Some(oe);
                            }
                            // My end → their start.
                            let d = (new_end - os).abs();
                            if d < best_dist {
                                best_dist = d;
                                best_snap = Some(os - clip_dur);
                            }
                            // My start → their start.
                            let d = (new_start - os).abs();
                            if d < best_dist {
                                best_dist = d;
                                best_snap = Some(os);
                            }
                        }
                        // Snap to time 0.
                        if new_start.abs() < snap_threshold && new_start.abs() < best_dist {
                            best_snap = Some(0.0);
                        }
                        if let Some(s) = best_snap {
                            new_start = s.max(0.0);
                        }

                        // Prevent overlap: clamp so we don't sit on top of others.
                        let mut clamped_start = new_start;
                        for &(os, oe) in &other_edges {
                            let cs = clamped_start;
                            let ce = clamped_start + clip_dur;
                            // If overlapping, push to the nearest side.
                            if ce > os && cs < oe {
                                let push_right = oe - cs;
                                let push_left = ce - os;
                                if push_left <= push_right {
                                    clamped_start = os - clip_dur;
                                } else {
                                    clamped_start = oe;
                                }
                            }
                        }
                        clamped_start = clamped_start.max(0.0);

                        if let Some(clip) = self.project.timeline.clip_mut(drag_id) {
                            clip.timeline_start = clamped_start;
                        }
                    }
                // Dragging a text overlay body.
                } else if resp.dragged() && self.dragging_overlay_id.is_some() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        let new_start = (t - self.overlay_drag_offset).max(0.0);
                        let oid = self.dragging_overlay_id.unwrap();
                        if let Some(overlay) = self.project.timeline.text_overlay_mut(oid) {
                            let dur = overlay.end - overlay.start;
                            overlay.start = new_start;
                            overlay.end = new_start + dur;
                        }
                    }
                // Dragging left edge of a text overlay to resize.
                } else if resp.dragged() && self.dragging_overlay_left_edge.is_some() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        let oid = self.dragging_overlay_left_edge.unwrap();
                        if let Some(overlay) = self.project.timeline.text_overlay_mut(oid) {
                            let new_start = t.clamp(0.0, overlay.end - 0.1);
                            overlay.start = new_start;
                        }
                    }
                // Dragging right edge of a text overlay to resize.
                } else if resp.dragged() && self.dragging_overlay_right_edge.is_some() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        let oid = self.dragging_overlay_right_edge.unwrap();
                        if let Some(overlay) = self.project.timeline.text_overlay_mut(oid) {
                            let new_end = t.max(overlay.start + 0.1);
                            overlay.end = new_end;
                        }
                    }
                } else if resp.dragged() {
                    // Not dragging clip or playhead → scrub the playhead.
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        self.project.timeline.set_playhead(t);
                        // Reset trim heads so they follow the playhead.
                        self.trim_head_left = None;
                        self.trim_head_right = None;
                        if playhead_not_playing {
                            self.request_scrub_frame();
                        }
                    }
                }

                // End drag.
                if resp.drag_stopped() {
                    if self.dragging_clip_id.is_some() {
                        self.project.timeline.sort_clips();
                        self.dragging_clip_id = None;
                    }
                    self.dragging_playhead = false;
                    self.dragging_trim_left = false;
                    self.dragging_trim_right = false;
                    self.dragging_overlay_id = None;
                    self.dragging_overlay_left_edge = None;
                    self.dragging_overlay_right_edge = None;
                }

                // Click (no drag): move playhead + select clip.
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let t = ((pos.x - origin.x) / self.zoom) as f64;
                        self.project.timeline.set_playhead(t);
                        // Reset trim heads so they follow the playhead.
                        self.trim_head_left = None;
                        self.trim_head_right = None;
                        if playhead_not_playing {
                            self.request_scrub_frame();
                        }
                        let py = pos.y - (origin.y + ruler_h);
                        if py >= 0.0 && py <= track_h {
                            self.selected_clip_id = self
                                .project
                                .timeline
                                .clips()
                                .iter()
                                .find(|c| {
                                    t >= c.timeline_start && t <= c.timeline_end()
                                })
                                .map(|c| c.id);
                        }
                        // Click on text track: select overlay.
                        let text_py = pos.y - (origin.y + ruler_h + 4.0 + track_h + track_gap);
                        if text_py >= 0.0 && text_py <= text_track_h {
                            self.selected_overlay_id = self
                                .project
                                .timeline
                                .text_overlays()
                                .iter()
                                .find(|o| t >= o.start && t <= o.end)
                                .map(|o| o.id);
                        }
                    }
                }
            });

        // ── Clip inspector ────────────────────────────────────────────────
        if let Some(sel_id) = self.selected_clip_id {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Selected clip:");
                let tl = &mut self.project.timeline;
                if let Some(clip) = tl.clip_mut(sel_id) {
                    ui.text_edit_singleline(&mut clip.label);
                    ui.label("  Trim:");
                    ui.add(
                        egui::DragValue::new(&mut clip.trim_start)
                            .speed(0.05)
                            .clamp_range(0.0..=clip.trim_end - 0.1)
                            .suffix("s"),
                    );
                    ui.label("–");
                    let trim_end_max = clip.trim_end; // for borrow checker
                    ui.add(
                        egui::DragValue::new(&mut clip.trim_end)
                            .speed(0.05)
                            .clamp_range((clip.trim_start + 0.1)..=trim_end_max + 3600.0)
                            .suffix("s"),
                    );
                    ui.label("  Speed:");
                    ui.add(
                        egui::DragValue::new(&mut clip.speed)
                            .speed(0.1)
                            .clamp_range(0.25..=50.0)
                            .suffix("x"),
                    );
                }
                if ui
                    .button("✂ Split")
                    .on_hover_text("Split clip at playhead")
                    .clicked()
                {
                    let playhead = self.project.timeline.playhead;
                    if let Some(_new_id) = self.project.timeline.split_clip(sel_id, playhead) {
                        self.status("Clip split at playhead");
                    }
                }
                if ui.button("🗑 Delete").clicked() {
                    self.project.timeline.remove_clip(sel_id);
                    self.selected_clip_id = None;
                    self.invalidate_resolution_cache();
                }
            });
        }

        // ── Trim heads & Cut ──────────────────────────────────────────────
        ui.separator();
        ui.horizontal(|ui| {
            let ph = self.project.timeline.playhead;
            if ui
                .button("⌊ Set Left")
                .on_hover_text("Place left trim head at playhead")
                .clicked()
            {
                self.trim_head_left = Some(ph);
            }
            if ui
                .button("Set Right ⌋")
                .on_hover_text("Place right trim head at playhead")
                .clicked()
            {
                self.trim_head_right = Some(ph);
            }

            let has_range = self.trim_range().is_some();
            if ui
                .add_enabled(has_range, egui::Button::new("✂ Cut"))
                .on_hover_text("Delete the section between the trim heads")
                .clicked()
            {
                self.perform_cut();
            }

            if ui
                .add_enabled(
                    self.trim_head_left.is_some() || self.trim_head_right.is_some(),
                    egui::Button::new("✕ Clear Trim"),
                )
                .on_hover_text("Remove both trim heads")
                .clicked()
            {
                self.trim_head_left = None;
                self.trim_head_right = None;
            }

            // Show current range info.
            if let Some((start, end)) = self.trim_range() {
                ui.label(
                    RichText::new(format!(
                        "  Range: {} – {} ({:.1}s)",
                        fmt_duration(start),
                        fmt_duration(end),
                        end - start
                    ))
                    .color(COLOR_TRIM_HANDLE),
                );
            }
        });

        // ── Text overlay controls ────────────────────────────────────────
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button("🔤 Add Text")
                .on_hover_text("Add a text callout at the playhead")
                .clicked()
            {
                let ph = self.project.timeline.playhead;
                let overlay = TextOverlay::new(0, "Text", ph, ph + 3.0);
                let id = self.project.timeline.add_text_overlay(overlay);
                self.selected_overlay_id = Some(id);
                self.status("Text overlay added");
            }

            if let Some(sel_id) = self.selected_overlay_id {
                if ui.button("🗑 Delete Text").clicked() {
                    self.project.timeline.remove_text_overlay(sel_id);
                    self.selected_overlay_id = None;
                }
            }
        });

        // ── Text overlay inspector (single compact row) ──────────────────
        if let Some(sel_id) = self.selected_overlay_id {
            if self.project.timeline.text_overlay_mut(sel_id).is_some() {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Text:");
                    let tl = &mut self.project.timeline;
                    if let Some(overlay) = tl.text_overlay_mut(sel_id) {
                        let mut buf = overlay.text.clone();
                        if ui.add(egui::TextEdit::singleline(&mut buf).desired_width(120.0)).changed() {
                            overlay.text = buf;
                        }
                        ui.label("Size:");
                        ui.add(
                            egui::DragValue::new(&mut overlay.font_size)
                                .speed(1.0)
                                .clamp_range(8.0..=200.0)
                                .suffix("px"),
                        );
                        ui.label("Color:");
                        let mut color = egui::Color32::from_rgba_unmultiplied(
                            overlay.color[0],
                            overlay.color[1],
                            overlay.color[2],
                            overlay.color[3],
                        );
                        if ui.color_edit_button_srgba(&mut color).changed() {
                            overlay.color = [color.r(), color.g(), color.b(), color.a()];
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "{} – {}",
                                fmt_duration(overlay.start),
                                fmt_duration(overlay.end),
                            ))
                            .color(Color32::from_gray(160)),
                        );
                        ui.label(
                            RichText::new("(Drag edges on timeline to resize)")
                                .color(Color32::from_gray(120))
                                .italics(),
                        );
                    }
                });
            }
        }

        // ── Zoom slider ───────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 10.0..=400.0).suffix(" px/s"));
        });
    }

    // ── Dialogs ──────────────────────────────────────────────────────────────

    fn show_export_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_export_dialog;
        egui::Window::new("Export Video")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Output file path:");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.export_path);
                    if ui.button("Browse…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Export Video")
                            .add_filter("MP4 Video", &["mp4"])
                            .add_filter("All files", &["*"])
                            .set_file_name(
                                PathBuf::from(&self.export_path)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "output.mp4".into()),
                            )
                            .save_file()
                        {
                            self.export_path = path.to_string_lossy().into_owned();
                        }
                    }
                });
                ui.add_space(8.0);

                // Show progress bar if exporting.
                if self.exporting {
                    let progress = self.export_progress.unwrap_or(0.0);
                    let bar = egui::ProgressBar::new(progress)
                        .text(format!("Exporting… {:.0}%", progress * 100.0))
                        .animate(true);
                    ui.add(bar);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Please wait — encoding video with ffmpeg")
                            .color(Color32::from_gray(160))
                            .italics(),
                    );
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("Export").clicked() {
                            self.do_export();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_export_dialog = false;
                        }
                    });
                }
            });
        self.show_export_dialog = open;
    }

    fn show_about_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About Freetasia")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("🎬 Freetasia");
                ui.label("A free, open-source screen recorder and video editor for Windows.");
                ui.add_space(4.0);
                ui.label("Built with Rust · egui · cpal · ffmpeg");
                ui.add_space(4.0);
                ui.hyperlink_to(
                    "GitHub – SpencerHD2020/Freetasia",
                    "https://github.com/SpencerHD2020/Freetasia",
                );
            });
        self.show_about = open;
    }

    // ── Actions ──────────────────────────────────────────────────────────────

    fn start_recording(&mut self) {
        match self.recorder.start_recording() {
            Ok(()) => self.status("Recording started"),
            Err(e) => self.status(format!("Failed to start recording: {e}")),
        }
    }

    fn stop_recording(&mut self) {
        if let Some(session) = self.recorder.stop_recording() {
            let dur = session.duration.as_secs_f64();
            if dur < 0.5 {
                self.status("Recording too short (<0.5 s) – discarded");
                return;
            }
            let label = format!(
                "Recording {}",
                chrono::Local::now().format("%H:%M:%S")
            );
            let clip = Clip::new(0, session.video_path, dur, label);
            let id = self.project.timeline.add_clip(clip);
            self.selected_clip_id = Some(id);
            self.invalidate_resolution_cache();
            self.status(format!("Recording added to timeline ({:.1}s)", dur));
        }
    }

    fn save_project(&mut self) {
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(self.project.default_output_name().with_extension("json"));
        match self.project.save(&path) {
            Ok(()) => self.status(format!("Saved to {}", path.display())),
            Err(e) => self.status(format!("Save failed: {e}")),
        }
    }

    fn open_project(&mut self) {
        // Without a native file picker, we read from the current directory.
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("project.json");
        match Project::load(&path) {
            Ok(p) => {
                self.project = p;
                self.selected_clip_id = None;
                self.invalidate_resolution_cache();
                self.status(format!("Opened {}", path.display()));
            }
            Err(e) => self.status(format!("Open failed: {e}")),
        }
    }

    fn do_export(&mut self) {
        let output = PathBuf::from(&self.export_path);
        let (tx, rx) = crossbeam_channel::unbounded();
        match export::export_timeline_async(&self.project.timeline, &output, tx) {
            Ok(()) => {
                self.exporting = true;
                self.export_progress = Some(0.0);
                self.export_progress_rx = Some(rx);
                self.status("Exporting…");
            }
            Err(e) => self.status(format!("Export failed: {e}")),
        }
    }

    fn poll_export_progress(&mut self) {
        let rx = match self.export_progress_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        // Drain all pending messages.
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ExportProgress::Progress(frac) => {
                    self.export_progress = Some(frac);
                }
                ExportProgress::Done => {
                    self.exporting = false;
                    self.export_progress = None;
                    self.export_progress_rx = None;
                    self.show_export_dialog = false;
                    self.status(format!("Exported to {}", self.export_path));
                    return;
                }
                ExportProgress::Error(msg) => {
                    self.exporting = false;
                    self.export_progress = None;
                    self.export_progress_rx = None;
                    self.status(format!("Export failed: {msg}"));
                    return;
                }
            }
        }
    }

    fn status(&mut self, msg: impl Into<String>) {
        self.status_msg = msg.into();
        log::info!("{}", self.status_msg);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Attempt to enumerate connected monitors and return human-readable names.
fn detect_monitor_names() -> Vec<String> {
    #[cfg(not(test))]
    {
        use screenshots::Screen;
        match Screen::all() {
            Ok(screens) if !screens.is_empty() => screens
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    format!(
                        "Monitor {} ({}×{})",
                        i + 1,
                        s.display_info.width,
                        s.display_info.height
                    )
                })
                .collect(),
            _ => vec!["Monitor 1 (unknown)".into()],
        }
    }
    #[cfg(test)]
    vec!["Monitor 1 (test)".into()]
}

/// Format a duration in seconds as `MM:SS`.
fn fmt_duration(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Format a duration in seconds as `HH:MM:SS`.
fn fmt_duration_hms(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Choose a ruler tick interval (in seconds) based on the current zoom level.
fn ruler_step_secs(zoom: f32) -> f64 {
    // Target about 80px between ticks.
    let approx = 80.0 / zoom as f64;
    for &step in &[0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0] {
        if step >= approx {
            return step;
        }
    }
    600.0
}
