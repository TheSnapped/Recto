//! Startup measurement for testing: with `RECTO_TIMING=<file>` set, Recto appends how
//! long after process creation the first frame and the first page image appeared.
//! `RECTO_EXIT=1` additionally closes the window once the page (or, without a file,
//! the first frame) is shown.

use eframe::egui::{self, ViewportCommand};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

pub struct Timing {
    file: Option<String>,
    exit: bool,
    window: bool,
    page: bool,
}

fn ms_since_start() -> f64 {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut a, mut b, mut c, mut now) = (z, z, z, z, z);
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
    unsafe {
        GetProcessTimes(GetCurrentProcess(), &mut created, &mut a, &mut b, &mut c);
        GetSystemTimePreciseAsFileTime(&mut now);
    }
    t(now) - t(created)
}

impl Timing {
    pub fn from_env() -> Timing {
        Timing { file: std::env::var("RECTO_TIMING").ok(), exit: std::env::var("RECTO_EXIT").is_ok_and(|v| v == "1"), window: false, page: false }
    }

    /// Logs a named point in time (when timing is on).
    pub fn mark(what: &str) {
        if let Ok(f) = std::env::var("RECTO_TIMING") {
            Timing { file: Some(f), exit: false, window: false, page: false }.log(what);
        }
    }

    fn log(&self, what: &str) {
        if let Some(f) = &self.file {
            use std::io::Write as _;
            if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
                let _ = writeln!(out, "{what}={:.1}", ms_since_start());
            }
        }
    }

    pub fn frame(&mut self, ctx: &egui::Context) {
        if self.file.is_none() || self.window {
            return;
        }
        self.window = true;
        self.log("first_frame_ms");
        if self.exit && std::env::args_os().len() < 2 {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    pub fn page_shown(&mut self, ctx: &egui::Context) {
        if self.file.is_none() || self.page {
            return;
        }
        self.page = true;
        self.log("first_page_ms");
        if self.exit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}
