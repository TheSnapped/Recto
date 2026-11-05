//! Text search on its own thread, with its own copy of the document. It starts at
//! the current page, wraps around, and reports hits as it goes; a new query stops
//! the old one at the next page.

use eframe::egui::{self, pos2, Rect};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub struct Hit {
    pub page: usize,
    /// In page points.
    pub rect: Rect,
}

#[derive(Default)]
pub struct Progress {
    pub query: String,
    /// Sorted by page, then position on the page.
    pub hits: Vec<Hit>,
    pub scanned: usize,
    pub total: usize,
    pub done: bool,
    pub failed: bool,
}

struct Request {
    query: String,
    start: usize,
}

pub struct Searcher {
    tx: Option<Sender<Request>>,
    pub progress: Arc<Mutex<Progress>>,
}

impl Searcher {
    pub fn new() -> Searcher {
        Searcher { tx: None, progress: Arc::new(Mutex::new(Progress::default())) }
    }

    /// Starts searching `query` (case-insensitive) from page `start`.
    pub fn find(&mut self, path: &str, password: Option<String>, query: &str, start: usize, ctx: &egui::Context) {
        *self.progress.lock().unwrap() = Progress { query: query.to_owned(), ..Default::default() };
        if query.is_empty() {
            self.progress.lock().unwrap().done = true;
            return;
        }
        if self.tx.is_none() {
            let (tx, rx) = channel();
            let (path, progress, ctx) = (path.to_owned(), self.progress.clone(), ctx.clone());
            let _ = std::thread::Builder::new().name("search".into()).spawn(move || run(path, password, rx, progress, ctx));
            self.tx = Some(tx);
        }
        let _ = self.tx.as_ref().unwrap().send(Request { query: query.to_owned(), start });
    }
}

fn run(path: String, password: Option<String>, rx: Receiver<Request>, progress: Arc<Mutex<Progress>>, ctx: egui::Context) {
    let doc = crate::doc::open_copy(&path, password.as_deref());
    let mut next: Option<Request> = None;
    loop {
        let req = match next.take() {
            Some(r) => r,
            None => match rx.recv() {
                Ok(r) => r,
                Err(_) => return,
            },
        };
        let Some(doc) = &doc else {
            let mut p = progress.lock().unwrap();
            p.failed = true;
            p.done = true;
            ctx.request_repaint();
            continue;
        };
        let total = doc.page_count().unwrap_or(0).max(0) as usize;
        progress.lock().unwrap().total = total;
        // Wake the UI at most every 50 ms; fast pages would otherwise flood it.
        let mut shown = std::time::Instant::now();
        for k in 0..total {
            match rx.try_recv() {
                Ok(r) => {
                    next = Some(r);
                    break;
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {}
            }
            let page = (req.start + k) % total;
            let mut found = Vec::new();
            if let Ok(tp) = doc.load_page(page as i32).and_then(|p| p.to_text_page(mupdf::TextPageFlags::empty())) {
                if let Ok(quads) = tp.search(&req.query) {
                    for q in quads {
                        let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                        let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                        let min = pos2(xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min));
                        let max = pos2(xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max));
                        found.push(Hit { page, rect: Rect::from_min_max(min, max) });
                    }
                }
            }
            let mut p = progress.lock().unwrap();
            if p.query != req.query {
                break;
            }
            p.scanned = k + 1;
            let first = p.hits.is_empty() && !found.is_empty();
            if !found.is_empty() {
                let at = p.hits.partition_point(|h| h.page < page);
                p.hits.splice(at..at, found);
            }
            if first || shown.elapsed().as_millis() > 50 {
                shown = std::time::Instant::now();
                ctx.request_repaint();
            }
        }
        if next.is_none() {
            let mut p = progress.lock().unwrap();
            if p.query == req.query {
                p.done = true;
            }
            ctx.request_repaint();
        }
    }
}
