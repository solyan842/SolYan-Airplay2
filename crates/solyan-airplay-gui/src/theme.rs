use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};

static LIGHT_MODE: AtomicBool = AtomicBool::new(true);

pub const ACCENT: Color32 = Color32::from_rgb(255, 149, 0);
pub const GREEN: Color32 = Color32::from_rgb(52, 199, 89);
pub const RED: Color32 = Color32::from_rgb(255, 69, 58);
pub const BLUE: Color32 = Color32::from_rgb(0, 122, 255);

pub fn is_light() -> bool { LIGHT_MODE.load(Ordering::Relaxed) }

pub fn bg() -> Color32 {
    if is_light() { Color32::from_rgb(246, 246, 248) } else { Color32::from_rgb(12, 13, 16) }
}
pub fn sidebar() -> Color32 {
    if is_light() { Color32::from_rgb(250, 250, 252) } else { Color32::from_rgb(17, 18, 22) }
}
pub fn card_color() -> Color32 {
    if is_light() { Color32::WHITE } else { Color32::from_rgb(23, 24, 29) }
}
pub fn card_hover() -> Color32 {
    if is_light() { Color32::from_rgb(242, 242, 247) } else { Color32::from_rgb(29, 30, 36) }
}
pub fn border() -> Color32 {
    if is_light() { Color32::from_rgb(218, 218, 223) } else { Color32::from_rgb(47, 49, 58) }
}
pub fn text() -> Color32 {
    if is_light() { Color32::from_rgb(28, 28, 30) } else { Color32::from_rgb(238, 239, 242) }
}
pub fn muted() -> Color32 {
    if is_light() { Color32::from_rgb(99, 99, 102) } else { Color32::from_rgb(151, 154, 166) }
}
pub fn accent_soft() -> Color32 {
    if is_light() { Color32::from_rgb(255, 239, 220) } else { Color32::from_rgb(66, 39, 22) }
}
pub fn control_bg() -> Color32 {
    if is_light() { Color32::from_rgb(248, 248, 250) } else { Color32::from_rgb(34, 35, 42) }
}
pub fn control_border() -> Color32 {
    if is_light() { Color32::from_rgb(198, 198, 204) } else { Color32::from_rgb(86, 89, 102) }
}

pub fn apply(ctx: &egui::Context, light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);

    #[cfg(windows)]
    {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
        let font_path = std::path::Path::new(&windir).join("Fonts").join("segoeui.ttf");
        if let Ok(bytes) = std::fs::read(font_path) {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "solyan-segoe-ui".to_owned(),
                FontData::from_owned(bytes).into(),
            );
            if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
                family.insert(0, "solyan-segoe-ui".to_owned());
            }
            if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
                family.push("solyan-segoe-ui".to_owned());
            }
            ctx.set_fonts(fonts);
        }
    }

    let selected_theme = if light { egui::Theme::Light } else { egui::Theme::Dark };
    ctx.set_theme(selected_theme);
    let mut style = (*ctx.style_of(selected_theme)).clone();

    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.interact_size = egui::vec2(42.0, 30.0);
    style.spacing.slider_width = 320.0;
    style.spacing.slider_rail_height = 7.0;
    style.spacing.icon_width = 18.0;
    style.spacing.icon_width_inner = 11.0;

    let mut visuals = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    visuals.panel_fill = bg();
    visuals.window_fill = card_color();
    visuals.extreme_bg_color = sidebar();
    visuals.faint_bg_color = card_color();
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(2.0, ACCENT);
    visuals.slider_trailing_fill = true;
    visuals.widgets.inactive.bg_fill = control_bg();
    visuals.widgets.inactive.weak_bg_fill = card_hover();
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.1, control_border());
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.1, text());
    visuals.widgets.hovered.bg_fill = accent_soft();
    visuals.widgets.hovered.weak_bg_fill = accent_soft();
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.6, ACCENT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.3, text());
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.weak_bg_fill = accent_soft();
    visuals.widgets.active.bg_stroke = Stroke::new(1.8, ACCENT);
    visuals.widgets.open.bg_fill = card_hover();
    style.visuals = visuals;

    ctx.set_style_of(selected_theme, style);
}

pub fn card() -> Frame {
    Frame::new()
        .fill(card_color())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(16))
}

pub fn sidebar_card() -> Frame {
    Frame::new()
        .fill(sidebar())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(14))
}
