//! Pneuma's monochrome, square terminal style, with actX's red wordmark.
use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Stroke};

pub const BACKGROUND: Color32 = Color32::BLACK;
pub const RAISED: Color32 = Color32::from_gray(9);
pub const HOVER: Color32 = Color32::from_gray(17);
pub const SELECTED: Color32 = Color32::from_gray(26);
pub const LINE: Color32 = Color32::from_gray(38);
pub const BORDER: Color32 = Color32::from_gray(74);
pub const TEXT: Color32 = Color32::WHITE;
pub const SECONDARY: Color32 = Color32::from_gray(206);
pub const MUTED: Color32 = Color32::from_gray(122);
pub const RED: Color32 = Color32::from_rgb(239, 68, 68);
pub const OK: Color32 = Color32::from_rgb(95, 224, 122);
pub const WARNING: Color32 = Color32::from_rgb(245, 180, 93);

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("actx-bold".into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, data) in [
        (
            "actx-mono",
            &include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf")[..],
        ),
        (
            "actx-bold",
            &include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf")[..],
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), egui::FontData::from_static(data).into());
    }
    let cjk =
        zstd::decode_all(&include_bytes!("../../assets/fonts/NotoSansSC-Actx.ttf.zst")[..]).expect("bundled CJK font");
    fonts
        .font_data
        .insert("actx-cjk".into(), egui::FontData::from_owned(cjk).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let names = fonts.families.entry(family).or_default();
        names.insert(0, "actx-mono".into());
        names.push("actx-cjk".into());
    }
    let mut fallback = fonts.families[&FontFamily::Proportional].clone();
    fallback.insert(0, "actx-bold".into());
    fonts.families.insert(FontFamily::Name("actx-bold".into()), fallback);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (egui::TextStyle::Small, FontId::monospace(11.)),
        (egui::TextStyle::Body, FontId::monospace(13.)),
        (egui::TextStyle::Button, FontId::monospace(12.)),
        (egui::TextStyle::Monospace, FontId::monospace(13.)),
        (egui::TextStyle::Heading, bold(16.)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10., 10.);
    style.spacing.button_padding = egui::vec2(10., 6.);
    style.spacing.interact_size.y = 28.;
    style.animation_time = 0.1;
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = BACKGROUND;
    visuals.window_stroke = Stroke::new(1., BORDER);
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::ZERO;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.extreme_bg_color = BACKGROUND;
    visuals.text_edit_bg_color = Some(BACKGROUND);
    visuals.faint_bg_color = RAISED;
    visuals.code_bg_color = HOVER;
    visuals.weak_text_color = Some(MUTED);
    visuals.hyperlink_color = TEXT;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = RED;
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(1., TEXT);
    for (widget, fill, border, text) in [
        (&mut visuals.widgets.noninteractive, BACKGROUND, LINE, SECONDARY),
        (&mut visuals.widgets.inactive, RAISED, BORDER, SECONDARY),
        (&mut visuals.widgets.hovered, HOVER, TEXT, TEXT),
        (&mut visuals.widgets.active, SELECTED, TEXT, TEXT),
        (&mut visuals.widgets.open, SELECTED, BORDER, TEXT),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1., border);
        widget.fg_stroke = Stroke::new(1., text);
        widget.corner_radius = egui::CornerRadius::ZERO;
        widget.expansion = 0.;
    }
    style.visuals = visuals;
    ctx.set_global_style(style);
}

pub fn dialog<'open>(title: impl Into<egui::WidgetText>) -> egui::Window<'open> {
    egui::Window::new(title).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BACKGROUND)
        .stroke(Stroke::new(1., BORDER))
        .outer_margin(5)
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 18,
            bottom: 12,
        })
}

/// Keep the heading in the widget tree while placing it across the panel's top border.
pub fn panel_title(ui: &mut egui::Ui, title: &str) {
    let font = bold(12.);
    let label = RichText::new(format!("┤ {title} ├")).font(font.clone()).color(TEXT);
    let width = ui.painter().layout_no_wrap(format!("┤ {title} ├"), font, TEXT).size().x;
    let pos = ui.max_rect().min - egui::vec2(0., 26.);
    let rect = egui::Rect::from_min_size(pos, egui::vec2(width.min(ui.available_width()) + 10., 18.));
    ui.painter().rect_filled(rect, 0., BACKGROUND);
    ui.place(rect, egui::Label::new(label).truncate());
}

pub fn project(ui: &mut egui::Ui, selected: bool, label: impl Into<String>) -> egui::Response {
    let label = format!("{} {}", if selected { ">" } else { " " }, label.into());
    ui.add_sized(
        [ui.available_width(), 28.],
        egui::Button::new(label).selected(selected).frame(selected).wrap(),
    )
}
