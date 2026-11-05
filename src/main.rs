#![windows_subsystem = "windows"]
//! Recto: a lightweight, read-only PDF viewer. `recto.exe <file.pdf> [--page N]` opens
//! a file (at page N), and `--register` / `--unregister` add or remove it as a Windows
//! PDF viewer.

mod doc;
mod fonts;
mod print;
#[cfg(feature = "script")]
mod script;
mod search;
mod settings;
mod timing;
mod ui;
mod view;
mod widgets;
mod win;

use doc::{Job, Output, PageText, Pool, TocEntry};
use eframe::egui::{self, vec2, Color32, ColorImage, FontId, Key, Modifiers, Rect, Stroke, TextureHandle, ViewportCommand};
use settings::Settings;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use view::{Fit, View};

pub const ACCENT: Color32 = Color32::from_rgb(255, 150, 64);
/// Memory for rendered pages and thumbnails; the least recently shown go first.
const PAGE_BUDGET: usize = 320 << 20;
const THUMB_BUDGET: usize = 48 << 20;
/// Pages above this many pixels render smaller (and show slightly soft) to save memory.
const MAX_PIXELS: f32 = 24e6;
/// New page images handed to the GPU per frame, so a burst of them can't stutter.
const UPLOADS_PER_FRAME: usize = 3;

fn main() -> eframe::Result {
    timing::Timing::mark("main_ms");
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    match args.first().map(String::as_str) {
        Some("--register") => {
            match win::register() {
                Ok(()) => {
                    win::open_default_apps();
                    win::message("Recto is now registered as a PDF viewer.\n\nIn Default apps, choose Recto for .pdf files.");
                }
                Err(e) => win::message(&format!("Registration failed: {e}")),
            }
            return Ok(());
        }
        Some("--unregister") => {
            win::unregister();
            win::message("Recto's file associations were removed.");
            return Ok(());
        }
        _ => {}
    }
    let (file, page) = parse_args(&args);

    let settings = Settings::load();
    // Start reading the file now, in parallel with creating the window.
    let pool = file.as_ref().map(|f| Pool::open(f, workers(), first_render(&settings, page)));
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(file.as_ref().and_then(|f| f.file_name()).map_or("Recto".into(), |n| format!("{} - Recto", n.to_string_lossy())))
        .with_inner_size(settings.size)
        .with_min_inner_size([360.0, 260.0])
        .with_maximized(settings.maximized)
        .with_drag_and_drop(true)
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/recto.png")).expect("valid icon"));
    if let Some(pos) = settings.pos {
        viewport = viewport.with_position(pos);
    }
    // No vsync: NVIDIA's OpenGL driver busy-waits for it, burning a core.
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, vsync: false, ..Default::default() };
    eframe::run_native("Recto", options, Box::new(move |cc| Ok(Box::new(App::new(cc, settings, file.zip(pool), page)))))
}

fn workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(2, 3)
}

/// The first page as it will be shown (fit to width), guessed from the saved window
/// size, so it can render before the window exists. A wrong guess costs one render.
fn first_render(s: &Settings, page: Option<usize>) -> Vec<Job> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};
    let ppp = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
    let width = if s.maximized {
        let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut r as *mut windows_sys::Win32::Foundation::RECT).cast(), 0) };
        (r.right - r.left) as f32 / ppp.max(0.5)
    } else {
        s.size[0]
    };
    let side = if s.sidebar { s.sidebar_width } else { 0.0 };
    let w = ((width - side - 2.0 * view::GAP) * ppp).round();
    if w < 50.0 {
        return Vec::new();
    }
    vec![Job::Render { page: page.unwrap_or(0), w: w as u32, invert: s.dark }]
}

/// `file.pdf`, `--page N` / `--page=N` / `-p N` (1-based), in any order.
fn parse_args(args: &[String]) -> (Option<PathBuf>, Option<usize>) {
    let (mut file, mut page) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--page" || a == "-p" {
            page = it.next().and_then(|n| n.trim().parse::<usize>().ok());
        } else if let Some(n) = a.strip_prefix("--page=") {
            page = n.trim().parse::<usize>().ok();
        } else if file.is_none() && !a.starts_with("--") {
            let p = PathBuf::from(a);
            file = Some(std::path::absolute(&p).unwrap_or(p));
        }
    }
    (file, page.map(|p| p.max(1) - 1))
}

pub struct Tex {
    pub tex: TextureHandle,
    pub w: u32,
    pub invert: bool,
    used: u64,
    bytes: usize,
}

#[derive(PartialEq)]
pub enum Load {
    Opening,
    Password { wrong: bool },
    Ready,
    Failed(String),
}

/// A text position: a page and a caret index into its characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Caret {
    pub page: usize,
    pub at: usize,
}

#[derive(Clone, Copy)]
pub struct Sel {
    pub anchor: Caret,
    pub focus: Caret,
}

impl Sel {
    pub fn ordered(&self) -> (Caret, Caret) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }

    /// Character index range selected on `page` (with `n` characters).
    pub fn on_page(&self, page: usize, n: usize) -> std::ops::Range<usize> {
        let (a, b) = self.ordered();
        if page < a.page || page > b.page {
            return 0..0;
        }
        let from = if page == a.page { a.at } else { 0 };
        let to = if page == b.page { b.at } else { n };
        from.min(n)..to.min(n).max(from.min(n))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Drag {
    Select,
    Pan,
}

/// The open document and everything shown about it.
pub struct Doc {
    pub path: PathBuf,
    pub name: String,
    pub pool: Pool,
    pub load: Load,
    pub password: String,
    pub view: Option<View>,
    pub sizes_known: usize,
    pub toc: Vec<TocEntry>,
    pub toc_open: HashSet<usize>,
    pub tex: HashMap<usize, Tex>,
    pub thumbs: HashMap<usize, Tex>,
    pub texts: HashMap<usize, Arc<PageText>>,
    /// Pages MuPDF couldn't render.
    pub broken: HashSet<usize>,
    uploads: VecDeque<(Job, ColorImage)>,
    pub sel: Option<Sel>,
    pub drag: Option<Drag>,
    pub search: search::Searcher,
    pub start_page: Option<usize>,
    pub back: Vec<(usize, f32)>,
    pub forward: Vec<(usize, f32)>,
    /// When zoom or size last changed: re-rendering waits until it settles.
    pub zoomed_at: Instant,
    pub fit_key: (Rect, Fit, usize, usize, bool),
    /// Thumbnails the sidebar shows this frame.
    pub side_thumbs: Vec<usize>,
    pub page_shown: bool,
}

pub struct App {
    pub ctx: egui::Context,
    pub settings: Settings,
    saved: Settings,
    last_save: Instant,
    pub fonts: fonts::Fonts,
    pub doc: Option<Doc>,
    pub frame: u64,
    /// The page area of the last frame.
    pub area: Rect,
    pub toast: Option<(String, Instant)>,
    pub show_keys: bool,
    pub search_open: bool,
    pub search_text: String,
    pub search_sent: String,
    pub search_edit: Instant,
    pub focus_search: bool,
    pub hit: Option<usize>,
    /// Waiting to jump to the first hit at or after this page.
    pub hit_from: Option<usize>,
    pub page_edit: Option<String>,
    pub count: String,
    pub glide: Option<egui::Vec2>,
    pub flip: f32,
    pub fullscreen: bool,
    pub printing: Option<print::Status>,
    /// What the sidebar last scrolled to: (tab, row or page).
    pub follow: Option<(u8, usize)>,
    title: String,
    timing: timing::Timing,
    #[cfg(feature = "script")]
    script: Option<script::Script>,
    #[cfg(feature = "script")]
    print_tested: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext, settings: Settings, file: Option<(PathBuf, Pool)>, page: Option<usize>) -> Self {
        timing::Timing::mark("app_new_ms");
        let ctx = cc.egui_ctx.clone();
        style(&ctx);
        // Ctrl +/- zoom the page, not the whole interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let fonts = fonts::Fonts::install(&ctx);
        let mut app = App {
            ctx: ctx.clone(),
            saved: settings.clone(),
            settings,
            last_save: Instant::now(),
            fonts,
            doc: None,
            frame: 0,
            area: Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 700.0)),
            toast: None,
            show_keys: false,
            search_open: false,
            search_text: String::new(),
            search_sent: String::new(),
            search_edit: Instant::now(),
            focus_search: false,
            hit: None,
            hit_from: None,
            page_edit: None,
            count: String::new(),
            glide: None,
            flip: 0.0,
            fullscreen: false,
            printing: None,
            follow: None,
            title: String::new(),
            timing: timing::Timing::from_env(),
            #[cfg(feature = "script")]
            script: script::Script::from_env(),
            #[cfg(feature = "script")]
            print_tested: false,
        };
        if let Some((f, pool)) = file {
            pool.attach(&ctx);
            app.open_with(f, page, pool);
        }
        timing::Timing::mark("app_ready_ms");
        app
    }

    pub fn open(&mut self, path: PathBuf, page: Option<usize>) {
        let pool = Pool::open(&path, workers(), Vec::new());
        pool.attach(&self.ctx);
        self.open_with(path, page, pool);
    }

    fn open_with(&mut self, path: PathBuf, page: Option<usize>, pool: Pool) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.doc = Some(Doc {
            pool,
            path,
            name,
            load: Load::Opening,
            password: String::new(),
            view: None,
            sizes_known: 0,
            toc: Vec::new(),
            toc_open: HashSet::new(),
            tex: HashMap::new(),
            thumbs: HashMap::new(),
            texts: HashMap::new(),
            broken: HashSet::new(),
            uploads: VecDeque::new(),
            sel: None,
            drag: None,
            search: search::Searcher::new(),
            start_page: page,
            back: Vec::new(),
            forward: Vec::new(),
            zoomed_at: Instant::now(),
            fit_key: (Rect::NOTHING, Fit::None, 0, 0, false),
            side_thumbs: Vec::new(),
            page_shown: false,
        });
        self.search_open = false;
        self.search_text.clear();
        self.search_sent.clear();
        self.hit = None;
        self.hit_from = None;
        self.page_edit = None;
        self.glide = None;
    }

    pub fn open_dialog(&mut self) {
        let dir = self.doc.as_ref().and_then(|d| d.path.parent().map(PathBuf::from)).or_else(|| self.settings.last_dir.clone());
        if let Some(p) = win::open_dialog(dir.as_deref()) {
            self.settings.last_dir = p.parent().map(PathBuf::from);
            self.open(p, None);
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    // ---- background results ----

    fn pump(&mut self, ctx: &egui::Context) {
        let area = self.area;
        let continuous = self.settings.continuous;
        let Some(d) = self.doc.as_mut() else { return };
        let mut toc_text = Vec::new();
        for o in d.pool.take() {
            match o {
                Output::NeedPassword { wrong } => {
                    d.load = Load::Password { wrong };
                    d.password.clear();
                }
                Output::Failed(e) => {
                    // "MuPDF error, code: 7, message: no objects found" -> "no objects found"
                    let e = e.rsplit_once("message: ").map_or(e.as_str(), |(_, m)| m).trim().to_owned();
                    d.load = Load::Failed(e);
                }
                Output::Opened { pages, first } => {
                    d.load = Load::Ready;
                    let mut v = View::new(pages, first, continuous);
                    v.viewport = area;
                    v.apply_fit();
                    d.fit_key = (area, v.fit, 0, 0, continuous);
                    if let Some(p) = d.start_page.take() {
                        v.go_to(p, None);
                    }
                    d.view = Some(v);
                }
                Output::Toc(toc) => {
                    toc_text = toc.iter().map(|t| t.title.clone()).collect();
                    // Open the top level of short tables of contents.
                    if toc.len() <= 60 {
                        d.toc_open = toc.iter().enumerate().filter(|(_, t)| t.depth == 0).map(|(i, _)| i).collect();
                    }
                    d.toc = toc;
                }
                Output::Sizes { from, sizes } => {
                    d.sizes_known = from + sizes.len();
                    if let Some(v) = d.view.as_mut() {
                        v.set_sizes(from, &sizes);
                    }
                }
                Output::Image { job, image } => d.uploads.push_back((job, image)),
                Output::Broken { page } => {
                    d.broken.insert(page);
                }
                Output::Text { page, text } => {
                    d.texts.insert(page, Arc::new(text));
                }
            }
        }
        for t in toc_text {
            self.fonts.cover(ctx, &t);
        }
        let frame = self.frame;
        let Some(d) = self.doc.as_mut() else { return };
        for _ in 0..UPLOADS_PER_FRAME {
            let Some((job, image)) = d.uploads.pop_front() else { break };
            let bytes = image.pixels.len() * 4;
            let (w, name) = (image.size[0] as u32, format!("{job:?}"));
            let tex = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
            match job {
                Job::Render { page, invert, .. } => {
                    d.tex.insert(page, Tex { tex, w, invert, used: frame, bytes });
                }
                Job::Thumb { page, invert } => {
                    d.thumbs.insert(page, Tex { tex, w, invert, used: frame, bytes });
                }
                _ => {}
            }
        }
        if !d.uploads.is_empty() {
            pace(ctx);
        }
        // Drop the least recently shown images beyond the budgets.
        for (map, budget) in [(&mut d.tex, PAGE_BUDGET), (&mut d.thumbs, THUMB_BUDGET)] {
            let mut total: usize = map.values().map(|t| t.bytes).sum();
            if total > budget {
                let mut by_age: Vec<(u64, usize, usize)> = map.iter().map(|(&p, t)| (t.used, p, t.bytes)).collect();
                by_age.sort_unstable();
                for (used, p, b) in by_age {
                    if total <= budget || used + 1 >= frame {
                        break;
                    }
                    map.remove(&p);
                    total -= b;
                }
            }
        }
        // Text of far-away pages (keeps a selection's pages).
        if d.texts.len() > 400 {
            let cur = d.view.as_mut().map_or(0, |v| v.current());
            let keep = d.sel.map(|s| s.ordered());
            d.texts.retain(|&p, _| p.abs_diff(cur) < 150 || keep.is_some_and(|(a, b)| p >= a.page && p <= b.page));
        }
    }

    /// The pixel width page `i` is rendered at for the current zoom.
    pub fn render_width(v: &View, i: usize, ppp: f32, max_side: usize) -> u32 {
        let sz = v.sizes[i] * v.px_per_pt() * ppp;
        let mut w = sz.x.max(1.0);
        let area = sz.x * sz.y;
        if area > MAX_PIXELS {
            w *= (MAX_PIXELS / area).sqrt();
        }
        let h = w * sz.y / sz.x.max(1.0);
        if h > max_side as f32 {
            w *= max_side as f32 / h;
        }
        w.min(max_side as f32).round().max(1.0) as u32
    }

    /// Tells the workers what to do next, most urgent first.
    fn request(&mut self, ctx: &egui::Context) {
        let dark = self.settings.dark;
        let ppp = ctx.pixels_per_point();
        let max_side = ctx.input(|i| i.max_texture_side).min(16384);
        let frame = self.frame;
        let Some(d) = self.doc.as_mut() else { return };
        let Some(v) = d.view.as_mut() else { return };
        let settled = d.zoomed_at.elapsed() > Duration::from_millis(160);
        let vis: Vec<usize> = v.visible(0.0).filter(|i| !d.broken.contains(i)).collect();
        let cur = v.current();
        let mut jobs = Vec::new();
        for &i in &vis {
            if let Some(t) = d.tex.get_mut(&i) {
                t.used = frame;
            }
            if let Some(t) = d.thumbs.get_mut(&i) {
                t.used = frame;
            }
        }
        // A quick low-resolution picture for pages with nothing to show yet.
        for &i in &vis {
            if !d.tex.contains_key(&i) && !d.thumbs.get(&i).is_some_and(|t| t.invert == dark) {
                jobs.push(Job::Thumb { page: i, invert: dark });
            }
        }
        let mut order = vis.clone();
        order.sort_by_key(|&i| i.abs_diff(cur));
        for &i in &order {
            let w = Self::render_width(v, i, ppp, max_side);
            match d.tex.get(&i) {
                Some(t) if t.w == w && t.invert == dark => {}
                Some(t) if !settled && t.invert == dark => {}
                _ => jobs.push(Job::Render { page: i, w, invert: dark }),
            }
        }
        if d.sizes_known < v.pages() {
            jobs.push(Job::Sizes { from: d.sizes_known });
        }
        for &i in &vis {
            if !d.texts.contains_key(&i) {
                jobs.push(Job::Text { page: i });
            }
        }
        // Text of every page in a selection, so it can be copied.
        if let Some(s) = d.sel {
            let (a, b) = s.ordered();
            for p in a.page..=b.page.min(a.page + 300) {
                if !d.texts.contains_key(&p) {
                    jobs.push(Job::Text { page: p });
                }
            }
        }
        // Look ahead: the next two pages and the previous one.
        if settled {
            let last = vis.last().copied().unwrap_or(0);
            let first = vis.first().copied().unwrap_or(0);
            for i in [last + 1, last + 2, first.wrapping_sub(1)] {
                if i < v.pages() && !d.broken.contains(&i) {
                    let w = Self::render_width(v, i, ppp, max_side);
                    if !d.tex.get(&i).is_some_and(|t| t.w == w && t.invert == dark) {
                        jobs.push(Job::Render { page: i, w, invert: dark });
                    }
                    if let Some(t) = d.tex.get_mut(&i) {
                        t.used = frame;
                    }
                }
            }
        }
        for &i in d.side_thumbs.iter().filter(|i| !d.broken.contains(i)) {
            if let Some(t) = d.thumbs.get_mut(&i) {
                t.used = frame;
            }
            if !d.thumbs.get(&i).is_some_and(|t| t.invert == dark) {
                jobs.push(Job::Thumb { page: i, invert: dark });
            }
        }
        jobs.dedup();
        d.pool.want(jobs);
        if !settled {
            ctx.request_repaint_after(Duration::from_millis(170));
        }
    }

    // ---- navigation ----

    /// Where the view is, for "back" after a jump.
    fn here(&mut self) -> Option<(usize, f32)> {
        let v = self.doc.as_mut()?.view.as_mut()?;
        let a = v.anchor();
        Some((a.page, a.at.y))
    }

    /// Jumps somewhere, remembering where we were for Alt+Left.
    pub fn jump(&mut self, page: usize, y: Option<f32>) {
        if let Some(h) = self.here() {
            let d = self.doc.as_mut().unwrap();
            d.back.push(h);
            d.forward.clear();
        }
        self.glide = None;
        if let Some(v) = self.doc.as_mut().and_then(|d| d.view.as_mut()) {
            v.go_to(page, y);
        }
    }

    fn history(&mut self, back: bool) {
        let Some(h) = self.here() else { return };
        let d = self.doc.as_mut().unwrap();
        let (from, to) = if back { (&mut d.back, &mut d.forward) } else { (&mut d.forward, &mut d.back) };
        if let Some((p, y)) = from.pop() {
            to.push(h);
            if let Some(v) = d.view.as_mut() {
                v.go_to(p, Some(y + view::GAP / v.px_per_pt()));
            }
        }
    }

    pub fn follow(&mut self, target: &doc::Target) {
        match target {
            doc::Target::Page { page, y } => self.jump(*page, *y),
            doc::Target::Uri(u) => {
                if win::open_url(u) {
                    self.toast(format!("Opening {u}"));
                } else {
                    self.ctx.copy_text(u.clone());
                    self.toast("Link copied (Recto only opens web and mail links)");
                }
            }
        }
    }

    /// Scrolls smoothly by `delta` pixels.
    pub fn scroll_by(&mut self, delta: egui::Vec2) {
        let Some(v) = self.doc.as_mut().and_then(|d| d.view.as_mut()) else { return };
        let from = self.glide.unwrap_or(v.scroll);
        let c = v.content();
        let max = (c - v.viewport.size()).max(egui::Vec2::ZERO);
        self.glide = Some((from + delta).clamp(egui::Vec2::ZERO, max));
    }

    /// Next/previous page (`dir` = 1/-1), or a screen's worth in continuous mode.
    pub fn page_step(&mut self, dir: i32) {
        let Some(v) = self.doc.as_mut().and_then(|d| d.view.as_mut()) else { return };
        let cur = v.current();
        let to = (cur as i64 + dir as i64).clamp(0, v.pages() as i64 - 1) as usize;
        self.glide = None;
        v.go_to(to, None);
    }

    fn screen_step(&mut self, dir: f32) {
        let Some(v) = self.doc.as_mut().and_then(|d| d.view.as_mut()) else { return };
        if !v.continuous && v.at_edge(dir > 0.0) {
            return self.page_step(dir as i32);
        }
        let h = (v.viewport.height() - 48.0).max(40.0);
        self.scroll_by(vec2(0.0, h * dir));
    }

    pub fn zoom_step(&mut self, dir: i32, around: Option<egui::Pos2>) {
        if let Some(d) = self.doc.as_mut() {
            if let Some(v) = d.view.as_mut() {
                v.step_zoom(dir, around);
                d.zoomed_at = Instant::now();
                self.glide = None;
            }
        }
    }

    pub fn set_fit(&mut self, fit: Fit) {
        if let Some(d) = self.doc.as_mut() {
            if let Some(v) = d.view.as_mut() {
                v.fit = fit;
                d.fit_key.1 = Fit::None; // recompute this frame
                d.zoomed_at = Instant::now();
                if fit == Fit::None {
                    v.set_zoom(1.0, None);
                }
            }
        }
    }

    pub fn set_continuous(&mut self, on: bool) {
        self.settings.continuous = on;
        if let Some(v) = self.doc.as_mut().and_then(|d| d.view.as_mut()) {
            v.set_continuous(on);
        }
        self.glide = None;
    }

    pub fn current_page(&mut self) -> Option<usize> {
        Some(self.doc.as_mut()?.view.as_mut()?.current())
    }

    // ---- search ----

    pub fn run_search(&mut self) {
        let q = self.search_text.trim().to_owned();
        self.search_sent = q.clone();
        self.hit = None;
        let Some(start) = self.current_page() else { return };
        let ctx = self.ctx.clone();
        let d = self.doc.as_mut().unwrap();
        let (path, pw) = d.pool.credentials();
        d.search.find(&path, pw, &q, start, &ctx);
        self.hit_from = if q.is_empty() { None } else { Some(start) };
    }

    /// Moves to the next (1) or previous (-1) match.
    pub fn next_hit(&mut self, dir: i32) {
        if self.search_text.trim() != self.search_sent {
            self.run_search();
            return;
        }
        let Some(d) = self.doc.as_ref() else { return };
        let n = d.search.progress.lock().unwrap().hits.len();
        if n == 0 {
            return;
        }
        let i = match self.hit {
            Some(h) => (h as i64 + dir as i64).rem_euclid(n as i64) as usize,
            None => 0,
        };
        self.hit_from = None;
        self.show_hit(i);
    }

    pub fn show_hit(&mut self, i: usize) {
        self.hit = Some(i);
        let Some(d) = self.doc.as_mut() else { return };
        let Some(h) = d.search.progress.lock().unwrap().hits.get(i).copied() else { return };
        let Some(v) = d.view.as_mut() else { return };
        if !v.continuous && v.single != h.page {
            v.single = h.page;
            v.invalidate();
        }
        let r = v.rect_page_to_screen(h.page, h.rect);
        let vp = v.viewport.shrink(24.0);
        if !vp.contains_rect(r) {
            let mut s = v.scroll;
            if r.top() < vp.top() || r.bottom() > vp.bottom() {
                s.y += r.top() - (v.viewport.top() + v.viewport.height() * 0.3);
            }
            if r.left() < vp.left() || r.right() > vp.right() {
                s.x += r.center().x - v.viewport.center().x;
            }
            v.scroll = s;
            v.clamp();
        }
        self.glide = None;
    }

    /// Jumps to the first match at or after the page the search started on, once found.
    fn auto_hit(&mut self) {
        let Some(from) = self.hit_from else { return };
        let Some(d) = self.doc.as_ref() else { return };
        let found = {
            let p = d.search.progress.lock().unwrap();
            if p.query != self.search_sent {
                return;
            }
            let i = p.hits.partition_point(|h| h.page < from);
            if i < p.hits.len() {
                Some(i)
            } else if p.done && !p.hits.is_empty() {
                Some(0)
            } else {
                None
            }
        };
        if let Some(i) = found {
            self.hit_from = None;
            self.show_hit(i);
        }
    }

    // ---- selection ----

    pub fn copy_selection(&mut self) {
        let Some(d) = self.doc.as_ref() else { return };
        let Some(s) = d.sel else { return };
        let (a, b) = s.ordered();
        let mut out = String::new();
        for p in a.page..=b.page {
            let Some(t) = d.texts.get(&p) else {
                self.toast("Still reading the text, try again in a moment");
                return;
            };
            let r = s.on_page(p, t.chars.len());
            for ch in &t.chars[r] {
                out.push(ch.c);
                if ch.eol {
                    out.push('\n');
                }
            }
            if p < b.page && !out.ends_with('\n') {
                out.push('\n');
            }
        }
        let out = out.trim_end().to_owned();
        if out.is_empty() {
            return;
        }
        let n = out.chars().count();
        self.ctx.copy_text(out);
        self.toast(format!("Copied {n} character{}", if n == 1 { "" } else { "s" }));
    }

    pub fn select_page(&mut self) {
        let Some(p) = self.current_page() else { return };
        let d = self.doc.as_mut().unwrap();
        let n = d.texts.get(&p).map_or(0, |t| t.chars.len());
        d.sel = Some(Sel { anchor: Caret { page: p, at: 0 }, focus: Caret { page: p, at: n } });
    }

    // ---- keyboard ----

    fn keyboard(&mut self, ctx: &egui::Context) {
        let ctrl = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        let ctrl_shift = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, k));
        let alt = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::ALT, k));
        let shift = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, k));
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));

        // These work even while typing in the search or page box.
        if ctrl(Key::O) {
            self.open_dialog();
        }
        if ctrl(Key::F) {
            self.search_open = true;
            self.focus_search = true;
        }
        if pressed(Key::F11) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(self.fullscreen));
        }
        if pressed(Key::F1) {
            self.show_keys = !self.show_keys;
        }
        if shift(Key::F3) {
            self.next_hit(-1);
        }
        if pressed(Key::F3) {
            self.next_hit(1);
        }
        if ctrl(Key::P) {
            self.print();
        }
        if ctrl(Key::G) {
            self.page_edit = Some(String::new());
        }
        // In the search field, Enter / Shift+Enter step through matches and Esc closes it.
        if self.search_open && ctx.memory(|m| m.has_focus(egui::Id::new("search-field"))) {
            if pressed(Key::Escape) {
                self.close_search();
                ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("search-field")));
            }
            if shift(Key::Enter) {
                self.next_hit(-1);
            }
            if pressed(Key::Enter) {
                self.next_hit(1);
            }
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctrl(Key::C) {
            self.copy_selection();
        }
        if ctrl(Key::A) {
            self.select_page();
        }
        if ctrl(Key::Plus) || ctrl(Key::Equals) || ctrl_shift(Key::Plus) {
            self.zoom_step(1, None);
        }
        if ctrl(Key::Minus) {
            self.zoom_step(-1, None);
        }
        if ctrl(Key::Num0) {
            self.set_fit(Fit::None);
        }
        if alt(Key::ArrowLeft) {
            self.history(true);
        }
        if alt(Key::ArrowRight) {
            self.history(false);
        }
        if ctx.memory(|m| m.any_popup_open()) && pressed(Key::Escape) {
            ctx.memory_mut(|m| m.close_popup());
        }
        if pressed(Key::Escape) {
            if !self.count.is_empty() {
                self.count.clear();
            } else if self.search_open {
                self.close_search();
            } else if self.doc.as_ref().is_some_and(|d| d.sel.is_some()) {
                self.doc.as_mut().unwrap().sel = None;
            } else if self.fullscreen {
                self.fullscreen = false;
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
            }
        }
        let step = 64.0;
        let wide = self.doc.as_mut().and_then(|d| d.view.as_mut()).is_some_and(|v| v.content().x > v.viewport.width() + 1.0);
        if shift(Key::Space) {
            self.screen_step(-1.0);
        }
        if pressed(Key::Space) || pressed(Key::PageDown) {
            self.screen_step(1.0);
        }
        if pressed(Key::PageUp) {
            self.screen_step(-1.0);
        }
        if pressed(Key::ArrowDown) {
            self.scroll_by(vec2(0.0, step));
        }
        if pressed(Key::ArrowUp) {
            self.scroll_by(vec2(0.0, -step));
        }
        for (k, dir) in [(Key::ArrowLeft, -1.0), (Key::ArrowRight, 1.0)] {
            if pressed(k) {
                if wide { self.scroll_by(vec2(step * dir, 0.0)) } else { self.page_step(dir as i32) }
            }
        }
        if pressed(Key::Home) {
            self.jump(0, None);
        }
        if pressed(Key::End) {
            let n = self.doc.as_ref().and_then(|d| d.view.as_ref()).map_or(1, |v| v.pages());
            self.jump(n - 1, None);
        }
        if pressed(Key::Backspace) {
            self.history(true);
        }
        if pressed(Key::Enter) && !self.count.is_empty() {
            self.go_count();
        }

        // Letter commands, read as typed text so Shift (J, G, N) and keyboard layouts work.
        let texts: Vec<String> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        });
        for t in texts {
            for c in t.chars() {
                self.command(c, wide);
            }
        }
    }

    fn command(&mut self, c: char, wide: bool) {
        if c.is_ascii_digit() && !(c == '0' && self.count.is_empty()) {
            if self.count.len() < 6 {
                self.count.push(c);
            }
            return;
        }
        let had_count = !self.count.is_empty();
        match c {
            'j' => self.scroll_by(vec2(0.0, 64.0)),
            'k' => self.scroll_by(vec2(0.0, -64.0)),
            'h' if wide => self.scroll_by(vec2(-64.0, 0.0)),
            'l' if wide => self.scroll_by(vec2(64.0, 0.0)),
            'h' | 'K' => self.page_step(-1),
            'l' | 'J' => self.page_step(1),
            'g' => self.jump(0, None),
            'G' if had_count => self.go_count(),
            'G' => {
                let n = self.doc.as_ref().and_then(|d| d.view.as_ref()).map_or(1, |v| v.pages());
                self.jump(n - 1, None);
            }
            '/' => {
                self.search_open = true;
                self.focus_search = true;
            }
            'n' => self.next_hit(1),
            'N' => self.next_hit(-1),
            '+' | '=' => self.zoom_step(1, None),
            '-' => self.zoom_step(-1, None),
            '0' => self.set_fit(Fit::None),
            'w' => self.set_fit(Fit::Width),
            'f' => self.set_fit(Fit::Page),
            'c' => {
                let on = !self.settings.continuous;
                self.set_continuous(on);
                self.toast(if on { "Continuous scrolling" } else { "Single page" });
            }
            'i' => self.settings.dark = !self.settings.dark,
            't' => self.settings.sidebar = !self.settings.sidebar,
            _ => {}
        }
        if c != 'G' {
            self.count.clear();
        }
    }

    fn go_count(&mut self) {
        if let Ok(n) = self.count.parse::<usize>() {
            self.jump(n.max(1) - 1, None);
        }
        self.count.clear();
    }

    pub fn close_search(&mut self) {
        self.search_open = false;
        self.search_text.clear();
        self.search_sent.clear();
        self.hit = None;
        self.hit_from = None;
        if let Some(d) = self.doc.as_mut() {
            *d.search.progress.lock().unwrap() = search::Progress::default();
        }
    }

    pub fn print(&mut self) {
        let Some(d) = self.doc.as_mut() else { return };
        if d.load != Load::Ready {
            return;
        }
        let Some(v) = d.view.as_mut() else { return };
        let (pages, cur) = (v.pages(), v.current());
        let (path, pw) = d.pool.credentials();
        let name = d.name.clone();
        if let Some(s) = print::print(path, pw, pages, cur, name, &self.ctx) {
            self.printing = Some(s);
        }
    }

    fn remember_window(&mut self, ctx: &egui::Context) {
        if self.fullscreen {
            return;
        }
        ctx.input(|i| {
            let vp = i.viewport();
            let max = vp.maximized.unwrap_or(false);
            self.settings.maximized = max;
            if !max && !vp.minimized.unwrap_or(false) {
                if let Some(r) = vp.inner_rect {
                    self.settings.size = [r.width(), r.height()];
                }
                if let Some(r) = vp.outer_rect {
                    self.settings.pos = Some([r.min.x, r.min.y]);
                }
            }
        });
    }
}

impl eframe::App for App {
    #[cfg(feature = "script")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(s) = &mut self.script {
            s.hook(ctx, raw);
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame += 1;
        self.timing.frame(ctx);
        #[cfg(feature = "script")]
        if let Some(s) = &self.script {
            s.keep_alive(ctx);
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open(p, None);
        }
        self.pump(ctx);
        self.keyboard(ctx);
        if self.search_open && self.search_text.trim() != self.search_sent && self.search_edit.elapsed() > Duration::from_millis(250) {
            self.run_search();
        }
        self.auto_hit();

        if !self.fullscreen {
            self.top_bar(ctx);
        }
        let ready = self.doc.as_ref().is_some_and(|d| d.load == Load::Ready);
        if self.settings.sidebar && ready && !self.fullscreen {
            self.sidebar(ctx);
        } else {
            self.follow = None;
            if let Some(d) = self.doc.as_mut() {
                d.side_thumbs.clear();
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(widgets::DESK)).show(ctx, |ui| self.pages(ui));
        self.request(ctx);
        self.overlays(ctx);
        #[cfg(feature = "script")]
        if let Some(spec) = std::env::var("RECTO_PRINT_TEST").ok().filter(|_| ready && !std::mem::replace(&mut self.print_tested, true)) {
            // "printer|out.pdf|1,2,5" (1-based pages)
            let parts: Vec<&str> = spec.split('|').collect();
            let pages: Vec<usize> = parts.get(2).map_or(vec![1], |p| p.split(',').filter_map(|n| n.parse().ok()).collect()).iter().map(|n: &usize| n - 1).collect();
            let (path, pw) = self.doc.as_ref().unwrap().pool.credentials();
            let r = print::print_to_file(&path, pw.as_deref(), &pages, parts[0], parts.get(1).copied().unwrap_or("out.pdf"), ctx);
            self.toast(format!("print test: {r:?}"));
        }

        let title = self.doc.as_ref().map_or("Recto".into(), |d| format!("{} - Recto", d.name));
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if self.doc.as_ref().is_some_and(|d| d.page_shown) {
            self.timing.page_shown(ctx);
        }
        self.remember_window(ctx);
        if self.settings != self.saved && self.last_save.elapsed() > Duration::from_secs(5) {
            self.settings.save();
            self.saved = self.settings.clone();
            self.last_save = Instant::now();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.settings != self.saved {
            self.settings.save();
        }
    }
}

/// Asks for the next animation frame. Without vsync (NVIDIA's OpenGL driver busy-waits
/// for it, burning a core) an immediate repaint would spin as fast as the CPU allows;
/// this paces animations at about 144 frames a second instead. egui shortens every
/// delay by its predicted frame time, which is added back.
pub fn pace(ctx: &egui::Context) {
    let dt = ctx.input(|i| i.predicted_dt);
    ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 144.0 + dt.clamp(0.0, 0.1)));
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(27, 28, 34);
    v.window_fill = Color32::from_rgb(34, 35, 43);
    v.extreme_bg_color = Color32::from_rgb(18, 19, 23);
    v.faint_bg_color = Color32::from_rgb(31, 32, 39);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.bg_fill = Color32::from_rgb(44, 45, 55);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 45, 55);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(56, 57, 70);
    v.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.7);
    v.window_stroke = Stroke::new(1.0_f32, Color32::from_gray(60));
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.button_padding = vec2(8.0, 4.0);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::proportional(19.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::proportional(13.0));
    });
}
