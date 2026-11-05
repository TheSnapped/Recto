//! The open document, served by background workers. Each worker thread opens its own
//! copy of the file with MuPDF (whose objects can't cross threads) and takes the most
//! urgent job the UI currently wants: page images, thumbnails, page text and links,
//! or page sizes. The UI never waits on MuPDF.

use eframe::egui::{self, pos2, vec2, Color32, ColorImage, Rect, Vec2};
use mupdf::text_page::TextBlockType;
use mupdf::{Colorspace, DestinationKind, Document, Matrix, TextPageFlags};
use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};

/// Thumbnail width in pixels.
pub const THUMB_W: u32 = 150;
/// Pages whose sizes one `Sizes` job reads.
const SIZE_CHUNK: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Job {
    /// A page at `w` pixels wide.
    Render { page: usize, w: u32, invert: bool },
    Thumb { page: usize, invert: bool },
    Text { page: usize },
    /// The sizes of the pages from `from` on.
    Sizes { from: usize },
}

/// Where a link or a contents entry leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A page, optionally a spot on it (points from the top).
    Page { page: usize, y: Option<f32> },
    Uri(String),
}

pub struct Ch {
    pub c: char,
    /// In page points.
    pub rect: Rect,
    /// Last character of a line: copying adds a line break after it.
    pub eol: bool,
}

pub struct LinkArea {
    pub rect: Rect,
    pub target: Target,
}

#[derive(Default)]
pub struct PageText {
    pub chars: Vec<Ch>,
    pub links: Vec<LinkArea>,
}

pub struct TocEntry {
    pub title: String,
    pub depth: usize,
    pub target: Option<Target>,
    /// Number of entries nested below this one (they follow it directly).
    pub descendants: usize,
}

pub enum Output {
    NeedPassword { wrong: bool },
    Opened { pages: usize, first: Vec2 },
    Toc(Vec<TocEntry>),
    Failed(String),
    Sizes { from: usize, sizes: Vec<Vec2> },
    Image { job: Job, image: ColorImage },
    /// The page couldn't be rendered (damaged file); it isn't asked for again.
    Broken { page: usize },
    Text { page: usize, text: PageText },
}

#[derive(PartialEq)]
enum State {
    Opening,
    Ready,
    Closed,
}

struct Queue {
    wanted: Vec<Job>,
    busy: HashSet<Job>,
    /// Finished but not yet collected by the UI, so they aren't started again.
    done: HashSet<Job>,
    out: Vec<Output>,
    password: Option<String>,
    state: State,
}

struct Shared {
    path: String,
    /// Set once the window exists (the pool starts before it, see `Pool::open`).
    ctx: std::sync::OnceLock<egui::Context>,
    q: Mutex<Queue>,
    cv: Condvar,
}

/// Opens `path` on background threads; dropping it stops them.
pub struct Pool {
    shared: Arc<Shared>,
}

impl Pool {
    /// Starts opening `path`. This can run before the window exists, so the file is
    /// read (and `first` rendered) while the window and OpenGL start up.
    pub fn open(path: &std::path::Path, workers: usize, first: Vec<Job>) -> Pool {
        let shared = Arc::new(Shared {
            path: path.to_string_lossy().into_owned(),
            ctx: std::sync::OnceLock::new(),
            q: Mutex::new(Queue { wanted: first, busy: HashSet::new(), done: HashSet::new(), out: Vec::new(), password: None, state: State::Opening }),
            cv: Condvar::new(),
        });
        for i in 0..workers.max(1) {
            let sh = shared.clone();
            let _ = std::thread::Builder::new().name(format!("render-{i}")).spawn(move || worker(sh, i == 0));
        }
        Pool { shared }
    }

    /// Connects the pool to the window, which then repaints whenever results arrive.
    pub fn attach(&self, ctx: &egui::Context) {
        let _ = self.shared.ctx.set(ctx.clone());
        ctx.request_repaint();
    }

    /// Replaces the wish list, most urgent first.
    pub fn want(&self, jobs: Vec<Job>) {
        let mut q = self.shared.q.lock().unwrap();
        if q.wanted != jobs {
            q.wanted = jobs;
            self.shared.cv.notify_all();
        }
    }

    pub fn take(&self) -> Vec<Output> {
        let mut q = self.shared.q.lock().unwrap();
        q.done.clear();
        std::mem::take(&mut q.out)
    }

    pub fn password(&self, pw: String) {
        self.shared.q.lock().unwrap().password = Some(pw);
        self.shared.cv.notify_all();
    }

    /// The confirmed password, for printing and search (which open their own copies).
    pub fn credentials(&self) -> (String, Option<String>) {
        let q = self.shared.q.lock().unwrap();
        (self.shared.path.clone(), q.password.clone())
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.q.lock().unwrap().state = State::Closed;
        self.shared.cv.notify_all();
    }
}

impl Shared {
    fn push(&self, q: &mut Queue, o: Output) {
        q.out.push(o);
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }
}

/// Opens a copy of the document for a thread other than the workers (search, printing).
pub fn open_copy(path: &str, password: Option<&str>) -> Option<Document> {
    fonts::install();
    let mut doc = Document::open(path).ok()?;
    if doc.needs_password().unwrap_or(false) && !doc.authenticate(password.unwrap_or("")).unwrap_or(false) {
        return None;
    }
    Some(doc)
}

fn worker(sh: Arc<Shared>, primary: bool) {
    fonts::install();
    let doc = if primary {
        match open_primary(&sh) {
            Some(d) => d,
            None => return,
        }
    } else {
        // Wait until the first worker has opened the file (and the password is known).
        let pw = {
            let mut q = sh.q.lock().unwrap();
            while q.state == State::Opening {
                q = sh.cv.wait(q).unwrap();
            }
            if q.state == State::Closed {
                return;
            }
            q.password.clone()
        };
        match open_copy(&sh.path, pw.as_deref()) {
            Some(d) => d,
            None => return,
        }
    };
    loop {
        let job = {
            let mut q = sh.q.lock().unwrap();
            loop {
                if q.state == State::Closed {
                    return;
                }
                let next = q.wanted.iter().find(|j| !q.busy.contains(j) && !q.done.contains(j)).copied();
                if let Some(j) = next {
                    q.busy.insert(j);
                    break j;
                }
                q = sh.cv.wait(q).unwrap();
            }
        };
        let out = run(&doc, job);
        let mut q = sh.q.lock().unwrap();
        q.busy.remove(&job);
        q.done.insert(job);
        if let Some(o) = out {
            sh.push(&mut q, o);
        }
    }
}

fn open_primary(sh: &Shared) -> Option<Document> {
    let mut doc = match Document::open(sh.path.as_str()) {
        Ok(d) => d,
        Err(e) => {
            let mut q = sh.q.lock().unwrap();
            q.state = State::Closed;
            sh.push(&mut q, Output::Failed(e.to_string()));
            sh.cv.notify_all();
            return None;
        }
    };
    if doc.needs_password().unwrap_or(false) {
        let mut wrong = false;
        loop {
            let pw = {
                let mut q = sh.q.lock().unwrap();
                q.password = None;
                sh.push(&mut q, Output::NeedPassword { wrong });
                while q.password.is_none() && q.state != State::Closed {
                    q = sh.cv.wait(q).unwrap();
                }
                if q.state == State::Closed {
                    return None;
                }
                q.password.clone().unwrap_or_default()
            };
            if doc.authenticate(&pw).unwrap_or(false) {
                break;
            }
            wrong = true;
        }
    }
    let pages = doc.page_count().unwrap_or(0).max(0) as usize;
    let first = doc.load_page(0).ok().and_then(|p| p.bounds().ok()).map_or(vec2(612.0, 792.0), |b| vec2(b.x1 - b.x0, b.y1 - b.y0));
    {
        let mut q = sh.q.lock().unwrap();
        q.state = State::Ready;
        sh.push(&mut q, Output::Opened { pages, first });
        sh.cv.notify_all();
    }
    // The contents come second: the other workers render page 1 meanwhile.
    let toc = doc.outlines().map(|o| flatten(&o)).unwrap_or_default();
    let mut q = sh.q.lock().unwrap();
    sh.push(&mut q, Output::Toc(toc));
    drop(q);
    Some(doc)
}

fn flatten(items: &[mupdf::Outline]) -> Vec<TocEntry> {
    fn walk(items: &[mupdf::Outline], depth: usize, out: &mut Vec<TocEntry>) {
        for o in items {
            let target = match (&o.dest, &o.uri) {
                (Some(d), _) => Some(Target::Page { page: d.loc.page_number as usize, y: dest_top(d.kind) }),
                (None, Some(u)) if is_external(u) => Some(Target::Uri(u.clone())),
                _ => None,
            };
            let i = out.len();
            out.push(TocEntry { title: o.title.trim().to_owned(), depth, target, descendants: 0 });
            walk(&o.down, depth + 1, out);
            out[i].descendants = out.len() - i - 1;
        }
    }
    let mut out = Vec::new();
    walk(items, 0, &mut out);
    out
}

fn dest_top(k: DestinationKind) -> Option<f32> {
    match k {
        DestinationKind::XYZ { top, .. } | DestinationKind::FitH { top } | DestinationKind::FitBH { top } => top,
        DestinationKind::FitR { top, bottom, .. } => Some(top.min(bottom)),
        _ => None,
    }
    .filter(|t| t.is_finite())
}

fn is_external(uri: &str) -> bool {
    uri.split_once(':').is_some_and(|(scheme, _)| scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
}

fn run(doc: &Document, job: Job) -> Option<Output> {
    match job {
        Job::Render { page, w, invert } => Some(render(doc, page, w, invert).map_or(Output::Broken { page }, |image| Output::Image { job, image })),
        Job::Thumb { page, invert } => Some(render(doc, page, THUMB_W, invert).map_or(Output::Broken { page }, |image| Output::Image { job, image })),
        Job::Text { page } => Some(Output::Text { page, text: text(doc, page).unwrap_or_default() }),
        Job::Sizes { from } => {
            let n = doc.page_count().unwrap_or(0).max(0) as usize;
            let sizes = (from..n.min(from + SIZE_CHUNK))
                .map(|i| doc.load_page(i as i32).ok().and_then(|p| p.bounds().ok()).map_or(vec2(612.0, 792.0), |b| vec2(b.x1 - b.x0, b.y1 - b.y0)))
                .collect();
            Some(Output::Sizes { from, sizes })
        }
    }
}

/// Renders a page `w` pixels wide, with annotations, as an RGB image.
fn render(doc: &Document, page: usize, w: u32, invert: bool) -> Option<ColorImage> {
    let p = doc.load_page(page as i32).ok()?;
    let b = p.bounds().ok()?;
    let s = w as f32 / (b.x1 - b.x0).max(1.0);
    let pix = p.to_pixmap(&Matrix::new_scale(s, s), &Colorspace::device_rgb(), false, true).ok()?;
    let (pw, ph, n) = (pix.width() as usize, pix.height() as usize, pix.n() as usize);
    let stride = pix.stride() as usize;
    let src = pix.samples();
    if n != 3 || pw == 0 || ph == 0 {
        return None;
    }
    let mut pixels = Vec::with_capacity(pw * ph);
    for row in src.chunks(stride).take(ph) {
        for px in row[..pw * 3].as_chunks::<3>().0 {
            pixels.push(if invert { invert_px(px[0], px[1], px[2]) } else { Color32::from_rgb(px[0], px[1], px[2]) });
        }
    }
    Some(ColorImage { size: [pw, ph], pixels })
}

/// Dark mode: inverts brightness but keeps hues (so red stays red), and lifts pure
/// black to the dark grey of the window background.
pub fn invert_px(r: u8, g: u8, b: u8) -> Color32 {
    let (r, g, b) = (255 - r as i32, 255 - g as i32, 255 - b as i32);
    let m2 = (r + g + b) * 2 / 3;
    let f = |c: i32| (30 + (m2 - c).clamp(0, 255) * 205 / 255) as u8;
    Color32::from_rgb(f(r), f(g), f(b))
}

fn text(doc: &Document, page: usize) -> Option<PageText> {
    let p = doc.load_page(page as i32).ok()?;
    let mut out = PageText::default();
    if let Ok(tp) = p.to_text_page(TextPageFlags::empty()) {
        for block in tp.blocks() {
            if block.r#type() != TextBlockType::Text {
                continue;
            }
            for line in block.lines() {
                let start = out.chars.len();
                for ch in line.chars() {
                    let Some(c) = ch.char() else { continue };
                    let q = ch.quad();
                    let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                    let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                    let min = pos2(xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min));
                    let max = pos2(xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max));
                    if min.x.is_finite() && max.y.is_finite() {
                        out.chars.push(Ch { c, rect: Rect::from_min_max(min, max), eol: false });
                    }
                }
                if out.chars.len() > start {
                    out.chars.last_mut().unwrap().eol = true;
                }
            }
        }
    }
    if let Ok(links) = p.links() {
        for l in links {
            let rect = Rect::from_min_max(pos2(l.bounds.x0, l.bounds.y0), pos2(l.bounds.x1, l.bounds.y1));
            let target = match &l.dest {
                Some(d) => Target::Page { page: d.loc.page_number as usize, y: dest_top(d.kind) },
                None if is_external(&l.uri) => Target::Uri(l.uri.clone()),
                None => match doc.resolve_link(&l.uri) {
                    Ok(Some(d)) => Target::Page { page: d.loc.page_number as usize, y: dest_top(d.kind) },
                    _ => continue,
                },
            };
            out.links.push(LinkArea { rect, target });
        }
    }
    Some(out)
}

/// Fonts a PDF names without embedding them, taken from Windows instead of being
/// replaced by a lookalike (or, for Chinese/Japanese/Korean, left blank).
mod fonts {
    use mupdf::{CjkFontOrdering, Font, FontHints, FontLoader};
    use std::collections::HashMap;
    use std::sync::{Mutex, Once};

    /// Font files read so far (each worker's copy of the document asks again).
    type Cache = HashMap<&'static str, Option<Vec<u8>>>;
    static FILES: Mutex<Option<Cache>> = Mutex::new(None);

    fn file(name: &'static str) -> Option<Vec<u8>> {
        let mut g = FILES.lock().unwrap();
        g.get_or_insert_with(HashMap::new)
            .entry(name)
            .or_insert_with(|| {
                let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
                std::fs::read(std::path::Path::new(&windir).join("Fonts").join(name)).ok()
            })
            .clone()
    }

    fn load(files: &[&'static str]) -> Option<Font> {
        files.iter().find_map(|f| file(f).and_then(|d| Font::from_bytes_with_index(f, 0, &d).ok()))
    }

    struct Windows;

    impl FontLoader for Windows {
        fn load_font(&self, name: &str, h: FontHints) -> Option<Font> {
            // "ABCDEF+Arial,Bold" -> "arialbold"
            let name = name.rsplit('+').next().unwrap_or(name);
            let key: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
            let bold = h.bold || key.contains("bold");
            let italic = h.italic || key.contains("italic") || key.contains("oblique");
            let pick = |r: &'static str, b: &'static str, i: &'static str, bi: &'static str| match (bold, italic) {
                (false, false) => r,
                (true, false) => b,
                (false, true) => i,
                (true, true) => bi,
            };
            let f = if key.starts_with("arial") {
                pick("arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf")
            } else if key.starts_with("timesnewroman") {
                pick("times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf")
            } else if key.starts_with("couriernew") {
                pick("cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf")
            } else if key.starts_with("calibri") {
                pick("calibri.ttf", "calibrib.ttf", "calibrii.ttf", "calibriz.ttf")
            } else if key.starts_with("cambria") {
                pick("cambria.ttc", "cambriab.ttf", "cambriai.ttf", "cambriaz.ttf")
            } else if key.starts_with("verdana") {
                pick("verdana.ttf", "verdanab.ttf", "verdanai.ttf", "verdanaz.ttf")
            } else if key.starts_with("georgia") {
                pick("georgia.ttf", "georgiab.ttf", "georgiai.ttf", "georgiaz.ttf")
            } else if key.starts_with("tahoma") {
                pick("tahoma.ttf", "tahomabd.ttf", "tahoma.ttf", "tahomabd.ttf")
            } else if key.starts_with("segoeui") {
                pick("segoeui.ttf", "segoeuib.ttf", "segoeuii.ttf", "segoeuiz.ttf")
            } else if key.starts_with("consolas") {
                pick("consola.ttf", "consolab.ttf", "consolai.ttf", "consolaz.ttf")
            } else if key.starts_with("trebuchet") {
                pick("trebuc.ttf", "trebucbd.ttf", "trebucit.ttf", "trebucbi.ttf")
            } else if key.starts_with("garamond") {
                "gara.ttf"
            } else if key.starts_with("bookantiqua") {
                pick("bkant.ttf", "antquab.ttf", "antquai.ttf", "antquabi.ttf")
            } else if key.starts_with("wingdings") {
                "wingding.ttf"
            } else if key.starts_with("msgothic") || key.starts_with("mspgothic") {
                "msgothic.ttc"
            } else if key.starts_with("msmincho") || key.starts_with("mspmincho") {
                "msmincho.ttc"
            } else if key.starts_with("simsun") {
                "simsun.ttc"
            } else if key.starts_with("simhei") {
                "simhei.ttf"
            } else if key.starts_with("malgun") {
                "malgun.ttf"
            } else {
                return None;
            };
            load(&[f])
        }

        fn load_cjk_font(&self, _name: &str, ordering: CjkFontOrdering, serif: bool) -> Option<Font> {
            match ordering {
                CjkFontOrdering::AdobeJapan => load(if serif { &["msmincho.ttc", "YuGothM.ttc"] } else { &["YuGothM.ttc", "msgothic.ttc"] }),
                CjkFontOrdering::AdobeKorea => load(&["malgun.ttf", "batang.ttc"]),
                CjkFontOrdering::AdobeGb => load(if serif { &["simsun.ttc", "msyh.ttc"] } else { &["msyh.ttc", "simsun.ttc"] }),
                CjkFontOrdering::AdobeCns => load(&["msjh.ttc", "mingliub.ttc"]),
            }
        }

        fn load_fallback_font(&self, script: u32, _language: u32, _h: FontHints) -> Option<Font> {
            use mupdf_sys as s;
            match script {
                s::UCDN_SCRIPT_HAN => load(&["msyh.ttc", "simsun.ttc"]),
                s::UCDN_SCRIPT_HIRAGANA | s::UCDN_SCRIPT_KATAKANA => load(&["YuGothM.ttc", "msgothic.ttc"]),
                s::UCDN_SCRIPT_HANGUL => load(&["malgun.ttf"]),
                s::UCDN_SCRIPT_ARABIC | s::UCDN_SCRIPT_HEBREW => load(&["arial.ttf"]),
                s::UCDN_SCRIPT_DEVANAGARI | s::UCDN_SCRIPT_BENGALI | s::UCDN_SCRIPT_TAMIL | s::UCDN_SCRIPT_TELUGU | s::UCDN_SCRIPT_GUJARATI | s::UCDN_SCRIPT_KANNADA | s::UCDN_SCRIPT_MALAYALAM | s::UCDN_SCRIPT_GURMUKHI => load(&["Nirmala.ttf"]),
                s::UCDN_SCRIPT_THAI => load(&["LeelawUI.ttf", "tahoma.ttf"]),
                _ => load(&["seguisym.ttf"]),
            }
        }
    }

    pub fn install() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| mupdf::set_font_loader(Windows));
    }
}
