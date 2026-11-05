//! The window's parts: the slim top bar, the sidebar (contents and thumbnails), the
//! page area with its mouse handling, and the floating windows and overlays.

use crate::doc::{PageText, Target, THUMB_W};
use crate::view::{Fit, ZOOM_STEPS};
use crate::widgets::*;
use crate::{win, App, Caret, Drag, Load, Sel, ACCENT};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CursorIcon, FontId, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BAR_H: f32 = 38.0;
const HIT: Color32 = Color32::from_rgba_premultiplied(120, 100, 0, 100);
const HIT_NOW: Color32 = Color32::from_rgba_premultiplied(180, 90, 0, 140);
/// Selected text: the usual blue, so it can't be mistaken for (orange) search matches.
const SELECTED: Color32 = Color32::from_rgba_premultiplied(40, 100, 200, 90);

/// The caret nearest to `p` (page points) among a page's characters.
fn caret_at(t: &PageText, p: Pos2) -> usize {
    let mut best = (f32::MAX, 0);
    for (i, ch) in t.chars.iter().enumerate() {
        let r = ch.rect;
        let dy = if p.y < r.top() { r.top() - p.y } else if p.y > r.bottom() { p.y - r.bottom() } else { 0.0 };
        let dx = if p.x < r.left() { r.left() - p.x } else if p.x > r.right() { p.x - r.right() } else { 0.0 };
        // Strongly prefer the line the pointer is on.
        let score = dy * 8.0 + dx;
        if score < best.0 {
            best = (score, i);
        }
    }
    let i = best.1;
    match t.chars.get(i) {
        Some(ch) if p.x > ch.rect.center().x => i + 1,
        Some(_) => i,
        None => 0,
    }
}

/// True when `p` is on or right next to text, where dragging selects.
fn over_text(t: &PageText, p: Pos2) -> bool {
    t.chars.iter().any(|c| c.rect.expand2(vec2(3.0, 1.5)).contains(p))
}

fn link_at(t: &PageText, p: Pos2) -> Option<&Target> {
    t.links.iter().find(|l| l.rect.contains(p)).map(|l| &l.target)
}

/// The word around character `i`.
fn word_at(t: &PageText, i: usize) -> (usize, usize) {
    let is_word = |k: usize| t.chars.get(k).is_some_and(|c| c.c.is_alphanumeric() || c.c == '_' || c.c == '\'');
    if !is_word(i) {
        return (i, (i + 1).min(t.chars.len()));
    }
    let (mut a, mut b) = (i, i + 1);
    while a > 0 && is_word(a - 1) && !t.chars[a - 1].eol {
        a -= 1;
    }
    while b < t.chars.len() && is_word(b) && !t.chars[b - 1].eol {
        b += 1;
    }
    (a, b)
}

impl App {
    pub(crate) fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("bar").exact_height(BAR_H).frame(egui::Frame::new().fill(BAR_BG)).show(ctx, |ui| {
            let full = ui.max_rect();
            let cy = full.center().y;
            let ready = self.doc.as_ref().is_some_and(|d| d.load == Load::Ready);
            let p = ui.painter().clone();

            // Left: sidebar toggle, then the page number.
            let on = self.settings.sidebar;
            if icon_button(ui, pos2(full.left() + 20.0, cy), 15.0, "side", "Contents and thumbnails (T)", on.then_some(ACCENT), draw_sidebar).clicked() {
                self.settings.sidebar = !on;
            }
            if ready {
                self.page_box(ui, pos2(full.left() + 44.0, cy));
                self.zoom_box(ui, full.center());
            } else if let Some(d) = &self.doc {
                let t = match &d.load {
                    Load::Opening => "Opening...",
                    Load::Password { .. } => "Password needed",
                    _ => "",
                };
                p.text(pos2(full.left() + 44.0, cy), Align2::LEFT_CENTER, t, FontId::proportional(13.5), DIM);
            }

            // Right: menu, dark mode, layout, search.
            let mut x = full.right() - 20.0;
            let menu = icon_button(ui, pos2(x, cy), 15.0, "menu", "", None, draw_menu);
            x -= 34.0;
            let dark = self.settings.dark;
            if icon_button(ui, pos2(x, cy), 15.0, "dark", "Dark pages (I)", dark.then_some(ACCENT), |p, c, col| draw_moon(p, c, col, BAR_BG)).clicked() {
                self.settings.dark = !dark;
            }
            if !ready {
                return self.menu_popup(ui, &menu, ready);
            }
            x -= 34.0;
            let cont = self.settings.continuous;
            let tip = if cont { "Continuous scrolling (C switches to single pages)" } else { "Single page (C switches to continuous)" };
            if icon_button(ui, pos2(x, cy), 15.0, "layout", tip, None, |p, c, col| draw_layout(p, c, col, cont)).clicked() {
                self.set_continuous(!cont);
            }
            x -= 34.0;
            if self.search_open && ready {
                self.search_box(ui, Rect::from_min_max(pos2((x - 300.0).max(full.center().x + 110.0), full.top()), pos2(x + 16.0, full.bottom())));
            } else if icon_button(ui, pos2(x, cy), 15.0, "search", "Search (/ or Ctrl+F)", None, draw_search).clicked() {
                self.search_open = true;
                self.focus_search = true;
            }

            self.menu_popup(ui, &menu, ready);
        });
    }

    fn menu_popup(&mut self, ui: &mut Ui, menu: &egui::Response, ready: bool) {
        let id = ui.id().with("menu-popup");
        if menu.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        egui::popup_below_widget(ui, id, menu, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(250.0);
            self.menu(ui, ready);
        });
    }

    fn menu(&mut self, ui: &mut Ui, ready: bool) {
        let item = |ui: &mut Ui, text: &str, key: &str, enabled: bool| ui.add_enabled(enabled, egui::Button::new(text).shortcut_text(key).frame(false).min_size(vec2(ui.available_width(), 24.0))).clicked();
        if item(ui, "Open...", "Ctrl+O", true) {
            self.open_dialog();
        }
        if item(ui, "Print...", "Ctrl+P", ready) {
            self.print();
        }
        if item(ui, "Copy selected text", "Ctrl+C", self.doc.as_ref().is_some_and(|d| d.sel.is_some())) {
            self.copy_selection();
        }
        ui.separator();
        let mut cont = self.settings.continuous;
        if ui.checkbox(&mut cont, "Continuous scrolling   (C)").changed() {
            self.set_continuous(cont);
        }
        ui.checkbox(&mut self.settings.dark, "Dark pages   (I)");
        ui.checkbox(&mut self.settings.sidebar, "Sidebar   (T)");
        if item(ui, "Full screen", "F11", true) {
            self.fullscreen = !self.fullscreen;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        ui.separator();
        if item(ui, "Keyboard shortcuts", "F1", true) {
            self.show_keys = true;
        }
        if item(ui, "Set Recto as default PDF viewer", "", true) {
            match win::register() {
                Ok(()) => {
                    win::open_default_apps();
                    self.toast("In Default apps, choose Recto for .pdf files");
                }
                Err(e) => self.toast(format!("Registration failed: {e}")),
            }
        }
    }

    /// "12 / 340": click (or Ctrl+G) to type a page number.
    fn page_box(&mut self, ui: &mut Ui, left: Pos2) {
        let (cur, n) = {
            let v = self.doc.as_mut().unwrap().view.as_mut().unwrap();
            (v.current(), v.pages())
        };
        let font = FontId::proportional(14.0);
        if let Some(text) = &mut self.page_edit {
            let r = Rect::from_min_size(left - vec2(0.0, 12.0), vec2(56.0, 24.0));
            let resp = ui.put(r, egui::TextEdit::singleline(text).id(egui::Id::new("page-field")).font(font.clone()).horizontal_align(egui::Align::Center).char_limit(6));
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
            }
            ui.painter().text(pos2(r.right() + 6.0, r.center().y), Align2::LEFT_CENTER, format!("/ {n}"), font, DIM);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if resp.lost_focus() {
                let go = text.trim().parse::<usize>().ok().filter(|_| enter);
                self.page_edit = None;
                if let Some(p) = go {
                    self.jump(p.clamp(1, n) - 1, None);
                }
            }
            return;
        }
        let label = format!("{} / {}", cur + 1, n);
        let g = ui.painter().layout_no_wrap(label, font, TEXT);
        let r = Rect::from_min_size(left - vec2(6.0, 12.0), vec2(g.size().x + 12.0, 24.0));
        let resp = ui.interact(r, ui.id().with("pagebox"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, Color32::from_white_alpha(14));
        }
        ui.painter().galley(pos2(left.x, r.center().y - g.size().y / 2.0), g, TEXT);
        if resp.on_hover_text("Go to page (Ctrl+G)").clicked() {
            self.page_edit = Some(String::new());
        }
    }

    /// "- 125 % +" in the middle of the bar; the percentage opens the zoom choices.
    fn zoom_box(&mut self, ui: &mut Ui, c: Pos2) {
        let (zoom, fit) = {
            let v = self.doc.as_ref().unwrap().view.as_ref().unwrap();
            (v.zoom, v.fit)
        };
        if icon_button(ui, c - vec2(44.0, 0.0), 13.0, "zoom-out", "Zoom out (-)", None, draw_minus).clicked() {
            self.zoom_step(-1, None);
        }
        if icon_button(ui, c + vec2(44.0, 0.0), 13.0, "zoom-in", "Zoom in (+)", None, draw_plus).clicked() {
            self.zoom_step(1, None);
        }
        let label = match fit {
            Fit::Width => "Fit width".to_owned(),
            Fit::Page => "Fit page".to_owned(),
            Fit::None => format!("{:.0} %", zoom * 100.0),
        };
        let r = Rect::from_center_size(c, vec2(64.0, 24.0));
        let resp = ui.interact(r, ui.id().with("zoombox"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, Color32::from_white_alpha(14));
        }
        ui.painter().text(c, Align2::CENTER_CENTER, label, FontId::proportional(13.5), TEXT);
        let tip = format!("Zoom {:.0} %", zoom * 100.0);
        let resp = resp.on_hover_text(tip);
        let id = ui.id().with("zoom-popup");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(150.0);
            let item = |ui: &mut Ui, text: &str, key: &str, on: bool| {
                let t = if on { RichText::new(text).color(ACCENT) } else { RichText::new(text) };
                ui.add(egui::Button::new(t).shortcut_text(key).frame(false).min_size(vec2(ui.available_width(), 22.0))).clicked()
            };
            if item(ui, "Fit width", "W", fit == Fit::Width) {
                self.set_fit(Fit::Width);
            }
            if item(ui, "Fit page", "F", fit == Fit::Page) {
                self.set_fit(Fit::Page);
            }
            ui.separator();
            for &z in ZOOM_STEPS.iter().filter(|&&z| (0.25..=4.0).contains(&z) && ![0.33, 0.67, 0.9, 1.1].contains(&z)) {
                let key = if z == 1.0 { "0" } else { "" };
                if item(ui, &format!("{:.0} %", z * 100.0), key, fit == Fit::None && (zoom - z).abs() < 0.005) {
                    if let Some(d) = self.doc.as_mut() {
                        if let Some(v) = d.view.as_mut() {
                            v.set_zoom(z, None);
                            d.zoomed_at = Instant::now();
                        }
                    }
                }
            }
        });
    }

    fn search_box(&mut self, ui: &mut Ui, area: Rect) {
        let cy = area.center().y;
        // From the right: close, next, previous, the count, then the text field.
        let close_c = pos2(area.right() - 14.0, cy);
        if icon_button(ui, close_c, 12.0, "s-close", "Close search (Esc)", None, draw_close).clicked() {
            self.close_search();
            return;
        }
        if icon_button(ui, close_c - vec2(26.0, 0.0), 12.0, "s-next", "Next match (Enter, n, F3)", None, |p, c, col| draw_chevron(p, c, col, 1)).clicked() {
            self.next_hit(1);
        }
        if icon_button(ui, close_c - vec2(52.0, 0.0), 12.0, "s-prev", "Previous match (Shift+Enter, N)", None, |p, c, col| draw_chevron(p, c, col, -1)).clicked() {
            self.next_hit(-1);
        }
        let status = {
            let d = self.doc.as_ref().unwrap();
            let p = d.search.progress.lock().unwrap();
            if self.search_sent.is_empty() || p.query != self.search_sent {
                String::new()
            } else if p.failed {
                "Can't search".into()
            } else if p.hits.is_empty() {
                if p.done { "No matches".into() } else { format!("{} %", p.scanned * 100 / p.total.max(1)) }
            } else {
                let n = p.hits.len();
                let more = if p.done { "" } else { "+" };
                match self.hit {
                    Some(h) => format!("{} of {n}{more}", h + 1),
                    None => format!("{n}{more}"),
                }
            }
        };
        let count_right = close_c.x - 68.0;
        ui.painter().text(pos2(count_right, cy), Align2::RIGHT_CENTER, &status, FontId::proportional(12.5), DIM);
        let field = Rect::from_min_max(pos2(area.left(), cy - 12.0), pos2(count_right - 74.0, cy + 12.0));
        let before = self.search_text.clone();
        let resp = ui.put(field, egui::TextEdit::singleline(&mut self.search_text).id(egui::Id::new("search-field")).hint_text("Search").desired_width(field.width()));
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if self.search_text != before {
            self.search_edit = Instant::now();
        }
        if !self.search_sent.is_empty() || self.search_text.trim() != self.search_sent {
            ui.ctx().request_repaint_after(Duration::from_millis(260));
        }
    }

    pub(crate) fn sidebar(&mut self, ctx: &egui::Context) {
        let width = self.settings.sidebar_width;
        egui::SidePanel::left("side")
            .resizable(true)
            .default_width(width)
            .width_range(160.0..=520.0)
            .frame(egui::Frame::new().fill(SIDE_BG).inner_margin(egui::Margin { left: 8, right: 4, top: 8, bottom: 4 }))
            .show(ctx, |ui| {
                self.settings.sidebar_width = ui.available_width() + 12.0;
                let has_toc = self.doc.as_ref().is_some_and(|d| !d.toc.is_empty());
                ui.horizontal(|ui| {
                    if has_toc {
                        ui.selectable_value(&mut self.settings.sidebar_tab, 0, "Contents");
                    }
                    ui.selectable_value(&mut self.settings.sidebar_tab, 1, "Pages");
                });
                ui.add_space(4.0);
                let tab = if has_toc { self.settings.sidebar_tab } else { 1 };
                if tab == 0 {
                    self.contents(ui);
                    self.doc.as_mut().unwrap().side_thumbs.clear();
                } else {
                    self.thumbnails(ui);
                }
            });
    }

    fn contents(&mut self, ui: &mut Ui) {
        let mut go = None;
        let d = self.doc.as_mut().unwrap();
        // The reading position: a third down the window.
        let (cur, cur_y) = d.view.as_mut().map_or((0, 0.0), |v| {
            let a = v.anchor_at(v.viewport.min + vec2(v.viewport.width() / 2.0, v.viewport.height() * 0.3));
            (a.page, a.at.y)
        });
        // Rows to show: skip what's inside closed entries.
        let mut rows = Vec::new();
        let mut i = 0;
        while i < d.toc.len() {
            rows.push(i);
            i += if d.toc_open.contains(&i) { 1 } else { 1 + d.toc[i].descendants };
        }
        // The section being read: the last entry starting at or before this page.
        let here = d.toc.iter().rposition(|t| matches!(t.target, Some(Target::Page { page, y }) if page < cur || page == cur && y.unwrap_or(0.0) <= cur_y.max(0.0) + 1.0));
        // Inside a closed entry, its visible ancestor is marked instead.
        let mark = here.and_then(|h| rows.iter().copied().rfind(|&r| r <= h && r + d.toc[r].descendants >= h));
        let row_h = 24.0;
        let mut area = egui::ScrollArea::vertical().id_salt("toc").auto_shrink([false, false]);
        // Keep the section being read in view, unless the pointer is in the list.
        let key = (0, mark.unwrap_or(usize::MAX));
        if self.follow != Some(key) && (self.follow.map(|f| f.0) != Some(0) || !ui.rect_contains_pointer(ui.max_rect())) {
            if let Some(row) = mark.and_then(|m| rows.iter().position(|&r| r == m)) {
                area = area.vertical_scroll_offset((row as f32 * (row_h + ui.spacing().item_spacing.y) - ui.available_height() / 3.0).max(0.0));
            }
            self.follow = Some(key);
        }
        area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let t = &d.toc[i];
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                let indent = 4.0 + t.depth as f32 * 14.0;
                let open = d.toc_open.contains(&i);
                let text_rect = Rect::from_min_max(pos2(r.left() + indent + 16.0, r.top()), r.max);
                let resp = ui.interact(text_rect, ui.id().with(("toc", i)), Sense::click());
                if Some(i) == mark {
                    ui.painter().rect_filled(r, 4.0, ACCENT.gamma_multiply(0.22));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, Color32::from_white_alpha(10));
                }
                if t.descendants > 0 {
                    let c = pos2(r.left() + indent + 6.0, r.center().y);
                    let arrow = ui.interact(Rect::from_center_size(c, vec2(16.0, row_h)), ui.id().with(("toc-open", i)), Sense::click());
                    let col = if arrow.hovered() { Color32::WHITE } else { DIM };
                    draw_chevron(ui.painter(), c, col, if open { 1 } else { 0 });
                    if arrow.clicked() {
                        if open {
                            d.toc_open.remove(&i);
                        } else {
                            d.toc_open.insert(i);
                        }
                    }
                }
                let col = if Some(i) == mark { Color32::WHITE } else { TEXT };
                let g = line(ui.painter(), &t.title, 13.5, col, text_rect.width() - 4.0);
                ui.painter().galley(pos2(text_rect.left(), r.center().y - g.size().y / 2.0), g, col);
                let resp = if t.title.chars().count() > 30 { resp.on_hover_text(&t.title) } else { resp };
                if resp.clicked() {
                    go = t.target.clone();
                }
            }
        });
        if let Some(t) = go {
            self.follow(&t);
        }
    }

    fn thumbnails(&mut self, ui: &mut Ui) {
        let dark = self.settings.dark;
        let mut go = None;
        let d = self.doc.as_mut().unwrap();
        let Some(v) = d.view.as_mut() else { return };
        let cur = v.current();
        let n = v.pages();
        let w = (ui.available_width() - 28.0).clamp(60.0, THUMB_W as f32);
        let aspect = v.sizes[0].y / v.sizes[0].x.max(1.0);
        let cell_h = (w * aspect).min(w * 1.6) + 30.0;
        d.side_thumbs.clear();
        let mut area = egui::ScrollArea::vertical().id_salt("thumbs").auto_shrink([false, false]);
        // Keep the current page in view while reading (not while the user scrolls here).
        let key = (1, cur);
        if self.follow != Some(key) && (self.follow.map(|f| f.0) != Some(1) || !ui.rect_contains_pointer(ui.max_rect())) {
            area = area.vertical_scroll_offset((cur as f32 * (cell_h + ui.spacing().item_spacing.y) - ui.available_height() / 2.0 + cell_h / 2.0).max(0.0));
            self.follow = Some(key);
        }
        area.show_rows(ui, cell_h, n, |ui, range| {
            for i in range {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), cell_h), Sense::click());
                d.side_thumbs.push(i);
                let sz = v.sizes[i];
                let tw = w.min((cell_h - 30.0) * sz.x / sz.y.max(1.0));
                let th = tw * sz.y / sz.x.max(1.0);
                let img = Rect::from_min_size(pos2(r.center().x - tw / 2.0, r.top() + 6.0), vec2(tw, th));
                if i == cur {
                    ui.painter().rect_filled(img.expand(4.0), 5.0, ACCENT.gamma_multiply(0.45));
                } else if resp.hovered() {
                    ui.painter().rect_filled(img.expand(4.0), 5.0, Color32::from_white_alpha(20));
                }
                match d.thumbs.get(&i).or_else(|| d.tex.get(&i)) {
                    Some(t) => {
                        ui.painter().image(t.tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.painter().rect_filled(img, 0.0, if dark { Color32::from_gray(40) } else { Color32::from_gray(225) });
                    }
                }
                let col = if i == cur { Color32::WHITE } else { DIM };
                ui.painter().text(pos2(r.center().x, img.bottom() + 13.0), Align2::CENTER_CENTER, (i + 1).to_string(), FontId::proportional(12.0), col);
                if resp.clicked() {
                    go = Some(i);
                }
            }
        });
        if let Some(i) = go {
            self.jump(i, None);
        }
    }

    /// The document itself, or the start / error / password screens.
    pub(crate) fn pages(&mut self, ui: &mut Ui) {
        let area = ui.max_rect();
        self.area = area;
        let state = self.doc.as_ref().map(|d| match &d.load {
            Load::Failed(e) => Some(e.clone()),
            _ => None,
        });
        match state {
            None => return self.start_screen(ui, area),
            Some(Some(e)) => return self.error_screen(ui, area, &e),
            _ => {}
        }
        let dark = self.settings.dark;
        let ppp = ui.ctx().pixels_per_point();
        let frame = self.frame;
        let continuous = self.settings.continuous;
        let d = self.doc.as_mut().unwrap();
        let Some(v) = d.view.as_mut() else { return };

        // Layout: viewport, fit zoom.
        if v.viewport != area {
            // Keep the spot at the top centre where it was, relative to the new size.
            let mut a = v.anchor();
            v.viewport = area;
            a.screen.x = area.width() / 2.0;
            v.restore(a);
        }
        let key = (area, v.fit, d.sizes_known, if continuous { 0 } else { v.single }, continuous);
        if key != d.fit_key {
            d.fit_key = key;
            let z = v.zoom;
            v.apply_fit();
            if v.zoom != z {
                d.zoomed_at = Instant::now();
            }
        }

        let resp = ui.interact(area, ui.id().with("pages"), Sense::click_and_drag());
        let hover = resp.hover_pos();

        // Wheel: scroll; Ctrl+wheel (or a pinch): zoom around the pointer.
        if resp.hovered() || resp.dragged() {
            // Raw wheel steps feed our own glide, which is paced (see `pace`).
            let (scroll, zoom) = ui.input(|i| (if i.modifiers.command { egui::Vec2::ZERO } else { i.raw_scroll_delta }, i.zoom_delta()));
            if zoom != 1.0 {
                v.set_zoom(v.zoom * zoom, hover);
                d.zoomed_at = Instant::now();
                self.glide = None;
            }
            if scroll != egui::Vec2::ZERO {
                let before = self.glide.unwrap_or(v.scroll);
                let c = v.content();
                let target = (before - scroll).clamp(egui::Vec2::ZERO, (c - v.viewport.size()).max(egui::Vec2::ZERO));
                self.glide = Some(target);
                // Single-page mode: scrolling on past the page's end turns the page.
                if !v.continuous && scroll.y != 0.0 && (target.y - before.y).abs() < 0.5 {
                    self.flip -= scroll.y;
                    if self.flip.abs() > 90.0 {
                        let dir = self.flip.signum() as i64;
                        self.flip = 0.0;
                        let to = (v.single as i64 + dir).clamp(0, v.pages() as i64 - 1) as usize;
                        if to != v.single {
                            self.glide = None;
                            v.go_to(to, None);
                            if dir < 0 {
                                v.scroll.y = f32::MAX;
                                v.clamp();
                            }
                        }
                    }
                } else {
                    self.flip = 0.0;
                }
            }
        }
        if let Some(target) = self.glide {
            let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
            let k = 1.0 - (-dt * 20.0).exp();
            v.scroll += (target - v.scroll) * k;
            if (target - v.scroll).length() < 0.6 {
                v.scroll = target;
                self.glide = None;
            }
            v.clamp();
            crate::pace(ui.ctx());
        }

        // Pointer on the page: links, text, selection, panning.
        let at = hover.and_then(|p| v.hit(p));
        let text_here = at.and_then(|(pg, _)| d.texts.get(&pg).cloned());
        let link = match (&at, &text_here) {
            (Some((_, p)), Some(t)) => link_at(t, *p).cloned(),
            _ => None,
        };
        let on_text = matches!((&at, &text_here), (Some((_, p)), Some(t)) if over_text(t, *p));
        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).and_then(|p| v.hit(p));
            let starts_on_text = origin.and_then(|(pg, p)| d.texts.get(&pg).map(|t| over_text(t, p))).unwrap_or(false);
            if starts_on_text {
                let (pg, p) = origin.unwrap();
                let c = Caret { page: pg, at: caret_at(&d.texts[&pg], p) };
                d.sel = Some(Sel { anchor: c, focus: c });
                d.drag = Some(Drag::Select);
            } else {
                d.drag = Some(Drag::Pan);
            }
        } else if resp.drag_started_by(PointerButton::Middle) {
            d.drag = Some(Drag::Pan);
        }
        match d.drag {
            Some(Drag::Pan) if resp.dragged() => {
                v.scroll -= resp.drag_delta();
                v.clamp();
                self.glide = None;
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            Some(Drag::Select) if resp.dragged() => {
                if let Some(p) = resp.interact_pointer_pos() {
                    // Scroll while dragging past the top or bottom edge.
                    let edge = if p.y < area.top() + 20.0 { p.y - area.top() - 20.0 } else if p.y > area.bottom() - 20.0 { p.y - area.bottom() + 20.0 } else { 0.0 };
                    if edge != 0.0 {
                        v.scroll.y += edge * 0.5;
                        v.clamp();
                        crate::pace(ui.ctx());
                    }
                    if let Some((pg, pt)) = v.hit(p) {
                        if let (Some(t), Some(s)) = (d.texts.get(&pg), d.sel.as_mut()) {
                            s.focus = Caret { page: pg, at: caret_at(t, pt) };
                        }
                    }
                }
                ui.ctx().set_cursor_icon(CursorIcon::Text);
            }
            _ => {}
        }
        if resp.drag_stopped() {
            d.drag = None;
            if d.sel.is_some_and(|s| s.anchor == s.focus) {
                d.sel = None;
            }
        }
        let mut follow = None;
        if resp.clicked() {
            match &link {
                Some(t) => follow = Some(t.clone()),
                None => d.sel = None,
            }
        }
        if resp.double_clicked() {
            if let (Some((pg, p)), Some(t)) = (at, &text_here) {
                if over_text(t, p) {
                    let i = caret_at(t, p);
                    let i = if i > 0 && t.chars.get(i).is_none_or(|c| !c.rect.expand(1.0).contains(p)) { i - 1 } else { i };
                    let (a, b) = word_at(t, i);
                    d.sel = Some(Sel { anchor: Caret { page: pg, at: a }, focus: Caret { page: pg, at: b } });
                }
            }
        }
        if resp.triple_clicked() {
            if let (Some((pg, p)), Some(t)) = (at, &text_here) {
                // The whole line.
                let i = caret_at(t, p).min(t.chars.len().saturating_sub(1));
                let mut a = i;
                while a > 0 && !t.chars[a - 1].eol {
                    a -= 1;
                }
                let mut b = i;
                while b < t.chars.len() && !t.chars[b].eol {
                    b += 1;
                }
                d.sel = Some(Sel { anchor: Caret { page: pg, at: a }, focus: Caret { page: pg, at: (b + 1).min(t.chars.len()) } });
            }
        }
        if d.drag.is_none() && resp.hovered() {
            if let Some(Target::Uri(u)) = &link {
                resp.clone().on_hover_text_at_pointer(u.as_str());
            }
            if link.is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            } else if on_text {
                ui.ctx().set_cursor_icon(CursorIcon::Text);
            }
        }

        // Draw.
        let painter = ui.painter_at(area);
        let page_bg = if dark { Color32::from_rgb(30, 30, 30) } else { Color32::WHITE };
        let hits = d.search.progress.lock().unwrap();
        let show_hits = !self.search_sent.is_empty() && hits.query == self.search_sent;
        let mut drawn_full = false;
        for i in v.visible(0.0) {
            let r = v.screen_rect(i, ppp);
            painter.rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(90));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            let best = [d.tex.get(&i).filter(|t| t.invert == dark), d.thumbs.get(&i).filter(|t| t.invert == dark), d.tex.get(&i), d.thumbs.get(&i)].into_iter().flatten().next();
            match best {
                Some(t) => {
                    painter.image(t.tex.id(), r, uv, Color32::WHITE);
                    if d.tex.get(&i).is_some_and(|x| std::ptr::eq(x, t)) {
                        drawn_full = true;
                    }
                }
                None => {
                    painter.rect_filled(r, 0.0, page_bg);
                    let text = if d.broken.contains(&i) { format!("Page {} can't be shown (damaged)", i + 1) } else { (i + 1).to_string() };
                    painter.text(r.center(), Align2::CENTER_CENTER, text, FontId::proportional(if d.broken.contains(&i) { 15.0 } else { 28.0 }), Color32::from_gray(140));
                }
            }
            if let Some(t) = d.tex.get_mut(&i) {
                t.used = frame;
            }
            let s = v.px_per_pt();
            let on_screen = |rect: Rect| Rect::from_min_size(r.min + rect.min.to_vec2() * s, rect.size() * s);
            // Search matches.
            if show_hits {
                let from = hits.hits.partition_point(|h| h.page < i);
                for (k, h) in hits.hits[from..].iter().enumerate().take_while(|(_, h)| h.page == i) {
                    let now = self.hit == Some(from + k);
                    let hr = on_screen(h.rect).expand(1.0);
                    painter.rect_filled(hr, 2.0, if now { HIT_NOW } else { HIT });
                    if now {
                        painter.rect_stroke(hr.expand(1.0), 2.0, Stroke::new(1.5_f32, Color32::from_rgb(255, 150, 40)), egui::StrokeKind::Outside);
                    }
                }
            }
            // Selection, one rectangle per run of characters on a line.
            if let (Some(sel), Some(t)) = (d.sel, d.texts.get(&i)) {
                let range = sel.on_page(i, t.chars.len());
                let mut run: Option<Rect> = None;
                for k in range.clone() {
                    let cr = t.chars[k].rect;
                    run = Some(match run {
                        Some(rr) if (rr.center().y - cr.center().y).abs() < rr.height() * 0.5 && cr.left() >= rr.left() - 1.0 => rr.union(cr),
                        Some(rr) => {
                            painter.rect_filled(on_screen(rr), 1.0, SELECTED);
                            cr
                        }
                        None => cr,
                    });
                    if t.chars[k].eol {
                        if let Some(rr) = run.take() {
                            painter.rect_filled(on_screen(rr), 1.0, SELECTED);
                        }
                    }
                }
                if let Some(rr) = run {
                    painter.rect_filled(on_screen(rr), 1.0, SELECTED);
                }
            }
        }
        drop(hits);
        if drawn_full {
            d.page_shown = true;
        }
        self.scrollbars(ui, area);
        if let Some(t) = follow {
            self.follow(&t);
        }
        if !self.count.is_empty() {
            let c = self.count.clone();
            self.badge(ui, area, &format!("Go to page {c}  (G or Enter)"));
        }
    }

    fn badge(&self, ui: &Ui, area: Rect, text: &str) {
        let p = ui.painter();
        let g = p.layout_no_wrap(text.to_owned(), FontId::proportional(14.0), TEXT);
        let r = Rect::from_center_size(pos2(area.center().x, area.bottom() - 40.0), g.size() + vec2(28.0, 14.0));
        p.rect_filled(r, r.height() / 2.0, Color32::from_rgba_unmultiplied(20, 21, 26, 235));
        p.galley(r.center() - g.size() / 2.0, g, TEXT);
    }

    fn scrollbars(&mut self, ui: &mut Ui, area: Rect) {
        let Some(v) = self.doc.as_mut().and_then(|d| d.view.as_mut()) else { return };
        let c = v.content();
        let mut dragged = false;
        // Vertical.
        if c.y > area.height() + 1.0 {
            let track = Rect::from_min_max(pos2(area.right() - 12.0, area.top() + 2.0), pos2(area.right() - 2.0, area.bottom() - 2.0));
            let resp = ui.interact(track, ui.id().with("vbar"), Sense::click_and_drag());
            let len = (track.height() * area.height() / c.y).max(28.0);
            let room = track.height() - len;
            let max = c.y - area.height();
            let y = track.top() + room * (v.scroll.y / max).clamp(0.0, 1.0);
            let active = resp.hovered() || resp.dragged();
            let w = if active { 8.0 } else { 5.0 };
            let thumb = Rect::from_min_size(pos2(track.right() - w - 1.0, y), vec2(w, len));
            ui.painter().rect_filled(thumb, w / 2.0, Color32::from_white_alpha(if active { 110 } else { 55 }));
            if resp.dragged() || resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    v.scroll.y = ((p.y - track.top() - len / 2.0) / room.max(1.0)).clamp(0.0, 1.0) * max;
                    dragged = true;
                }
            }
        }
        // Horizontal, when a page in view is wider than the window.
        let wide = v.visible(0.0).any(|i| v.page_rect(i).width() + 2.0 * crate::view::GAP > area.width() + 1.0);
        if c.x > area.width() + 1.0 && (wide || v.scroll.x > 0.0) {
            let track = Rect::from_min_max(pos2(area.left() + 2.0, area.bottom() - 12.0), pos2(area.right() - 14.0, area.bottom() - 2.0));
            let resp = ui.interact(track, ui.id().with("hbar"), Sense::click_and_drag());
            let len = (track.width() * area.width() / c.x).max(28.0);
            let room = track.width() - len;
            let max = c.x - area.width();
            let x = track.left() + room * (v.scroll.x / max).clamp(0.0, 1.0);
            let active = resp.hovered() || resp.dragged();
            let h = if active { 8.0 } else { 5.0 };
            let thumb = Rect::from_min_size(pos2(x, track.bottom() - h - 1.0), vec2(len, h));
            ui.painter().rect_filled(thumb, h / 2.0, Color32::from_white_alpha(if active { 110 } else { 55 }));
            if resp.dragged() || resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    v.scroll.x = ((p.x - track.left() - len / 2.0) / room.max(1.0)).clamp(0.0, 1.0) * max;
                    dragged = true;
                }
            }
        }
        if dragged {
            v.clamp();
            self.glide = None;
        }
    }

    fn start_screen(&mut self, ui: &mut Ui, area: Rect) {
        let c = area.center();
        let p = ui.painter().clone();
        draw_mark(&p, c - vec2(0.0, 70.0), 64.0, ACCENT);
        p.text(c + vec2(0.0, 0.0), Align2::CENTER_CENTER, "Recto", FontId::proportional(30.0), TEXT);
        p.text(c + vec2(0.0, 34.0), Align2::CENTER_CENTER, "Open a PDF with Ctrl+O, or drop one here", FontId::proportional(14.0), DIM);
        let b = ui.put(Rect::from_center_size(c + vec2(0.0, 76.0), vec2(120.0, 30.0)), egui::Button::new("Open..."));
        if b.clicked() {
            self.open_dialog();
        }
        p.text(c + vec2(0.0, 116.0), Align2::CENTER_CENTER, "F1: keyboard shortcuts", FontId::proportional(12.5), Color32::from_gray(110));
    }

    fn error_screen(&mut self, ui: &mut Ui, area: Rect, e: &str) {
        let c = area.center();
        let name = self.doc.as_ref().map_or(String::new(), |d| d.name.clone());
        let p = ui.painter().clone();
        p.text(c - vec2(0.0, 30.0), Align2::CENTER_CENTER, format!("Can't open {name}"), FontId::proportional(18.0), Color32::from_rgb(255, 120, 110));
        let e = format!("It isn't a PDF, or it is damaged ({e}).");
        let g = p.layout(e, FontId::proportional(13.5), DIM, (area.width() - 80.0).min(600.0));
        p.galley(c - vec2(g.size().x / 2.0, 6.0), g, DIM);
        if ui.put(Rect::from_center_size(c + vec2(0.0, 50.0), vec2(150.0, 30.0)), egui::Button::new("Open another...")).clicked() {
            self.open_dialog();
        }
    }

    pub(crate) fn overlays(&mut self, ctx: &egui::Context) {
        // Password prompt.
        let mut cancel = false;
        if let Some(d) = self.doc.as_mut() {
            if let Load::Password { wrong } = d.load {
                let mut submit = false;
                egui::Window::new("Password required").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, -40.0)).show(ctx, |ui| {
                    ui.label(format!("{} is protected with a password.", d.name));
                    ui.add_space(4.0);
                    let r = ui.add(egui::TextEdit::singleline(&mut d.password).password(true).desired_width(260.0).hint_text("Password"));
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    if wrong {
                        ui.colored_label(Color32::from_rgb(255, 120, 110), "Wrong password, try again.");
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            submit = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit {
                    d.pool.password(std::mem::take(&mut d.password));
                    d.load = Load::Opening;
                }
            }
        }
        if cancel {
            self.doc = None;
        }

        if self.show_keys {
            let mut open = true;
            shortcuts_window(ctx, &mut open);
            self.show_keys = open;
        }

        // Toast, or the print status.
        let printing = self.printing.as_ref().and_then(|s| s.lock().unwrap().clone());
        if printing.is_none() {
            self.printing = None;
        }
        let toast = printing.or_else(|| match &self.toast {
            Some((t, at)) if at.elapsed() < Duration::from_millis(2500) => Some(t.clone()),
            _ => None,
        });
        if let Some(t) = toast {
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("toast")));
            let area = self.area;
            let g = p.layout_no_wrap(t, FontId::proportional(14.0), TEXT);
            let r = Rect::from_center_size(pos2(area.center().x, area.bottom() - 40.0), g.size() + vec2(28.0, 14.0));
            p.rect_filled(r, r.height() / 2.0, Color32::from_rgba_unmultiplied(20, 21, 26, 235));
            p.galley(r.center() - g.size() / 2.0, g, TEXT);
            ctx.request_repaint_after(Duration::from_millis(300));
        }

        // A file being dragged over the window.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            let r = ctx.screen_rect();
            p.rect_filled(r, 0.0, Color32::from_black_alpha(150));
            p.rect_stroke(r.shrink(12.0), 10.0, Stroke::new(2.0_f32, ACCENT), egui::StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, "Drop to open", FontId::proportional(22.0), TEXT);
        }
    }
}

/// A single line of text, cut off with "…" to fit `width`.
fn line(painter: &egui::Painter, text: &str, size: f32, color: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}
