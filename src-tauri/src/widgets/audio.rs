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

unsafe fn endpoint_for_role(role: ERole) -> Option<IAudioEndpointVolume> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, role).ok()?;
    device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
}

unsafe fn endpoint() -> Option<IAudioEndpointVolume> {
    // eConsole is the role the Windows tray slider controls — that's the
    // canonical "system volume" the user sees and expects to match. Earlier
    // builds used eMultimedia, but on systems where the user has set
    // different defaults per role (USB headset configured differently from
    // speakers, gaming headsets that split chat/game audio, etc.) that
    // didn't match the tray and the HUD looked wrong. Fall back to
    // eMultimedia → eCommunications if eConsole isn't usable.
    if let Some(ep) = endpoint_for_role(eConsole) { return Some(ep); }
    if let Some(ep) = endpoint_for_role(eMultimedia) { return Some(ep); }
    endpoint_for_role(eCommunications)
}

pub fn current() -> AudioState {
    ensure_com();
    unsafe {
        let Some(ep) = endpoint() else {
            crate::glog!("audio::current: no endpoint");
            return AudioState::default();
        };
        let muted = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        // Use scalar * 100 — same value we WRITE in set_volume, so the
        // round-trip is consistent. GetVolumeStepInfo was tried but caused
        // a 75 → 74 ping-pong: we'd write scalar 0.75, the endpoint would
        // snap it to step 37/50 = 74, the snapshot would emit 74, and the
        // HUD slider would jump back to 74 the moment the user-intent
        // window expired. Scalar in / scalar out avoids the mismatch.
        let volume = ep.GetMasterVolumeLevelScalar().unwrap_or(0.0);
        let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
        AudioState {
            volume_percent: percent,
            muted,
            has_device: true,
        }
    }
}

/// Log the volume-scalar value reported by every endpoint role. Diagnostic
/// only — invoked by the get_audio_diagnostics command so the user can
/// share a snapshot when the HUD's percentage looks wrong, and we can tell
/// at a glance whether the discrepancy is on our side or whether the user
/// has actually configured per-role differences in Sound Settings.
pub fn log_endpoint_diagnostics() {
    ensure_com();
    unsafe {
        for (label, role) in [
            ("eConsole", eConsole),
            ("eMultimedia", eMultimedia),
            ("eCommunications", eCommunications),
        ] {
            match endpoint_for_role(role) {
                Some(ep) => {
                    let v = ep.GetMasterVolumeLevelScalar().unwrap_or(-1.0);
                    let m = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                    let pct = if v < 0.0 { -1 } else { (v.clamp(0.0, 1.0) * 100.0).round() as i32 };
                    crate::glog!("audio[{label}]: scalar={:.4} percent={pct} muted={m}", v);
                }
                None => crate::glog!("audio[{label}]: no endpoint"),
            }
        }
    }
}

/// Set the system master volume. Returns the percentage Windows actually
/// committed — endpoints may snap to discrete steps so the requested
/// percent and the resulting percent can differ by 1-2. Callers use the
/// returned value to keep their UI in sync.
pub fn set_volume(percent: u8) -> Result<u8, String> {
    ensure_com();
    let v = (percent.min(100) as f32) / 100.0;
    unsafe {
        let ep = endpoint().ok_or_else(|| "no audio device".to_string())?;
        ep.SetMasterVolumeLevelScalar(v, std::ptr::null::<GUID>())
            .map_err(|e| e.to_string())?;
        // Read back the actual scalar — the endpoint may have snapped.
        let actual = ep.GetMasterVolumeLevelScalar().unwrap_or(v);
        Ok((actual.clamp(0.0, 1.0) * 100.0).round() as u8)
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
