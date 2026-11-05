//! Page layout: where each page sits for the current zoom and mode, scrolling,
//! zooming around a point, and mapping between the screen and page coordinates.
//!
//! Content coordinates are logical pixels from the top-left of the whole document
//! (continuous mode) or of the shown page (single-page mode). Page coordinates are
//! PDF points from the page's top-left.

use eframe::egui::{pos2, vec2, Pos2, Rect, Vec2};

/// Space above, below and between pages.
pub const GAP: f32 = 12.0;
/// Logical pixels per PDF point at 100 % (a point is 1/72 inch, a logical pixel 1/96).
pub const PX_PER_PT: f32 = 96.0 / 72.0;
pub const MIN_ZOOM: f32 = 0.08;
pub const MAX_ZOOM: f32 = 8.0;
pub const ZOOM_STEPS: &[f32] = &[0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.4, 8.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fit {
    Width,
    Page,
    None,
}

/// A spot in the document that should stay put when the layout changes.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub page: usize,
    /// Page coordinates of the spot.
    pub at: Pos2,
    /// Where it was, relative to the viewport's top-left.
    pub screen: Vec2,
}

pub struct View {
    /// Page sizes in points. Until the real sizes arrive, unknown pages copy page 1.
    pub sizes: Vec<Vec2>,
    pub zoom: f32,
    pub fit: Fit,
    pub continuous: bool,
    /// The viewport's top-left in content coordinates.
    pub scroll: Vec2,
    /// The page shown in single-page mode.
    pub single: usize,
    /// The screen area pages are drawn in.
    pub viewport: Rect,
    tops: Vec<f32>,
    max_w: f32,
    height: f32,
    dirty: bool,
}

impl View {
    pub fn new(pages: usize, first: Vec2, continuous: bool) -> View {
        let mut v = View {
            sizes: vec![first; pages.max(1)],
            zoom: 1.0,
            fit: Fit::Width,
            continuous,
            scroll: Vec2::ZERO,
            single: 0,
            viewport: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
            tops: Vec::new(),
            max_w: 0.0,
            height: 0.0,
            dirty: true,
        };
        v.relayout();
        v
    }

    pub fn pages(&self) -> usize {
        self.sizes.len()
    }

    fn scale(&self) -> f32 {
        self.zoom * PX_PER_PT
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn relayout(&mut self) {
        let s = self.scale();
        self.tops.clear();
        let mut y = GAP;
        self.max_w = 0.0;
        for sz in &self.sizes {
            self.tops.push(y);
            y += sz.y * s + GAP;
            self.max_w = self.max_w.max(sz.x * s);
        }
        self.height = y;
        self.dirty = false;
    }

    fn ensure(&mut self) {
        if self.dirty {
            self.relayout();
        }
    }

    /// The scrollable content size.
    pub fn content(&mut self) -> Vec2 {
        self.ensure();
        let s = self.scale();
        let (w, h) = if self.continuous {
            (self.max_w, self.height)
        } else {
            let p = self.sizes[self.single];
            (p.x * s, p.y * s + 2.0 * GAP)
        };
        vec2((w + 2.0 * GAP).max(self.viewport.width()), h.max(self.viewport.height()))
    }

    /// Page `i` in content coordinates. Pages narrower than the window are centred in
    /// it; wider ones start at the left margin and scroll sideways.
    pub fn page_rect(&mut self, i: usize) -> Rect {
        self.ensure();
        let s = self.scale();
        let sz = self.sizes[i] * s;
        let y = if self.continuous { self.tops[i] } else { GAP };
        Rect::from_min_size(pos2(((self.viewport.width() - sz.x) / 2.0).max(GAP), y), sz)
    }

    /// The width most pages have (a single fold-out page shouldn't shrink the rest).
    fn typical_width(&self) -> f32 {
        if !self.continuous {
            return self.sizes[self.single].x;
        }
        let mut w: Vec<f32> = self.sizes.iter().map(|s| s.x).collect();
        let mid = w.len() / 2;
        *w.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
    }

    /// Page `i` on screen, snapped to whole physical pixels so text stays crisp.
    pub fn screen_rect(&mut self, i: usize, ppp: f32) -> Rect {
        let r = self.page_rect(i).translate(self.viewport.min.to_vec2() - self.scroll);
        let snap = |v: f32| (v * ppp).round() / ppp;
        Rect::from_min_size(pos2(snap(r.min.x), snap(r.min.y)), r.size())
    }

    /// Pages overlapping the viewport, grown by `extra` pixels above and below.
    pub fn visible(&mut self, extra: f32) -> std::ops::Range<usize> {
        self.ensure();
        if !self.continuous {
            return self.single..self.single + 1;
        }
        let (top, bottom) = (self.scroll.y - extra, self.scroll.y + self.viewport.height() + extra);
        let s = self.scale();
        let first = self.tops.partition_point(|&t| t <= top).saturating_sub(1);
        let mut last = first;
        while last < self.tops.len() && self.tops[last] < bottom {
            last += 1;
        }
        let first = if self.tops[first] + self.sizes[first].y * s < top { first + 1 } else { first };
        first.min(last)..last.max(first + 1).min(self.pages())
    }

    /// The page most of the viewport's upper part shows.
    pub fn current(&mut self) -> usize {
        if !self.continuous {
            return self.single;
        }
        self.ensure();
        let y = self.scroll.y + self.viewport.height() * 0.3;
        self.tops.partition_point(|&t| t <= y).saturating_sub(1).min(self.pages() - 1)
    }

    pub fn clamp(&mut self) {
        let c = self.content();
        let max = (c - self.viewport.size()).max(Vec2::ZERO);
        self.scroll = self.scroll.clamp(Vec2::ZERO, max);
        if c.x <= self.viewport.width() {
            self.scroll.x = 0.0;
        }
    }

    /// The page and page coordinates under a screen point; the nearest page when
    /// the point is in a gap.
    pub fn hit(&mut self, p: Pos2) -> Option<(usize, Pos2)> {
        self.ensure();
        let c = p - self.viewport.min + self.scroll;
        let i = if self.continuous { self.tops.partition_point(|&t| t <= c.y + GAP / 2.0).saturating_sub(1).min(self.pages() - 1) } else { self.single };
        let r = self.page_rect(i);
        let s = self.scale();
        Some((i, ((c.to_pos2() - r.min) / s).to_pos2()))
    }

    pub fn page_to_screen(&mut self, page: usize, at: Pos2) -> Pos2 {
        let r = self.page_rect(page);
        r.min + at.to_vec2() * self.scale() + self.viewport.min.to_vec2() - self.scroll
    }

    pub fn anchor_at(&mut self, screen: Pos2) -> Anchor {
        let (page, at) = self.hit(screen).unwrap_or((0, Pos2::ZERO));
        Anchor { page, at, screen: screen - self.viewport.min }
    }

    /// The anchor that keeps the view still: the spot at the viewport's top-left.
    pub fn anchor(&mut self) -> Anchor {
        let p = self.viewport.min + vec2(self.viewport.width() / 2.0, 0.0);
        self.anchor_at(p)
    }

    pub fn restore(&mut self, a: Anchor) {
        self.ensure();
        if !self.continuous {
            self.single = a.page.min(self.pages() - 1);
        }
        let r = self.page_rect(a.page.min(self.pages() - 1));
        let c = r.min + a.at.to_vec2() * self.scale();
        self.scroll = c.to_vec2() - a.screen;
        self.clamp();
    }

    /// Recomputes the zoom for the fit mode (after a resize or page change).
    pub fn apply_fit(&mut self) {
        let page = self.current();
        let sz = self.sizes[page];
        let (vw, vh) = (self.viewport.width() - 2.0 * GAP, self.viewport.height() - 2.0 * GAP);
        let z = match self.fit {
            Fit::None => return,
            Fit::Width => vw / (self.typical_width() * PX_PER_PT),
            Fit::Page => (vw / (sz.x * PX_PER_PT)).min(vh / (sz.y * PX_PER_PT)),
        };
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        if (z - self.zoom).abs() > 1e-4 {
            let a = self.anchor();
            self.zoom = z;
            self.dirty = true;
            self.restore(a);
            if self.fit == Fit::Page {
                // Show the whole page.
                self.go_to(page, None);
            }
        }
    }

    /// Sets the zoom, keeping the spot under `around` (a screen point) in place.
    pub fn set_zoom(&mut self, z: f32, around: Option<Pos2>) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        let a = match around {
            Some(p) => self.anchor_at(p),
            None => self.anchor_at(self.viewport.center()),
        };
        self.zoom = z;
        self.fit = Fit::None;
        self.dirty = true;
        self.restore(a);
    }

    pub fn step_zoom(&mut self, dir: i32, around: Option<Pos2>) {
        let z = self.zoom;
        let next = if dir > 0 { ZOOM_STEPS.iter().copied().find(|&s| s > z * 1.01) } else { ZOOM_STEPS.iter().rev().copied().find(|&s| s < z / 1.01) };
        self.set_zoom(next.unwrap_or(z), around);
    }

    /// Scrolls so page `page` (at `y` points down it, else its top) is at the top.
    pub fn go_to(&mut self, page: usize, y: Option<f32>) {
        let page = page.min(self.pages() - 1);
        if !self.continuous {
            self.single = page;
        }
        let r = self.page_rect(page);
        let off = y.map_or(-GAP, |y| y * self.scale() - GAP * 2.0);
        self.scroll.y = r.min.y + off;
        if self.fit == Fit::Page && y.is_none() && self.continuous {
            // Centre the page vertically when it fits.
            let free = self.viewport.height() - r.height();
            if free > 0.0 {
                self.scroll.y = r.min.y - free / 2.0;
            }
        }
        self.clamp();
    }

    pub fn set_continuous(&mut self, on: bool) {
        if on == self.continuous {
            return;
        }
        // Anchor on the page being read (the top edge may be in the gap above it).
        let cur = self.current();
        let mut a = self.anchor();
        if a.page != cur {
            a = Anchor { page: cur, at: pos2(a.at.x, 0.0), screen: vec2(a.screen.x, GAP) };
        }
        self.continuous = on;
        self.single = a.page;
        self.dirty = true;
        self.restore(a);
    }

    /// Real page sizes from the document, `from` onwards.
    pub fn set_sizes(&mut self, from: usize, sizes: &[Vec2]) {
        let a = self.anchor();
        for (i, s) in sizes.iter().enumerate() {
            if let Some(slot) = self.sizes.get_mut(from + i) {
                *slot = *s;
            }
        }
        self.dirty = true;
        self.restore(a);
    }

    /// Page-coordinate rect on screen.
    pub fn rect_page_to_screen(&mut self, page: usize, r: Rect) -> Rect {
        Rect::from_min_max(self.page_to_screen(page, r.min), self.page_to_screen(page, r.max))
    }

    pub fn px_per_pt(&self) -> f32 {
        self.scale()
    }

    /// True when the view is at the very top / bottom (single-page flipping).
    pub fn at_edge(&mut self, down: bool) -> bool {
        let max = (self.content().y - self.viewport.height()).max(0.0);
        if down { self.scroll.y >= max - 0.5 } else { self.scroll.y <= 0.5 }
    }
}
