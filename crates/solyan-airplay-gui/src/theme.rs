use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, Stroke};

pub const BG: Color32 = Color32::from_rgb(12, 13, 16);
pub const SIDEBAR: Color32 = Color32::from_rgb(17, 18, 22);
pub const CARD: Color32 = Color32::from_rgb(23, 24, 29);
pub const CARD_HOVER: Color32 = Color32::from_rgb(29, 30, 36);
pub const BORDER: Color32 = Color32::from_rgb(47, 49, 58);
pub const TEXT: Color32 = Color32::from_rgb(238, 239, 242);
pub const MUTED: Color32 = Color32::from_rgb(151, 154, 166);
pub const ACCENT: Color32 = Color32::from_rgb(255, 132, 43);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(66, 39, 22);
pub const GREEN: Color32 = Color32::from_rgb(87, 205, 128);
pub const RED: Color32 = Color32::from_rgb(239, 99, 99);
pub const BLUE: Color32 = Color32::from_rgb(95, 164, 255);

pub fn apply(ctx: &egui::Context) {
    // Use the native Windows UI font first. Segoe UI contains the full
    // Vietnamese glyph set and avoids tofu squares for names such as
    // "Phòng ngủ". Fall back to egui defaults if Windows font loading fails.
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

    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.slider_width = 360.0;

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = CARD;
    visuals.extreme_bg_color = SIDEBAR;
    visuals.faint_bg_color = CARD;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(2.0, ACCENT);
    visuals.slider_trailing_fill = true;
    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.bg_fill = CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(67, 69, 80));
    visuals.widgets.active.bg_fill = ACCENT_SOFT;
    visuals.widgets.active.weak_bg_fill = ACCENT_SOFT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.open.bg_fill = CARD_HOVER;
    style.visuals = visuals;

    ctx.set_style_of(egui::Theme::Dark, style);
}

pub fn card() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(16))
}

pub fn sidebar_card() -> Frame {
    Frame::new()
        .fill(SIDEBAR)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(14))
}
