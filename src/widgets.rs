//! Painted icons and small controls, and the shortcuts window.

use eframe::egui::{self, pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Response, Sense, Stroke, Ui};

pub const TEXT: Color32 = Color32::from_gray(230);
pub const DIM: Color32 = Color32::from_gray(150);
pub const BAR_BG: Color32 = Color32::from_rgb(33, 34, 42);
pub const DESK: Color32 = Color32::from_rgb(22, 23, 28);
pub const SIDE_BG: Color32 = Color32::from_rgb(27, 28, 34);

pub fn icon_button(ui: &mut Ui, center: Pos2, radius: f32, id: &str, tip: &str, active: Option<Color32>, draw: impl FnOnce(&Painter, Pos2, Color32)) -> Response {
    let resp = ui.interact(Rect::from_center_size(center, vec2(radius, radius) * 2.0), ui.id().with(id), Sense::click());
    let painter = ui.painter();
    if resp.hovered() {
        painter.circle_filled(center, radius, Color32::from_white_alpha(18));
    }
    let col = match active {
        Some(c) => c,
        None if resp.hovered() => Color32::WHITE,
        None => Color32::from_gray(205),
    };
    draw(painter, center, col);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

pub fn draw_sidebar(p: &Painter, c: Pos2, col: Color32) {
    let r = Rect::from_center_size(c, vec2(16.0, 13.0));
    let s = Stroke::new(1.5_f32, col);
    p.rect_stroke(r, 2.0, s, egui::StrokeKind::Middle);
    p.line_segment([pos2(r.left() + 5.5, r.top()), pos2(r.left() + 5.5, r.bottom())], s);
}

pub fn draw_search(p: &Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.6_f32, col);
    p.circle_stroke(c + vec2(-1.5, -1.5), 5.0, s);
    p.line_segment([c + vec2(2.2, 2.2), c + vec2(6.5, 6.5)], Stroke::new(2.0_f32, col));
}

/// A crescent moon (dark pages on) or a sun.
pub fn draw_moon(p: &Painter, c: Pos2, col: Color32, bg: Color32) {
    p.circle_filled(c, 7.0, col);
    p.circle_filled(c + vec2(3.8, -3.0), 6.0, bg);
}

pub fn draw_menu(p: &Painter, c: Pos2, col: Color32) {
    for dy in [-5.0, 0.0, 5.0] {
        p.rect_filled(Rect::from_center_size(c + vec2(0.0, dy), vec2(15.0, 1.8)), 1.0, col);
    }
}

/// A chevron pointing up (`dir` = -1) or down (1), or right (0).
pub fn draw_chevron(p: &Painter, c: Pos2, col: Color32, dir: i32) {
    let s = Stroke::new(1.6_f32, col);
    let pts = match dir {
        -1 => [c + vec2(-4.5, 2.5), c + vec2(0.0, -2.0), c + vec2(4.5, 2.5)],
        1 => [c + vec2(-4.5, -2.5), c + vec2(0.0, 2.0), c + vec2(4.5, -2.5)],
        _ => [c + vec2(-2.0, -4.5), c + vec2(2.5, 0.0), c + vec2(-2.0, 4.5)],
    };
    p.add(egui::Shape::line(pts.to_vec(), s));
}

pub fn draw_close(p: &Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.6_f32, col);
    p.line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], s);
    p.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], s);
}

pub fn draw_minus(p: &Painter, c: Pos2, col: Color32) {
    p.rect_filled(Rect::from_center_size(c, vec2(10.0, 1.8)), 1.0, col);
}

pub fn draw_plus(p: &Painter, c: Pos2, col: Color32) {
    p.rect_filled(Rect::from_center_size(c, vec2(10.0, 1.8)), 1.0, col);
    p.rect_filled(Rect::from_center_size(c, vec2(1.8, 10.0)), 1.0, col);
}

/// Continuous pages (two stacked sheets) or a single sheet.
pub fn draw_layout(p: &Painter, c: Pos2, col: Color32, continuous: bool) {
    let s = Stroke::new(1.5_f32, col);
    if continuous {
        p.rect_stroke(Rect::from_center_size(c + vec2(0.0, -4.0), vec2(11.0, 7.0)), 1.5, s, egui::StrokeKind::Middle);
        p.rect_stroke(Rect::from_center_size(c + vec2(0.0, 5.0), vec2(11.0, 7.0)), 1.5, s, egui::StrokeKind::Middle);
    } else {
        p.rect_stroke(Rect::from_center_size(c, vec2(11.0, 15.0)), 1.5, s, egui::StrokeKind::Middle);
    }
}

/// The app's mark: a page with its corner folded, used on the start screen.
pub fn draw_mark(p: &Painter, c: Pos2, size: f32, col: Color32) {
    let (w, h) = (size * 0.72, size);
    let r = Rect::from_center_size(c, vec2(w, h));
    let fold = size * 0.26;
    let s = Stroke::new(size * 0.05, col);
    let pts = vec![r.left_top(), pos2(r.right() - fold, r.top()), pos2(r.right(), r.top() + fold), r.right_bottom(), r.left_bottom()];
    p.add(egui::Shape::closed_line(pts, s));
    p.add(egui::Shape::line(vec![pos2(r.right() - fold, r.top()), pos2(r.right() - fold, r.top() + fold), pos2(r.right(), r.top() + fold)], s));
    for k in 0..3 {
        let y = r.top() + h * (0.45 + 0.15 * k as f32);
        let x1 = if k == 2 { r.left() + w * 0.55 } else { r.right() - w * 0.2 };
        p.line_segment([pos2(r.left() + w * 0.2, y), pos2(x1, y)], s);
    }
}

pub fn shortcuts_window(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("Keyboard shortcuts").open(open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, -20.0)).show(ctx, |ui| {
        egui::Grid::new("keys").num_columns(2).spacing([24.0, 3.0]).show(ui, |ui| {
            for (k, v) in [
                ("j / k,  Down / Up", "Scroll down / up"),
                ("h / l,  Left / Right", "Scroll left / right (previous / next page when it fits)"),
                ("Space / Shift+Space", "Down / up a screen (also PageDown / PageUp)"),
                ("J / K", "Next / previous page"),
                ("g / G,  Home / End", "First / last page"),
                ("12 G  or  Ctrl+G", "Go to page 12 / type a page number"),
                ("Alt+Left / Alt+Right", "Back / forward after following a link"),
                ("+ / -,  Ctrl+wheel", "Zoom in / out"),
                ("0", "Actual size (100 %)"),
                ("w / f", "Fit width / fit page"),
                ("c", "Continuous / single-page"),
                ("/  or  Ctrl+F", "Search"),
                ("n / N,  F3 / Shift+F3", "Next / previous match"),
                ("Drag over text", "Select text (double-click: a word)"),
                ("Ctrl+C / Ctrl+A", "Copy / select the page's text"),
                ("i", "Dark pages (inverted colours)"),
                ("t", "Contents and thumbnails sidebar"),
                ("Ctrl+O / Ctrl+P", "Open / print"),
                ("F11", "Full screen"),
                ("F1", "This list"),
            ] {
                ui.label(egui::RichText::new(k).strong());
                ui.label(v);
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        ui.weak("Drag empty space (or with the middle button) to move the page. Drop a PDF onto the window to open it.");
    });
}
