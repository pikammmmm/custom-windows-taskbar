//! Window snapshot capture: PrintWindow -> 32bpp DIB -> downscaled PNG ->
//! base64 data URL. Mirrors the GDI lifecycle proven in icons.rs.

use anyhow::{anyhow, Result};
use base64::Engine;
use windows::Win32::Foundation::{HANDLE, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};

// PW_RENDERFULLCONTENT — capture composited/DirectComposition content (Chrome,
// Electron, UWP). Not always exported as a named const in windows 0.58.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(0x0000_0002);

/// Capture `hwnd` to a downscaled PNG data URL whose longest side is `max_px`.
/// Returns Err for minimized/zero-size windows or when PrintWindow yields a
/// blank buffer (hardware-accelerated fullscreen) — callers fall back to the
/// app icon.
pub fn window_thumbnail_data_url(hwnd: isize, max_px: u32) -> Result<String> {
    let jpeg = window_thumbnail_bytes(hwnd, max_px)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&jpeg)
    ))
}

fn window_thumbnail_bytes(hwnd: isize, max_px: u32) -> Result<Vec<u8>> {
    let h = HWND(hwnd as *mut _);
    unsafe {
        if IsIconic(h).as_bool() {
            return Err(anyhow!("minimized"));
        }
        let mut rect = RECT::default();
        GetWindowRect(h, &mut rect)?;
        let w = (rect.right - rect.left).max(0) as u32;
        let ht = (rect.bottom - rect.top).max(0) as u32;
        if w == 0 || ht == 0 {
            return Err(anyhow!("zero-size window"));
        }

        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(anyhow!("GetDC failed"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            ReleaseDC(None, screen_dc);
            return Err(anyhow!("CreateCompatibleDC failed"));
        }

        let result = capture_into_dib(h, screen_dc, mem_dc, w, ht);

        // Matched-pair cleanup (leaking DCs exhausts GDI after ~10 captures).
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        let (mut rgba, cw, ch) = result?;

        // Reject all-black/empty captures (hardware-accelerated content).
        if !rgba.chunks_exact(4).any(|px| px[0] | px[1] | px[2] != 0) {
            return Err(anyhow!("blank capture"));
        }
        // PrintWindow gives opaque content; force full alpha so resize/encoding
        // doesn't multiply against garbage alpha.
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }

        let img = image::RgbaImage::from_raw(cw, ch, rgba)
            .ok_or_else(|| anyhow!("from_raw failed"))?;
        let (tw, th) = fit(cw, ch, max_px);
        // Lanczos3 keeps text/edges crisp when downscaling a large window.
        let scaled = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Lanczos3);
        // JPEG (q88): far smaller than PNG for screenshots, so we can afford a
        // much higher capture resolution for the same IPC payload. Window
        // content is opaque, so dropping alpha is fine.
        let rgb = image::DynamicImage::ImageRgba8(scaled).to_rgb8();
        let mut out = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
        enc.encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
        Ok(out)
    }
}

/// Render `hwnd` into a fresh top-down 32bpp DIB and return (RGBA, w, h).
unsafe fn capture_into_dib(
    h: HWND,
    screen_dc: HDC,
    mem_dc: HDC,
    w: u32,
    ht: u32,
) -> Result<(Vec<u8>, u32, u32)> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(ht as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib: HBITMAP =
        CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE(std::ptr::null_mut()), 0)?;
    if dib.is_invalid() || bits.is_null() {
        // CreateDIBSection can (pathologically) return a valid handle with null
        // bits. The `?` above only catches the null-handle case, so free a
        // valid-but-unusable DIB here to avoid a GDI handle leak.
        if !dib.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(dib.0 as *mut _));
        }
        return Err(anyhow!("CreateDIBSection failed"));
    }
    let prev = SelectObject(mem_dc, HGDIOBJ(dib.0 as *mut _));

    let ok = PrintWindow(h, mem_dc, PW_RENDERFULLCONTENT);

    // Copy bits out before freeing the DIB.
    let mut rgba = std::slice::from_raw_parts(bits as *const u8, (w * ht * 4) as usize).to_vec();
    SelectObject(mem_dc, prev);
    let _ = DeleteObject(HGDIOBJ(dib.0 as *mut _));

    if !ok.as_bool() {
        return Err(anyhow!("PrintWindow returned false"));
    }
    // BGRA -> RGBA
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok((rgba, w, ht))
}

/// Scale (w,h) so the longest side == max_px (never upscale).
fn fit(w: u32, h: u32, max_px: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= max_px || longest == 0 {
        return (w.max(1), h.max(1));
    }
    let s = max_px as f64 / longest as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_caps_longest_side() {
        assert_eq!(fit(1920, 1080, 480), (480, 270));
        assert_eq!(fit(1080, 1920, 480), (270, 480));
    }

    #[test]
    fn fit_never_upscales() {
        assert_eq!(fit(200, 100, 480), (200, 100));
    }

    /// Live smoke test — capture the current foreground window.
    /// Run explicitly: `cargo test --bin glassbar capture::tests::live_capture -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_capture() {
        let hwnd = crate::win32::foreground_hwnd();
        match window_thumbnail_data_url(hwnd, 480) {
            Ok(url) => {
                assert!(url.starts_with("data:image/jpeg;base64,"));
                assert!(url.len() > 1000, "thumbnail suspiciously small: {}", url.len());
                println!("captured {} byte data url", url.len());
            }
            Err(e) => println!("capture failed (acceptable for some windows): {e}"),
        }
    }
}
