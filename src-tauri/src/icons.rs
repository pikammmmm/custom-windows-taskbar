use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL, HICON};

use crate::config;

const ICON_SIZE: u32 = 32;

pub fn get_icon_png(exe_path: &str) -> Result<Vec<u8>> {
    let cache_path = cache_path_for(exe_path)?;
    if cache_path.exists() {
        return Ok(std::fs::read(&cache_path)?);
    }
    let png = extract_icon_png(exe_path)?;
    let _ = std::fs::write(&cache_path, &png);
    Ok(png)
}

pub fn get_icon_data_url(exe_path: &str) -> Result<String> {
    let png = get_icon_png(exe_path)?;
    use base64::Engine;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    ))
}

fn cache_path_for(exe_path: &str) -> Result<PathBuf> {
    let mut h = Sha256::new();
    h.update(exe_path.as_bytes());
    let hash = format!("{:x}", h.finalize());
    Ok(config::icon_cache_dir()?.join(format!("{}.png", &hash[..16])))
}

fn extract_icon_png(exe_path: &str) -> Result<Vec<u8>> {
    let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if result == 0 || info.hIcon.0.is_null() {
        return Err(anyhow!("SHGetFileInfoW returned no icon for {exe_path}"));
    }

    let png = hicon_to_png(info.hIcon, ICON_SIZE);
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    png
}

/// Renders an HICON onto a fresh 32bpp DIB section via DrawIconEx, then reads
/// the alpha-correct pixel buffer out as PNG. This handles color icons, mask
/// icons, and modern PNG-compressed icons uniformly.
fn hicon_to_png(hicon: HICON, size: u32) -> Result<Vec<u8>> {
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(anyhow!("GetDC returned null"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            ReleaseDC(None, screen_dc);
            return Err(anyhow!("CreateCompatibleDC failed"));
        }

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            screen_dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE(std::ptr::null_mut()),
            0,
        );
        let dib = match dib {
            Ok(h) if !h.is_invalid() && !bits.is_null() => h,
            _ => {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
                return Err(anyhow!("CreateDIBSection failed"));
            }
        };

        let prev = SelectObject(mem_dc, HGDIOBJ(dib.0 as *mut _));

        let draw_ok = DrawIconEx(
            mem_dc,
            0,
            0,
            hicon,
            size as i32,
            size as i32,
            0,
            None,
            DI_NORMAL,
        );

        // Restore selection so we can delete the DIB cleanly.
        SelectObject(mem_dc, prev);

        if let Err(e) = draw_ok {
            let _ = DeleteObject(HGDIOBJ(dib.0 as *mut _));
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            return Err(anyhow!("DrawIconEx failed: {e}"));
        }

        let len = (size * size * 4) as usize;
        let mut buf = std::slice::from_raw_parts(bits as *const u8, len).to_vec();

        let _ = DeleteObject(HGDIOBJ(dib.0 as *mut _));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        // BGRA -> RGBA.
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        // If DrawIconEx produced fully-zero alpha (some legacy icons), recover
        // opacity from luminance so the icon at least shows.
        let any_alpha = buf.chunks_exact(4).any(|px| px[3] != 0);
        if !any_alpha {
            for px in buf.chunks_exact_mut(4) {
                let lum_present = px[0] != 0 || px[1] != 0 || px[2] != 0;
                px[3] = if lum_present { 255 } else { 0 };
            }
        }

        let img = image::RgbaImage::from_raw(size, size, buf)
            .ok_or_else(|| anyhow!("failed to construct image"))?;

        let mut out = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut out),
            image::ImageFormat::Png,
        )?;
        Ok(out)
    }
}
