use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::OnceLock;
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HANDLE, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    ExtractIconExW, IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName,
    SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES,
    SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT,
};
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
    // Best path: IShellItemImageFactory. Same API File Explorer uses; handles
    // UWP package manifests, MSIX, traditional exes, and shell-shortcut
    // chains uniformly. Falls back through ExtractIconExW (PE resources) and
    // SHGetFileInfo (shell associations) for the rare cases this fails.
    if let Ok(png) = extract_via_shell_factory(exe_path) {
        if !looks_generic(&png) { return Ok(png); }
    }

    let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();

    let mut large = HICON(std::ptr::null_mut());
    let count = unsafe {
        ExtractIconExW(PCWSTR(wide.as_ptr()), 0, Some(&mut large), None, 1)
    };
    if count > 0 && !large.0.is_null() {
        let png = hicon_to_png(large, ICON_SIZE);
        unsafe { let _ = DestroyIcon(large); }
        if let Ok(data) = png {
            if !looks_generic(&data) { return Ok(data); }
        }
    }

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
        return Err(anyhow!("no icon for {exe_path}"));
    }
    let png = hicon_to_png(info.hIcon, ICON_SIZE);
    unsafe { let _ = DestroyIcon(info.hIcon); }
    let data = png?;
    if looks_generic(&data) {
        return Err(anyhow!("only generic icon available for {exe_path}"));
    }
    Ok(data)
}

fn ensure_com() {
    thread_local! { static INITED: Cell<bool> = const { Cell::new(false) }; }
    INITED.with(|i| {
        if !i.get() {
            unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
            i.set(true);
        }
    });
}

fn extract_via_shell_factory(exe_path: &str) -> Result<Vec<u8>> {
    ensure_com();
    let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
        let factory: IShellItemImageFactory = item.cast()?;
        let size = SIZE { cx: ICON_SIZE as i32, cy: ICON_SIZE as i32 };
        let hbitmap = factory.GetImage(size, SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK)?;
        let png = hbitmap_to_png(hbitmap, ICON_SIZE, ICON_SIZE);
        let _ = DeleteObject(HGDIOBJ(hbitmap.0 as *mut _));
        png
    }
}

fn hbitmap_to_png(hbitmap: HBITMAP, w: u32, h: u32) -> Result<Vec<u8>> {
    unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return Err(anyhow!("GetDC null"));
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
        let lines = GetDIBits(
            hdc,
            hbitmap,
            0,
            h,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, hdc);
        if lines == 0 {
            return Err(anyhow!("GetDIBits failed"));
        }
        for px in buf.chunks_exact_mut(4) { px.swap(0, 2); }
        let any_alpha = buf.chunks_exact(4).any(|px| px[3] != 0);
        if !any_alpha {
            for px in buf.chunks_exact_mut(4) {
                let lum = px[0] != 0 || px[1] != 0 || px[2] != 0;
                px[3] = if lum { 255 } else { 0 };
            }
        }
        let img = image::RgbaImage::from_raw(w, h, buf)
            .ok_or_else(|| anyhow!("from_raw failed"))?;
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
        Ok(out)
    }
}

/// Detect the system's stock "unknown application" PNG by hash. We probe it
/// once at first call by asking SHGetFileInfo for the icon of an exe path
/// that intentionally doesn't exist (with SHGFI_USEFILEATTRIBUTES so the
/// shell returns its placeholder instead of failing). Any extracted icon
/// matching that exact hash is the generic placeholder, regardless of size.
fn looks_generic(png: &[u8]) -> bool {
    let probe = generic_icon_hash();
    let zero = [0u8; 32];
    *probe != zero && sha_of(png) == *probe
}

fn sha_of(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

fn generic_icon_hash() -> &'static [u8; 32] {
    static H: OnceLock<[u8; 32]> = OnceLock::new();
    H.get_or_init(|| probe_generic_icon().map(|p| sha_of(&p)).unwrap_or([0u8; 32]))
}

fn probe_generic_icon() -> Option<Vec<u8>> {
    let probe = "C:\\__glassbar_probe_does_not_exist__.exe";
    let wide: Vec<u16> = probe.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0x80), // FILE_ATTRIBUTE_NORMAL
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if result == 0 || info.hIcon.0.is_null() {
        return None;
    }
    let png = hicon_to_png(info.hIcon, ICON_SIZE).ok();
    unsafe { let _ = DestroyIcon(info.hIcon); }
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
