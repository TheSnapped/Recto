// Embeds the icon and version info into recto.exe (shown by Explorer, the taskbar and Alt-Tab).
fn main() {
    println!("cargo:rerun-if-changed=assets/recto.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/recto.ico")
            .set("ProductName", "Recto")
            .set("FileDescription", "Recto PDF viewer");
        res.compile().expect("failed to embed Windows resources");
    }
}
