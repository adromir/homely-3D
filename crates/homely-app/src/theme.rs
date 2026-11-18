//! Native dark slate CAD theme, dock styling, and embedded logo texture for `homely-app`.

use bevy_egui::egui;

const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo.png");

pub const BG_CANVAS: egui::Color32 = egui::Color32::from_rgb(22, 25, 31);
pub const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(29, 33, 41);
pub const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(38, 43, 53);
pub const BG_HOVER: egui::Color32 = egui::Color32::from_rgb(48, 55, 68);
pub const BORDER_SUBTLE: egui::Color32 = egui::Color32::from_rgb(52, 59, 72);
pub const ACCENT_BLUE: egui::Color32 = egui::Color32::from_rgb(79, 168, 255);
pub const ACCENT_AMBER: egui::Color32 = egui::Color32::from_rgb(240, 168, 72);
pub const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(232, 236, 242);
pub const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(154, 163, 178);

/// Applies a sleek, flat native dark CAD theme to `ctx`.
pub fn apply_native_theme(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 22.0;

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_PANEL;
    v.extreme_bg_color = BG_CANVAS;
    v.faint_bg_color = BG_ELEVATED;
    v.window_stroke = egui::Stroke::new(1.0, BORDER_SUBTLE);
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.menu_corner_radius = egui::CornerRadius::same(6);
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: egui::Color32::from_black_alpha(110),
    };
    v.window_shadow = v.popup_shadow;
    v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(79, 168, 255, 65);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT_BLUE);

    v.widgets.noninteractive.bg_fill = BG_PANEL;
    v.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER_SUBTLE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);

    v.widgets.inactive.bg_fill = BG_ELEVATED;
    v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER_SUBTLE);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(5);

    v.widgets.hovered.bg_fill = BG_HOVER;
    v.widgets.hovered.weak_bg_fill = BG_HOVER;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT_BLUE);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(5);

    v.widgets.active.bg_fill = egui::Color32::from_rgb(45, 95, 155);
    v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(45, 95, 155);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_BLUE);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.widgets.active.corner_radius = egui::CornerRadius::same(5);

    style.visuals = v;
    ctx.set_global_style(style);
}

/// Builds a matching `egui_dock::Style` for the native dark theme.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let mut ds = egui_dock::Style::from_egui(style);
    ds.tab_bar.bg_fill = BG_CANVAS;
    ds.tab_bar.height = 28.0;
    ds.tab_bar.hline_color = BORDER_SUBTLE;
    ds.separator.width = 2.0;
    ds.separator.color_idle = BORDER_SUBTLE;
    ds.separator.color_hovered = ACCENT_BLUE;
    ds.separator.color_dragged = ACCENT_BLUE;
    ds.tab.tab_body.bg_fill = BG_PANEL;
    ds.tab.tab_body.inner_margin = egui::Margin::same(8);
    ds.tab.active.bg_fill = BG_PANEL;
    ds.tab.active.text_color = TEXT_PRIMARY;
    ds.tab.inactive.bg_fill = BG_CANVAS;
    ds.tab.inactive.text_color = TEXT_MUTED;
    ds.tab.hovered.bg_fill = BG_ELEVATED;
    ds.tab.hovered.text_color = egui::Color32::WHITE;
    ds
}

/// Loads and caches the embedded Homely 3D logo texture in `egui`.
pub fn load_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let img = image::load_from_memory(LOGO_PNG)
        .expect("embedded logo.png must be valid PNG")
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    ctx.load_texture("homely_logo", color_image, egui::TextureOptions::LINEAR)
}
