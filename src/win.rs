//! Small Win32 helpers: registering as a PDF viewer, the Open dialog, opening links,
//! and message boxes.
//!
//! Registration is per user (no admin rights needed). Windows doesn't let apps make
//! themselves the default, so after registering we open Settings > Default apps for the
//! user to pick Recto.

use std::path::PathBuf;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Shell::{SHChangeNotify, ShellExecuteW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const PROG_ID: &str = "Recto.PDF";
const CAPABILITIES: &str = r"Software\Recto\Capabilities";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Creates `key` under HKCU and sets the value `name` ("" for the default value).
fn set(key: &str, name: &str, value: Option<&str>) -> Result<(), String> {
    unsafe {
        let mut h: HKEY = null_mut();
        let k = wide(key);
        if RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, null_mut(), 0, KEY_WRITE, null_mut(), &mut h, null_mut()) != ERROR_SUCCESS {
            return Err(format!("can't create HKCU\\{key}"));
        }
        let n = wide(name);
        let r = match value {
            Some(v) => {
                let v = wide(v);
                RegSetValueExW(h, n.as_ptr(), 0, REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32)
            }
            None => RegSetValueExW(h, n.as_ptr(), 0, REG_NONE, null_mut(), 0),
        };
        RegCloseKey(h);
        if r != ERROR_SUCCESS {
            return Err(format!("can't write HKCU\\{key}\\{name}"));
        }
        Ok(())
    }
}

fn delete_tree(key: &str) {
    let k = wide(key);
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) };
}

fn delete_value(key: &str, name: &str) {
    unsafe {
        let mut h: HKEY = null_mut();
        let k = wide(key);
        if RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_WRITE, &mut h) == ERROR_SUCCESS {
            let n = wide(name);
            RegDeleteValueW(h, n.as_ptr());
            RegCloseKey(h);
        }
    }
}

fn notify() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

pub fn register() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    let classes = r"Software\Classes";

    set(&format!(r"{classes}\{PROG_ID}"), "", Some("PDF document"))?;
    set(&format!(r"{classes}\{PROG_ID}\DefaultIcon"), "", Some(&icon))?;
    set(&format!(r"{classes}\{PROG_ID}\shell\open\command"), "", Some(&open))?;

    let app = format!(r"{classes}\Applications\recto.exe");
    set(&app, "FriendlyAppName", Some("Recto"))?;
    set(&format!(r"{app}\shell\open\command"), "", Some(&open))?;
    set(&format!(r"{app}\SupportedTypes"), ".pdf", Some(""))?;

    set(CAPABILITIES, "ApplicationName", Some("Recto"))?;
    set(CAPABILITIES, "ApplicationDescription", Some("Lightweight PDF viewer"))?;
    set(CAPABILITIES, "ApplicationIcon", Some(&icon))?;
    set(&format!(r"{CAPABILITIES}\FileAssociations"), ".pdf", Some(PROG_ID))?;
    set(&format!(r"{classes}\.pdf\OpenWithProgids"), PROG_ID, None)?;
    set(r"Software\RegisteredApplications", "Recto", Some(CAPABILITIES))?;
    notify();
    Ok(())
}

pub fn unregister() {
    let classes = r"Software\Classes";
    delete_value(&format!(r"{classes}\.pdf\OpenWithProgids"), PROG_ID);
    delete_tree(&format!(r"{classes}\{PROG_ID}"));
    delete_tree(&format!(r"{classes}\Applications\recto.exe"));
    delete_tree(r"Software\Recto");
    delete_value(r"Software\RegisteredApplications", "Recto");
    notify();
}

/// Opens Settings > Default apps, where the user picks the PDF viewer.
pub fn open_default_apps() {
    // Windows 11 jumps straight to Recto's page; Windows 10 ignores the parameter.
    shell_open("ms-settings:defaultapps?registeredAppUser=Recto");
}

fn shell_open(target: &str) {
    let (verb, t) = (wide("open"), wide(target));
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), t.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

/// Opens a web or mail link from a document. Other schemes (file:, javascript:,
/// custom protocol handlers...) are refused: a PDF shouldn't be able to launch programs.
pub fn open_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        shell_open(url.trim());
        true
    } else {
        false
    }
}

pub fn message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let (t, c) = (wide(text), wide("Recto"));
    unsafe { MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

/// The standard Open dialog, filtered to PDFs.
pub fn open_dialog(dir: Option<&std::path::Path>) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut buf = vec![0u16; 32768];
    let filter: Vec<u16> = "PDF documents (*.pdf)\0*.pdf\0All files\0*.*\0\0".encode_utf16().collect();
    let dir = dir.map(|d| wide(&d.to_string_lossy()));
    let title = wide("Open PDF");
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = active_window();
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.lpstrInitialDir = dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
    ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_NOCHANGEDIR;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    use std::os::windows::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])))
}

/// Our window, as the owner of modal dialogs: the active one, else the UI thread's
/// visible top-level window (dialogs need an owner to be modal).
pub fn active_window() -> windows_sys::Win32::Foundation::HWND {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible};
    let h = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() };
    if !h.is_null() {
        return h;
    }
    unsafe extern "system" fn first_visible(h: HWND, out: LPARAM) -> BOOL {
        if unsafe { IsWindowVisible(h) } != 0 {
            unsafe { *(out as *mut HWND) = h };
            return 0;
        }
        1
    }
    let mut found: HWND = null_mut();
    unsafe { EnumThreadWindows(windows_sys::Win32::System::Threading::GetCurrentThreadId(), Some(first_visible), &mut found as *mut HWND as LPARAM) };
    found
}
