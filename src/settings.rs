//! App-wide preferences in %APPDATA%\Recto\settings.ini: window placement, dark mode,
//! the sidebar and the page layout. Nothing about individual files is stored.

use std::path::PathBuf;

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub size: [f32; 2],
    pub pos: Option<[f32; 2]>,
    pub maximized: bool,
    pub dark: bool,
    pub sidebar: bool,
    pub sidebar_width: f32,
    /// 0 = contents, 1 = thumbnails.
    pub sidebar_tab: u8,
    pub continuous: bool,
    pub last_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            size: [980.0, 760.0],
            pos: None,
            maximized: false,
            dark: false,
            sidebar: false,
            sidebar_width: 240.0,
            sidebar_tab: 0,
            continuous: true,
            last_dir: None,
        }
    }
}

pub fn dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Recto"))
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings::default();
        let text = dir().and_then(|d| std::fs::read_to_string(d.join("settings.ini")).ok()).unwrap_or_default();
        let (mut x, mut y) = (None, None);
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            let num = |lo: f32, hi: f32, cur: f32| v.parse::<f32>().ok().filter(|n| n.is_finite()).map_or(cur, |n| n.clamp(lo, hi));
            let flag = v == "true";
            match k.trim() {
                "width" => s.size[0] = num(360.0, 10000.0, s.size[0]),
                "height" => s.size[1] = num(240.0, 10000.0, s.size[1]),
                "x" => x = v.parse::<f32>().ok(),
                "y" => y = v.parse::<f32>().ok(),
                "maximized" => s.maximized = flag,
                "dark" => s.dark = flag,
                "sidebar" => s.sidebar = flag,
                "sidebar_width" => s.sidebar_width = num(160.0, 600.0, s.sidebar_width),
                "sidebar_tab" => s.sidebar_tab = v.parse().unwrap_or(0).min(1),
                "continuous" => s.continuous = flag,
                "last_dir" if !v.is_empty() => s.last_dir = Some(PathBuf::from(v)),
                _ => {}
            }
        }
        s.pos = x.zip(y).map(|(x, y)| [x, y]);
        s
    }

    pub fn save(&self) {
        let Some(d) = dir() else { return };
        let mut text = format!(
            "width={}\nheight={}\nmaximized={}\ndark={}\nsidebar={}\nsidebar_width={}\nsidebar_tab={}\ncontinuous={}\n",
            self.size[0], self.size[1], self.maximized, self.dark, self.sidebar, self.sidebar_width, self.sidebar_tab, self.continuous,
        );
        if let Some([x, y]) = self.pos {
            text += &format!("x={x}\ny={y}\n");
        }
        if let Some(l) = &self.last_dir {
            text += &format!("last_dir={}\n", l.display());
        }
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("settings.ini"), text);
    }
}
