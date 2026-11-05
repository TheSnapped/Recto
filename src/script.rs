//! Test driver, built only with `--features script`: `RECTO_SCRIPT=<file>` replays
//! input from a file (one step per line) and saves screenshots, so the UI can be tested
//! without touching the real mouse and keyboard.
//!
//!   wait 300            key j / key ctrl+f / key shift+f3      text zebra
//!   click 400 300       dblclick 400 300     tripleclick 400 300     rclick 400 300
//!   drag 100 200 400 260     wheel 400 300 -120 [ctrl]     move 400 300
//!   shot out.png        drop C:\file.pdf     quit

use eframe::egui::{self, pos2, vec2, Event, Key, Modifiers, PointerButton, RawInput};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct Script {
    steps: VecDeque<String>,
    next: Instant,
    shot: Option<String>,
}

impl Script {
    /// Keeps frames coming, paced, while steps remain (called from `update`).
    pub fn keep_alive(&self, ctx: &egui::Context) {
        if !self.steps.is_empty() || self.shot.is_some() {
            crate::pace(ctx);
        }
    }
}

impl Script {
    pub fn from_env() -> Option<Script> {
        let f = std::env::var("RECTO_SCRIPT").ok()?;
        let text = std::fs::read_to_string(f).ok()?;
        let steps = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
        Some(Script { steps, next: Instant::now() + Duration::from_millis(400), shot: None })
    }

    pub fn hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        for e in &raw.events {
            if let Event::Screenshot { image, .. } = e {
                if let Some(path) = self.shot.take() {
                    let _ = std::fs::write(path, png(image));
                }
            }
        }
        // Frames keep coming through `keep_alive` (paced); asking for repaints here would spin.
        if Instant::now() < self.next || self.shot.is_some() {
            return;
        }
        let Some(line) = self.steps.pop_front() else { return };
        if let Ok(f) = std::env::var("RECTO_SCRIPT") {
            use std::io::Write as _;
            if let Ok(mut o) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{f}.log")) {
                let _ = writeln!(o, "{line}");
            }
        }
        let mut it = line.split_whitespace();
        let cmd = it.next().unwrap_or_default();
        let args: Vec<&str> = it.collect();
        let num = |i: usize| args.get(i).and_then(|a| a.parse::<f32>().ok()).unwrap_or(0.0);
        let p = pos2(num(0), num(1));
        let mut wait = 60;
        let ev = &mut raw.events;
        match cmd {
            "wait" => wait = num(0) as u64,
            "key" => {
                let spec = args.first().copied().unwrap_or_default();
                let mut mods = Modifiers::NONE;
                let mut name = spec;
                while let Some((m, rest)) = name.split_once('+') {
                    match m {
                        "ctrl" => mods |= Modifiers::COMMAND,
                        "shift" => mods |= Modifiers::SHIFT,
                        "alt" => mods |= Modifiers::ALT,
                        _ => {}
                    }
                    name = rest;
                }
                raw.modifiers = mods;
                if let Some(key) = Key::from_name(name).or_else(|| Key::from_name(&name.to_uppercase())) {
                    ev.push(Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: mods });
                    ev.push(Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers: mods });
                }
                // Typed characters for letter commands (as a keyboard would produce them).
                if name.chars().count() == 1 && (mods.is_none() || mods == Modifiers::SHIFT) {
                    let c = name.chars().next().unwrap();
                    let c = if mods.shift { c.to_ascii_uppercase() } else { c };
                    ev.push(Event::Text(c.to_string()));
                }
            }
            "text" => ev.push(Event::Text(args.join(" "))),
            "move" => ev.push(Event::PointerMoved(p)),
            "click" | "dblclick" | "tripleclick" | "rclick" => {
                let button = if cmd == "rclick" { PointerButton::Secondary } else { PointerButton::Primary };
                let n = match cmd {
                    "dblclick" => 2,
                    "tripleclick" => 3,
                    _ => 1,
                };
                // One press or release per frame, as real clicks arrive.
                ev.push(Event::PointerMoved(p));
                let b = if button == PointerButton::Secondary { 1 } else { 0 };
                let mut seq = vec![format!("_release {} {} {b}", p.x, p.y)];
                for _ in 1..n {
                    seq.push(format!("_press {} {} {b}", p.x, p.y));
                    seq.push(format!("_release {} {} {b}", p.x, p.y));
                }
                for s in seq.into_iter().rev() {
                    self.steps.push_front(s);
                }
                ev.push(Event::PointerButton { pos: p, button, pressed: true, modifiers: Modifiers::NONE });
                wait = 20;
            }
            "_press" | "_release" => {
                let button = if num(2) == 1.0 { PointerButton::Secondary } else { PointerButton::Primary };
                ev.push(Event::PointerButton { pos: p, button, pressed: cmd == "_press", modifiers: Modifiers::NONE });
                wait = 20;
            }
            "drag" => {
                let to = pos2(num(2), num(3));
                ev.push(Event::PointerMoved(p));
                ev.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
                let mut seq = Vec::new();
                for k in 1..=12 {
                    let q = p + (to - p) * (k as f32 / 12.0);
                    seq.push(format!("move {} {}", q.x, q.y));
                    seq.push("wait 16".to_owned());
                }
                seq.push(format!("_release {} {} 0", to.x, to.y));
                for s in seq.into_iter().rev() {
                    self.steps.push_front(s);
                }
                wait = 16;
            }
            "wheel" => {
                let ctrl = args.get(3) == Some(&"ctrl");
                let mods = if ctrl { Modifiers::COMMAND } else { Modifiers::NONE };
                raw.modifiers = mods;
                ev.push(Event::PointerMoved(p));
                ev.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, num(2)), modifiers: mods });
                wait = 30;
            }
            "shot" => {
                self.shot = Some(args.join(" "));
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            "drop" => raw.dropped_files.push(egui::DroppedFile { path: Some(args.join(" ").into()), ..Default::default() }),
            "quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            _ => {}
        }
        self.next = Instant::now() + Duration::from_millis(wait);
    }
}

/// A minimal PNG writer (stored, uncompressed deflate blocks).
fn png(img: &egui::ColorImage) -> Vec<u8> {
    let (w, h) = (img.size[0], img.size[1]);
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h {
        raw.push(0);
        for p in &img.pixels[y * w..(y + 1) * w] {
            raw.extend_from_slice(&p.to_array());
        }
    }
    let mut z = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let crc = |data: &[u8]| {
        let mut c = !0u32;
        for &x in data {
            c ^= x as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |t: &[u8], d: &[u8]| {
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        let mut td = t.to_vec();
        td.extend_from_slice(d);
        out.extend_from_slice(&td);
        out.extend_from_slice(&crc(&td).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}
