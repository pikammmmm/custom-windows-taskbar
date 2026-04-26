use serde::Serialize;
use std::cell::Cell;
use windows::core::{Interface, GUID, HRESULT, PCWSTR, PROPVARIANT};
use windows::Win32::Media::Audio::{
    eConsole, eCommunications, eMultimedia, eRender, ERole,
    Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx,
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

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

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

// PKEY_Device_FriendlyName — fmtid {a45c254e-df1c-4efd-8020-67d146a850e0}, pid 14
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

pub fn list_devices() -> Result<Vec<AudioDevice>, String> {
    ensure_com();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let coll = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| e.to_string())?;
        let count = coll.GetCount().map_err(|e| e.to_string())?;

        let default_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()
            .and_then(|d| d.GetId().ok())
            .map(|p| p.to_string().unwrap_or_default())
            .unwrap_or_default();

        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let dev = match coll.Item(i) { Ok(d) => d, Err(_) => continue };
            let id = dev.GetId().ok().and_then(|p| p.to_string().ok()).unwrap_or_default();
            if id.is_empty() { continue; }
            let name = dev
                .OpenPropertyStore(STGM_READ).ok()
                .and_then(|store| store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME).ok())
                .and_then(|v| prop_to_string(&v))
                .unwrap_or_else(|| "Unknown device".to_string());
            let is_default = !default_id.is_empty() && id.eq_ignore_ascii_case(&default_id);
            out.push(AudioDevice { id, name, is_default });
        }
        Ok(out)
    }
}

unsafe fn prop_to_string(v: &PROPVARIANT) -> Option<String> {
    // VT_LPWSTR (31) — most friendly-name properties
    let raw = v.as_raw();
    if raw.Anonymous.Anonymous.vt == 31 {
        let pwsz = raw.Anonymous.Anonymous.Anonymous.pwszVal;
        if pwsz.is_null() { return None; }
        let mut len = 0usize;
        while *pwsz.add(len) != 0 { len += 1; }
        let slice = std::slice::from_raw_parts(pwsz, len);
        Some(String::from_utf16_lossy(slice))
    } else {
        None
    }
}

// CLSID PolicyConfigClient {870AF99C-171D-4F9E-AF0D-E63DF40C2BC9}
const CLSID_POLICY_CONFIG_CLIENT: GUID =
    GUID::from_u128(0x870AF99C_171D_4F9E_AF0D_E63DF40C2BC9);

// IID IPolicyConfig {F8679F50-850A-41CF-9C72-430F290290C8} — undocumented
// but ABI-stable interface in policyconfig.dll used by EarTrumpet/AudioSwitcher.
const IID_POLICY_CONFIG: GUID =
    GUID::from_u128(0xF8679F50_850A_41CF_9C72_430F290290C8);

// SetDefaultEndpoint sits at slot 14 of the COM vtable: 3 IUnknown methods
// (QueryInterface, AddRef, Release) + 11 preceding IPolicyConfig methods
// before it (GetMixFormat … SetPropertyValue), so its index from the start
// of the vtable is 14.
const SET_DEFAULT_ENDPOINT_VTABLE_SLOT: usize = 14;

pub fn set_default_device(id: &str) -> Result<(), String> {
    ensure_com();
    let id_wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // CoCreateInstance returning a raw IUnknown then QueryInterface
        // for IPolicyConfig — bypasses the windows-rs interface! macro
        // (which conflicts with the multiple windows-core versions in our
        // dep tree). We then call SetDefaultEndpoint by hand-walking the
        // vtable: simpler than defining the whole COM interface in safe Rust.
        let unk: windows::core::IUnknown =
            CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
        let mut policy_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        unk.query(&IID_POLICY_CONFIG, &mut policy_ptr)
            .ok().map_err(|e| e.to_string())?;
        if policy_ptr.is_null() { return Err("IPolicyConfig query returned null".into()); }

        // Vtable layout: *policy_ptr is a pointer to an array of fn pointers.
        let vtable_ptr = *(policy_ptr as *mut *const usize);
        let set_default_raw = *vtable_ptr.add(SET_DEFAULT_ENDPOINT_VTABLE_SLOT);
        type SetDefault = unsafe extern "system" fn(
            this: *mut core::ffi::c_void,
            device_id: PCWSTR,
            role: ERole,
        ) -> HRESULT;
        let set_default: SetDefault = std::mem::transmute(set_default_raw);

        // Set as default for all three roles so Console + Communications
        // follow whatever the user picked under Multimedia.
        let mut last_err = HRESULT(0);
        for role in [eConsole, eMultimedia, eCommunications] {
            let hr = set_default(policy_ptr, PCWSTR(id_wide.as_ptr()), role);
            if hr.0 < 0 { last_err = hr; }
        }

        // Release the queried interface — query() did an AddRef.
        let release_raw = *vtable_ptr.add(2);
        type Release = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
        let release: Release = std::mem::transmute(release_raw);
        let _ = release(policy_ptr);

        if last_err.0 < 0 { Err(format!("SetDefaultEndpoint failed: 0x{:08X}", last_err.0)) }
        else { Ok(()) }
    }
}
