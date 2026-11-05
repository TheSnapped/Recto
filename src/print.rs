//! Printing through Windows' own print dialog and GDI: no extra programs and no temp
//! files. Each page is rendered by MuPDF at the printer's resolution (up to 300 dpi),
//! turned to match the paper, scaled to fit the printable area and sent as a bitmap.

use crate::win::wide;
use eframe::egui;
use mupdf::{Colorspace, Matrix};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::{EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};
use windows_sys::Win32::UI::Controls::Dialogs::*;

/// Shown as a toast while printing runs.
pub type Status = Arc<Mutex<Option<String>>>;

struct Dc(HDC);
// A device context may be used by one thread at a time; the print thread takes it over.
unsafe impl Send for Dc {}

/// Shows the print dialog; if the user prints, the pages are sent on a background thread.
pub fn print(path: String, password: Option<String>, pages: usize, current: usize, name: String, ctx: &egui::Context) -> Option<Status> {
    let mut ranges = [PRINTPAGERANGE { nFromPage: 1, nToPage: pages as u32 }; 16];
    let mut pd: PRINTDLGEXW = unsafe { std::mem::zeroed() };
    pd.lStructSize = std::mem::size_of::<PRINTDLGEXW>() as u32;
    pd.hwndOwner = crate::win::active_window();
    pd.Flags = PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE | PD_NOSELECTION;
    pd.nMaxPageRanges = ranges.len() as u32;
    pd.lpPageRanges = ranges.as_mut_ptr();
    pd.nMinPage = 1;
    pd.nMaxPage = pages as u32;
    pd.nCopies = 1;
    pd.nStartPage = START_PAGE_GENERAL;
    let hr = unsafe { PrintDlgExW(&mut pd) };
    unsafe {
        if !pd.hDevMode.is_null() {
            GlobalFree(pd.hDevMode);
        }
        if !pd.hDevNames.is_null() {
            GlobalFree(pd.hDevNames);
        }
    }
    if hr != 0 || pd.dwResultAction != PD_RESULT_PRINT || pd.hDC.is_null() {
        if !pd.hDC.is_null() {
            unsafe { DeleteDC(pd.hDC) };
        }
        return None;
    }
    let list: Vec<usize> = if pd.Flags & PD_CURRENTPAGE != 0 {
        vec![current]
    } else if pd.Flags & PD_PAGENUMS != 0 {
        ranges[..pd.nPageRanges as usize].iter().flat_map(|r| r.nFromPage.min(r.nToPage)..=r.nFromPage.max(r.nToPage)).map(|p| p as usize - 1).filter(|&p| p < pages).collect()
    } else {
        (0..pages).collect()
    };
    let status: Status = Arc::new(Mutex::new(Some("Preparing to print...".into())));
    let (st, ctx, dc) = (status.clone(), ctx.clone(), Dc(pd.hDC));
    let _ = std::thread::Builder::new().name("print".into()).spawn(move || {
        let dc = dc;
        let msg = match send(&dc, &path, password.as_deref(), &list, &name, &st, &ctx) {
            Ok(()) => format!("Sent {} page{} to the printer", list.len(), if list.len() == 1 { "" } else { "s" }),
            Err(e) => format!("Printing failed: {e}"),
        };
        unsafe { DeleteDC(dc.0) };
        *st.lock().unwrap() = Some(msg);
        ctx.request_repaint();
        std::thread::sleep(std::time::Duration::from_secs(3));
        *st.lock().unwrap() = None;
        ctx.request_repaint();
    });
    Some(status)
}

/// Test build only: prints `pages` to the printer `printer` (e.g. "Microsoft Print to
/// PDF") into the file `out`, without the dialog.
#[cfg(feature = "script")]
pub fn print_to_file(path: &str, password: Option<&str>, pages: &[usize], printer: &str, out: &str, ctx: &egui::Context) -> Result<(), String> {
    let (driver, dev) = (wide("WINSPOOL"), wide(printer));
    let hdc = unsafe { CreateDCW(driver.as_ptr(), dev.as_ptr(), std::ptr::null(), std::ptr::null()) };
    if hdc.is_null() {
        return Err(format!("no printer named {printer}"));
    }
    let st: Status = Arc::new(Mutex::new(None));
    let r = send_to(&Dc(hdc), path, password, pages, "Recto test", Some(out), &st, ctx);
    unsafe { DeleteDC(hdc) };
    r
}

fn send(dc: &Dc, path: &str, password: Option<&str>, list: &[usize], name: &str, st: &Status, ctx: &egui::Context) -> Result<(), String> {
    send_to(dc, path, password, list, name, None, st, ctx)
}

#[allow(clippy::too_many_arguments)]
fn send_to(dc: &Dc, path: &str, password: Option<&str>, list: &[usize], name: &str, output: Option<&str>, st: &Status, ctx: &egui::Context) -> Result<(), String> {
    let doc = crate::doc::open_copy(path, password).ok_or("can't open the document")?;
    let hdc = dc.0;
    let cap = |i| unsafe { GetDeviceCaps(hdc, i as i32) };
    let (pw, ph) = (cap(HORZRES), cap(VERTRES));
    let dpi = cap(LOGPIXELSX).max(72) as f32;
    if pw <= 0 || ph <= 0 {
        return Err("the printer reported no printable area".into());
    }
    let title = wide(name);
    let output = output.map(wide);
    let di = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        lpszOutput: output.as_ref().map_or(std::ptr::null(), |o| o.as_ptr()),
        lpszDatatype: std::ptr::null(),
        fwType: 0,
    };
    if unsafe { StartDocW(hdc, &di) } <= 0 {
        return Err("the print job couldn't be started".into());
    }
    for (n, &i) in list.iter().enumerate() {
        *st.lock().unwrap() = Some(format!("Printing page {} of {}...", n + 1, list.len()));
        ctx.request_repaint();
        let page = doc.load_page(i as i32).map_err(|e| e.to_string())?;
        let b = page.bounds().map_err(|e| e.to_string())?;
        let (w, h) = ((b.x1 - b.x0).max(1.0), (b.y1 - b.y0).max(1.0));
        // Turn landscape pages on portrait paper (and the other way round).
        let rotate = (w > h) != (pw > ph);
        let (fw, fh) = if rotate { (h, w) } else { (w, h) };
        // Device pixels: fit the printable area, keeping the aspect ratio.
        let fit = (pw as f32 / fw).min(ph as f32 / fh);
        let (dw, dh) = ((fw * fit) as i32, (fh * fit) as i32);
        // Render at no more than 300 dpi; the printer driver scales the rest.
        let s = fit * (300.0 / dpi).min(1.0);
        let m = if rotate { Matrix::new(0.0, s, -s, 0.0, 0.0, 0.0) } else { Matrix::new_scale(s, s) };
        let pix = page.to_pixmap(&m, &Colorspace::device_bgr(), false, true).map_err(|e| e.to_string())?;
        let (bw, bh, stride) = (pix.width() as usize, pix.height() as usize, pix.stride() as usize);
        // DIB rows are padded to 4 bytes.
        let row = (bw * 3 + 3) & !3;
        let mut bits = vec![0u8; row * bh];
        for (y, src) in pix.samples().chunks(stride).take(bh).enumerate() {
            bits[y * row..y * row + bw * 3].copy_from_slice(&src[..bw * 3]);
        }
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = bw as i32;
        bmi.bmiHeader.biHeight = -(bh as i32); // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 24;
        bmi.bmiHeader.biCompression = BI_RGB;
        unsafe {
            if StartPage(hdc) <= 0 {
                EndDoc(hdc);
                return Err("the printer refused a page".into());
            }
            SetStretchBltMode(hdc, HALFTONE);
            StretchDIBits(hdc, (pw - dw) / 2, (ph - dh) / 2, dw, dh, 0, 0, bw as i32, bh as i32, bits.as_ptr().cast(), &bmi, DIB_RGB_COLORS, SRCCOPY);
            if EndPage(hdc) <= 0 {
                EndDoc(hdc);
                return Err("the printer refused a page".into());
            }
        }
    }
    unsafe { EndDoc(hdc) };
    Ok(())
}
