use serde::Serialize;
use std::cell::Cell;
use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AudioState {
    pub volume_percent: u8,
    pub muted: bool,
    pub has_device: bool,
}

fn ensure_com() {
    thread_local! { static INITED: Cell<bool> = const { Cell::new(false) }; }
    INITED.with(|i| {
        if !i.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            i.set(true);
        }
    });
}

unsafe fn endpoint() -> Option<IAudioEndpointVolume> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
    device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
}

pub fn current() -> AudioState {
    ensure_com();
    unsafe {
        let Some(ep) = endpoint() else { return AudioState::default() };
        let volume = ep.GetMasterVolumeLevelScalar().unwrap_or(0.0);
        let muted = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        AudioState {
            volume_percent: (volume.clamp(0.0, 1.0) * 100.0).round() as u8,
            muted,
            has_device: true,
        }
    }
}

pub fn set_volume(percent: u8) -> Result<(), String> {
    ensure_com();
    let v = (percent.min(100) as f32) / 100.0;
    unsafe {
        let ep = endpoint().ok_or_else(|| "no audio device".to_string())?;
        ep.SetMasterVolumeLevelScalar(v, std::ptr::null::<GUID>())
            .map_err(|e| e.to_string())
    }
}

pub fn set_mute(muted: bool) -> Result<(), String> {
    ensure_com();
    unsafe {
        let ep = endpoint().ok_or_else(|| "no audio device".to_string())?;
        ep.SetMute(muted, std::ptr::null::<GUID>()).map_err(|e| e.to_string())
    }
}
