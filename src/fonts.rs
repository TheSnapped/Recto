//! The UI font is Windows' own Segoe UI (egui's bundled fonts are left out of the exe).
//! Symbol, CJK and Indic fonts load only once text needing them shows up, such as a
//! Japanese table of contents.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

struct Script {
    name: &'static str,
    files: &'static [&'static str],
    test: fn(u32) -> bool,
}

const SCRIPTS: &[Script] = &[
    Script {
        name: "cjk",
        // Chinese (YaHei), then Japanese kana (Yu Gothic), then Korean (Malgun Gothic).
        files: &["msyh.ttc", "YuGothM.ttc", "malgun.ttf"],
        test: |c| matches!(c, 0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF),
    },
    Script { name: "indic", files: &["Nirmala.ttf"], test: |c| (0x0900..=0x0DFF).contains(&c) },
    Script { name: "thai", files: &["LeelawUI.ttf"], test: |c| (0x0E00..=0x0EFF).contains(&c) || (0x1780..=0x17FF).contains(&c) },
    // Arrows, check marks, dingbats and the like.
    Script { name: "symbols", files: &["seguisym.ttf"], test: |c| (0x2190..=0x2BFF).contains(&c) || (0x1F000..=0x1FAFF).contains(&c) },
    Script { name: "ethiopic", files: &["ebrima.ttf"], test: |c| (0x1200..=0x139F).contains(&c) || (0x07C0..=0x07FF).contains(&c) },
];

pub struct Fonts {
    defs: FontDefinitions,
    loaded: Vec<&'static str>,
}

fn file(name: &str) -> Option<Vec<u8>> {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    std::fs::read(std::path::Path::new(&windir).join("Fonts").join(name)).ok()
}

impl Fonts {
    pub fn install(ctx: &egui::Context) -> Self {
        let mut defs = FontDefinitions::empty();
        let mut add = |key: &str, data: Vec<u8>| {
            defs.font_data.insert(key.into(), Arc::new(FontData::from_owned(data)));
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families.entry(fam).or_default().push(key.into());
            }
        };
        match file("segoeui.ttf").or_else(|| file("arial.ttf")) {
            Some(d) => add("ui", d),
            // egui needs some font; a Windows without Segoe UI and Arial is unheard of.
            None => panic!("no UI font found in the Windows font folder"),
        }
        ctx.set_fonts(defs.clone());
        Fonts { defs, loaded: Vec::new() }
    }

    /// Loads fallback fonts for any script in `text` that isn't covered yet.
    pub fn cover(&mut self, ctx: &egui::Context, text: &str) {
        if text.is_ascii() {
            return;
        }
        let mut changed = false;
        for s in SCRIPTS {
            if self.loaded.contains(&s.name) || !text.chars().any(|c| (s.test)(c as u32)) {
                continue;
            }
            self.loaded.push(s.name);
            for (i, f) in s.files.iter().enumerate() {
                if let Some(d) = file(f) {
                    let key = format!("{}-{i}", s.name);
                    self.defs.font_data.insert(key.clone(), Arc::new(FontData::from_owned(d)));
                    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                        self.defs.families.entry(fam).or_default().push(key.clone());
                    }
                    changed = true;
                }
            }
        }
        if changed {
            ctx.set_fonts(self.defs.clone());
        }
    }
}
