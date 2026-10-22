//! Original bitmap lettering; no font download or platform glyph fallback.
use super::theme;
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub(super) enum Style {
    #[default]
    Pixel,
    Stencil,
    Ascii,
}
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Appearance {
    pub style: Style,
    pub motion: bool,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            style: Style::Pixel,
            motion: true,
        }
    }
}
impl Appearance {
    pub fn load() -> Self {
        std::fs::read(crate::ctx::ctx().state_dir.join("desktop-appearance.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> crate::ctx::Result<()> {
        let dir = &crate::ctx::ctx().state_dir;
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(file.as_file_mut(), self)?;
        file.persist(dir.join("desktop-appearance.json"))
            .map_err(|e| crate::ctx::Error(e.to_string()))?;
        Ok(())
    }
}
pub(super) const GLYPHS: [[u8; 7]; 4] = [
    [0, 0, 14, 1, 15, 17, 15],   // a
    [0, 0, 15, 16, 16, 16, 15],  // c
    [8, 8, 30, 8, 8, 9, 6],      // t
    [17, 17, 10, 4, 10, 17, 17], // X
];
pub(super) fn wordmark(ui: &mut egui::Ui, style: Style, motion: bool) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(138., 38.), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, "actX"));
    let time = ui.input(|i| i.time) as f32;
    // A short traveling pulse rather than a flashing wordmark; inactive windows stay quiet.
    let phase = (time % 4.) / 1.2;
    let animate = motion && ui.is_rect_visible(rect) && ui.input(|i| i.viewport().focused.unwrap_or(true));
    if animate {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    let origin = rect.min + egui::vec2(3., 5.);
    let cell = 4.;
    for (letter, glyph) in GLYPHS.iter().enumerate() {
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) == 0 {
                    continue;
                }
                let pos = origin + egui::vec2((letter * 7 + col) as f32 * cell, row as f32 * cell);
                let is_x = letter == 3;
                let scan = animate && is_x && phase <= 1. && (phase * 7. - row as f32).abs() < 1.;
                let color = if scan {
                    Color32::from_rgb(255, 150, 140)
                } else if is_x {
                    theme::RED
                } else {
                    theme::TEXT
                };
                let size = if style == Style::Stencil {
                    egui::vec2(3.7, 2.5)
                } else {
                    egui::vec2(3.2, 3.2)
                };
                if style == Style::Ascii {
                    ui.painter().text(
                        pos,
                        egui::Align2::LEFT_TOP,
                        if is_x { "X" } else { "#" },
                        egui::FontId::monospace(5.),
                        color,
                    );
                } else {
                    ui.painter()
                        .rect_filled(egui::Rect::from_min_size(pos, size), 0., color);
                }
            }
        }
    }
    let x = origin + egui::vec2(23. * cell, 3. * cell);
    if motion && animate {
        let glow = 0.25 + 0.25 * (time * 2.).sin();
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(x + egui::vec2(1.5, 1.5), egui::vec2(8., 8.)),
            0.,
            egui::Stroke::new(1., theme::RED.gamma_multiply(glow)),
            egui::StrokeKind::Inside,
        );
    }
}
