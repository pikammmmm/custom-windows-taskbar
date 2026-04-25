use anyhow::{Result, anyhow};
use sha2::{Sha256, Digest};
use std::path::PathBuf;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO, HICON};
use windows::Win32::Graphics::Gdi::{
    GetDIBits, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, BI_RGB,
    GetDC, ReleaseDC, GetObjectW, BITMAP, DeleteObject,
};
use windows::core::PCWSTR;
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
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)))
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
    if result == 0 || info.hIcon.0 == std::ptr::null_mut() {
        return Err(anyhow!("SHGetFileInfoW returned no icon for {exe_path}"));
    }
    let png = hicon_to_png(info.hIcon);
    unsafe { let _ = DestroyIcon(info.hIcon); }
    png
}

fn hicon_to_png(hicon: HICON) -> Result<Vec<u8>> {
    unsafe {
        let mut icon_info = ICONINFO::default();
        GetIconInfo(hicon, &mut icon_info)?;

        let mut bm = BITMAP::default();
        let obj_size = GetObjectW(icon_info.hbmColor, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        if obj_size == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            if !icon_info.hbmColor.is_invalid() { let _ = DeleteObject(icon_info.hbmColor); }
            if !icon_info.hbmMask.is_invalid()  { let _ = DeleteObject(icon_info.hbmMask);  }
            return Err(anyhow!("GetObjectW failed or icon has zero dimensions"));
        }
        let w = bm.bmWidth as u32;
        let h = bm.bmHeight as u32;

        let hdc = GetDC(None);
        if hdc.is_invalid() {
            if !icon_info.hbmColor.is_invalid() { let _ = DeleteObject(icon_info.hbmColor); }
            if !icon_info.hbmMask.is_invalid()  { let _ = DeleteObject(icon_info.hbmMask);  }
            return Err(anyhow!("GetDC returned null"));
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let scanlines = GetDIBits(hdc, icon_info.hbmColor, 0, h, Some(buf.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, hdc);

        // Now safe to clean up the bitmaps.
        if !icon_info.hbmColor.is_invalid() { let _ = DeleteObject(icon_info.hbmColor); }
        if !icon_info.hbmMask.is_invalid()  { let _ = DeleteObject(icon_info.hbmMask);  }

        if scanlines == 0 {
            return Err(anyhow!("GetDIBits failed (no scanlines copied)"));
        }

        // BGRA -> RGBA
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        let img = image::RgbaImage::from_raw(w, h, buf)
            .ok_or_else(|| anyhow!("failed to construct image"))?;

        let resized = image::imageops::resize(&img, ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3);

        let mut out = Vec::new();
        resized.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
        Ok(out)
    }
}
