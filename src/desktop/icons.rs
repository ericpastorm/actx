//! Small vector icons: the same strokes on every platform, with readable button labels.
use eframe::egui::{self, Color32, Stroke, Widget};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Settings,
    Refresh,
    Folder,
    Add,
    Sync,
    Delete,
    Recovery,
    Clean,
    Compare,
    Resume,
    Convert,
    Diagnostics,
    Automatic,
    Check,
    Cancel,
}
impl Icon {
    fn paint(self, painter: &egui::Painter, rect: egui::Rect, color: Color32) {
        let point = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (rect.width() / 20.);
        let stroke = Stroke::new(1.4, color);
        let line = |a: (f32, f32), b: (f32, f32)| {
            painter.line_segment([point(a.0, a.1), point(b.0, b.1)], stroke);
        };
        let path = |p: &[(f32, f32)]| {
            painter.add(egui::Shape::line(p.iter().map(|p| point(p.0, p.1)).collect(), stroke));
        };
        match self {
            Self::Settings => {
                let mut outline: Vec<_> = (0..32)
                    .map(|i| {
                        let angle = std::f32::consts::TAU * i as f32 / 32.;
                        let radius = if i % 4 < 2 { 8.5 } else { 6.5 };
                        point(10. + radius * angle.cos(), 10. + radius * angle.sin())
                    })
                    .collect();
                outline.push(outline[0]);
                painter.add(egui::Shape::line(outline, stroke));
                painter.circle_stroke(point(10., 10.), rect.width() * 0.15, stroke);
            }
            Self::Refresh | Self::Sync | Self::Automatic => {
                path(&[(3., 8.), (3., 4.), (8., 2.), (15., 4.), (17., 8.)]);
                path(&[(13., 8.), (17., 8.), (17., 4.)]);
                path(&[(17., 12.), (17., 16.), (12., 18.), (5., 16.), (3., 12.)]);
                path(&[(7., 12.), (3., 12.), (3., 16.)]);
            }
            Self::Folder => {
                path(&[
                    (2., 6.),
                    (2., 4.),
                    (8., 4.),
                    (10., 7.),
                    (18., 7.),
                    (18., 17.),
                    (2., 17.),
                    (2., 6.),
                ]);
            }
            Self::Add => {
                line((10., 3.), (10., 17.));
                line((3., 10.), (17., 10.));
            }
            Self::Delete => {
                path(&[(5., 6.), (6., 18.), (14., 18.), (15., 6.)]);
                line((3., 5.), (17., 5.));
                path(&[(7., 4.), (7., 2.), (13., 2.), (13., 4.)]);
                line((8., 8.), (8., 15.));
                line((12., 8.), (12., 15.));
            }
            Self::Recovery => {
                path(&[
                    (3., 9.),
                    (4., 5.),
                    (9., 3.),
                    (15., 5.),
                    (18., 10.),
                    (15., 16.),
                    (9., 18.),
                    (4., 15.),
                ]);
                path(&[(2., 4.), (2., 9.), (7., 9.)]);
                path(&[(10., 6.), (10., 11.), (13., 13.)]);
            }
            Self::Clean => {
                line((15., 2.), (8., 10.));
                path(&[(7., 9.), (3., 13.), (7., 18.), (12., 16.), (13., 14.), (7., 9.)]);
                line((4., 14.), (8., 17.));
            }
            Self::Compare => {
                path(&[(2., 4.), (7., 4.), (13., 16.), (18., 16.)]);
                path(&[(2., 16.), (7., 16.), (13., 4.), (18., 4.)]);
                path(&[(15., 1.), (18., 4.), (15., 7.)]);
                path(&[(15., 13.), (18., 16.), (15., 19.)]);
            }
            Self::Resume => path(&[(6., 3.), (17., 10.), (6., 17.), (6., 3.)]),
            Self::Convert => {
                path(&[(2., 6.), (17., 6.), (13., 2.)]);
                path(&[(18., 14.), (3., 14.), (7., 18.)]);
            }
            Self::Diagnostics => {
                path(&[(2., 11.), (5., 11.), (8., 4.), (12., 17.), (15., 10.), (18., 10.)]);
            }
            Self::Check => path(&[(3., 10.), (8., 15.), (17., 5.)]),
            Self::Cancel => {
                line((4., 4.), (16., 16.));
                line((16., 4.), (4., 16.));
            }
        }
    }

    fn atom(self, color: Option<Color32>) -> egui::Atom<'static> {
        egui::Atom::paint(egui::vec2(16., 16.), move |ui, args| {
            let color = if ui.is_enabled() {
                color.unwrap_or(args.fallback_text_color)
            } else {
                args.fallback_text_color
            };
            self.paint(ui.painter(), args.rect, color);
        })
    }
}

/// Official brand artwork, embedded locally; textures are uploaded once per GUI context.
fn paint_harness(ui: &egui::Ui, name: &str, rect: egui::Rect) {
    let bytes: &[u8] = match name {
        "claude" => include_bytes!("../../assets/harnesses/claude.rgba"),
        "codex" => include_bytes!("../../assets/harnesses/codex.rgba"),
        "opencode" => include_bytes!("../../assets/harnesses/opencode.rgba"),
        "antigravity" => include_bytes!("../../assets/harnesses/antigravity.rgba"),
        _ => return,
    };
    let id = egui::Id::unique(("harness-logo", name));
    let cached = ui.ctx().data_mut(|data| data.get_temp::<egui::TextureHandle>(id));
    let texture = cached.unwrap_or_else(|| {
        let texture = ui.ctx().load_texture(
            format!("harness-logo-{name}"),
            egui::ColorImage::from_rgba_unmultiplied([64, 64], bytes),
            egui::TextureOptions::LINEAR,
        );
        ui.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        Color32::WHITE,
    );
}

pub(super) fn harness(name: &str) -> egui::Atom<'static> {
    let name = name.to_owned();
    egui::Atom::paint(egui::vec2(16., 16.), move |ui, args| {
        paint_harness(ui, &name, args.rect)
    })
}

/// Full-width title with small logos in the metadata row beneath it.
pub(super) fn conversation(
    ui: &mut egui::Ui,
    title: &str,
    tools: &[String],
    status: &str,
    selected: bool,
    color: Color32,
) -> egui::Response {
    let width = (ui.available_width() - 2. * ui.spacing().button_padding.x).max(1.);
    let title_galley = ui.painter().layout(
        title.to_owned(),
        egui::FontId::monospace(13.),
        Color32::PLACEHOLDER,
        width,
    );
    let logo_width = (tools.len() as f32 * 16. - 4.).max(0.);
    let text_offset = if tools.is_empty() { 0. } else { logo_width + 8. };
    let status_galley = ui.painter().layout(
        status.to_owned(),
        egui::FontId::monospace(11.),
        color,
        (width - text_offset).max(1.),
    );
    let second_y = title_galley.size().y + 4.;
    let height = second_y + status_galley.size().y.max(12.);
    let tools_owned = tools.to_vec();
    let content = egui::Atom::paint(egui::vec2(width, height), move |ui, args| {
        ui.painter().galley(
            args.rect.min,
            title_galley.clone(),
            if selected {
                super::theme::TEXT
            } else {
                super::theme::SECONDARY
            },
        );
        let row = args.rect.min + egui::vec2(0., second_y);
        for (i, tool) in tools_owned.iter().enumerate() {
            paint_harness(
                ui,
                tool,
                egui::Rect::from_min_size(row + egui::vec2(i as f32 * 16., 0.), egui::vec2(12., 12.)),
            );
        }
        ui.painter()
            .galley(row + egui::vec2(text_offset, 0.), status_galley.clone(), color);
    });
    let response = ui.add(
        egui::Button::new(content)
            .selected(selected)
            .frame(selected)
            .min_size(egui::vec2(ui.available_width(), 58.)),
    );
    let names = tools.join(" · ");
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            ui.is_enabled(),
            selected,
            format!("{title} · {names} · {status}"),
        )
    });
    response.on_hover_text(names)
}
pub(super) struct IconButton {
    icon: Icon,
    label: String,
    color: Option<Color32>,
}
pub(super) fn button(icon: Icon, label: impl Into<String>) -> IconButton {
    IconButton {
        icon,
        label: label.into(),
        color: None,
    }
}
impl IconButton {
    pub(super) fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }
}
impl Widget for IconButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let mut text = egui::RichText::new(self.label);
        if let Some(color) = self.color {
            text = text.color(color);
        }
        // Separate atoms reserve the icon column on every wrapped text line.
        // The native button retains hover, disabled, focus and keyboard behavior.
        egui::Button::new((self.icon.atom(self.color), text))
            .gap(10.)
            .wrap()
            .ui(ui)
    }
}
