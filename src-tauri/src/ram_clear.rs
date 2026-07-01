//! Cached-RAM purge — the RAMMap / ISLC "Empty Standby List" operation.
//!
//! Windows parks evicted-but-maybe-useful file pages on the *standby list*:
//! they count as "available" but still occupy physical frames, and a huge
//! standby list can cause allocation-latency hiccups under memory pressure.
//! Purging it is the legitimate cached-RAM clear (RAMMap → Empty → Standby
//! List; ISLC's entire reason to exist), NOT the cosmetic working-set-only
//! trick — though we also trim working sets first so those pages get swept
//! into the purge too. The OS entry point is the undocumented
//! `NtSetSystemInformation(SystemMemoryListInformation = 0x50)`, which
//! requires the caller to be elevated AND hold
//! `SeProfileSingleProcessPrivilege`.
//!
//! glassbar itself deliberately runs non-elevated (an always-on elevated
//! shell would leak admin rights into everything it launches), so the purge
//! runs in a short-lived elevated relaunch of our own exe:
//! `glassbar.exe --clear-ram <result-file>`. main() dispatches that flag
//! BEFORE the singleton check — the resident instance is alive and holding
//! the mutex while the helper runs. The helper purges, writes a JSON result
//! file, and exits; the resident instance waits on the process handle and
//! reads the file back. One UAC prompt per click (install_lhm precedent).

use serde::{Deserialize, Serialize};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_CANCELLED, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID, WAIT_OBJECT_0,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation,
    LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::ProcessStatus::{
    GetPerformanceInfo, K32EmptyWorkingSet, K32EnumProcesses, PERFORMANCE_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, OpenProcessToken, WaitForSingleObject,
    PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA,
};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

const MB: u64 = 1024 * 1024;

// SYSTEM_INFORMATION_CLASS / SYSTEM_MEMORY_LIST_COMMAND values for the
// memory-list interface. Undocumented but stable since Vista — every
// standby-list tool (RAMMap, ISLC, EmptyStandbyList) uses these numbers.
const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 0x50;
const MEMORY_FLUSH_MODIFIED_LIST: i32 = 3;
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

type NtSetSystemInformationFn = unsafe extern "system" fn(
    system_information_class: i32,
    system_information: *mut core::ffi::c_void,
    system_information_length: u32,
) -> i32; // NTSTATUS

/// Wire format for both the helper's result file and the Tauri command's
/// return value. `status` is "done" or "cancelled" on the command surface;
/// "error" only ever appears inside the result file — the resident side
/// converts it into a rejected promise so the frontend has one error path.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ClearRamOutcome {
    pub status: String,
    /// Headline number: how much the system cache (standby list + system
    /// working set) shrank across the purge.
    pub freed_mb: u64,
    /// Change in available physical memory. Usually small for a pure
    /// standby purge (standby pages already count as available) — this
    /// mostly reflects the working-set trims.
    pub avail_delta_mb: i64,
    pub cache_before_mb: u64,
    pub cache_after_mb: u64,
    /// How many processes accepted the EmptyWorkingSet call. Partial
    /// coverage is expected — protected/system processes refuse the
    /// PROCESS_SET_QUOTA open and are skipped by design.
    pub trimmed_processes: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Entry point for the resident (non-elevated) instance. Blocking — call
/// from `spawn_blocking`, not the main thread: ShellExecuteExW doesn't
/// return until the user answers the UAC consent dialog.
pub fn clear_cached_ram_blocking() -> Result<ClearRamOutcome, String> {
    if is_elevated() {
        // Someone launched glassbar itself as admin — no helper needed.
        crate::glog!("ram_clear: already elevated, purging in-process");
        return perform_clear();
    }
    run_elevated_helper()
}

/// Entry point for the elevated `--clear-ram` helper relaunch (dispatched
/// from main() before the singleton check). Returns the process exit code.
pub fn run_helper(args: &[String]) -> i32 {
    let result_path = args
        .iter()
        .position(|a| a == "--clear-ram")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_result_path);
    let (outcome, code) = match perform_clear() {
        Ok(outcome) => (outcome, 0),
        Err(e) => {
            crate::glog!("ram_clear helper failed: {e}");
            let outcome = ClearRamOutcome {
                status: "error".into(),
                error: Some(e),
                ..Default::default()
            };
            (outcome, 1)
        }
    };
    let json = serde_json::to_string(&outcome).unwrap_or_else(|_| "{}".into());
    if std::fs::write(&result_path, json).is_err() {
        return 2;
    }
    code
}

/// The actual clear, in the order that maximises what the purge releases:
/// trim working sets first (trimmed pages land on the modified/standby
/// lists), flush the modified list (dirty pages have to hit disk before
/// they can leave), then purge the standby list.
fn perform_clear() -> Result<ClearRamOutcome, String> {
    let (cache_before, avail_before) = read_mem()?;

    // Purging the standby list requires SeProfileSingleProcessPrivilege —
    // present in an admin token but disabled by default. Hard requirement:
    // without it NtSetSystemInformation returns STATUS_PRIVILEGE_NOT_HELD.
    enable_privilege(w!("SeProfileSingleProcessPrivilege"))
        .map_err(|e| format!("SeProfileSingleProcessPrivilege: {e} (needs an elevated token)"))?;
    // SeDebugPrivilege widens OpenProcess reach for the working-set sweep.
    // Nice-to-have only — log and carry on without it.
    if let Err(e) = enable_privilege(w!("SeDebugPrivilege")) {
        crate::glog!(
            "ram_clear: SeDebugPrivilege unavailable ({e}) — sweep skips protected processes"
        );
    }

    let trimmed = trim_working_sets();

    if let Err(e) = nt_memory_command(MEMORY_FLUSH_MODIFIED_LIST) {
        // A failed flush just means dirty pages stay put — the standby
        // purge below is still worth doing, so don't abort on this one.
        crate::glog!("ram_clear: flush modified list failed ({e}) — continuing to purge");
    }
    nt_memory_command(MEMORY_PURGE_STANDBY_LIST)?;

    let (cache_after, avail_after) = read_mem()?;
    let outcome = ClearRamOutcome {
        status: "done".into(),
        freed_mb: cache_before.saturating_sub(cache_after) / MB,
        avail_delta_mb: (avail_after as i64 - avail_before as i64) / MB as i64,
        cache_before_mb: cache_before / MB,
        cache_after_mb: cache_after / MB,
        trimmed_processes: trimmed,
        error: None,
    };
    crate::glog!(
        "ram_clear: cache {} MB -> {} MB (freed {} MB), avail delta {} MB, {} working sets trimmed",
        outcome.cache_before_mb,
        outcome.cache_after_mb,
        outcome.freed_mb,
        outcome.avail_delta_mb,
        outcome.trimmed_processes
    );
    Ok(outcome)
}

/// Relaunch our own exe elevated with the hidden `--clear-ram` flag, wait
/// for it to finish, and read its JSON result file back. The result path
/// travels as an argument so both sides agree even if UAC runs the helper
/// as a different admin account (whose %TEMP% differs from ours).
fn run_elevated_helper() -> Result<ClearRamOutcome, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
    let result_path = default_result_path();
    // Stale file from a previous run must not masquerade as this run's result.
    let _ = std::fs::remove_file(&result_path);

    let file_w: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params = format!("--clear-ram \"{}\"", result_path.display());
    let params_w: Vec<u16> = params.encode_utf16().chain(std::iter::once(0)).collect();

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // NOCLOSEPROCESS so we get a waitable handle back; NOASYNC because
        // we're on a short-lived blocking thread; FLAG_NO_UI so a failure
        // surfaces as our own error string instead of a shell dialog.
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: PCWSTR(params_w.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // ShellExecuteExW blocks while the UAC consent dialog is up. A decline
    // comes back as ERROR_CANCELLED — the user's choice, not a failure.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        if e.code() == ERROR_CANCELLED.to_hresult() {
            crate::glog!("ram_clear: UAC declined");
            return Ok(ClearRamOutcome {
                status: "cancelled".into(),
                ..Default::default()
            });
        }
        return Err(format!("elevated relaunch failed: {e}"));
    }
    if info.hProcess.is_invalid() {
        return Err("elevated helper started but returned no process handle".into());
    }
    let wait = unsafe { WaitForSingleObject(info.hProcess, 60_000) };
    let mut code: u32 = 0;
    let _ = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
    let _ = unsafe { CloseHandle(info.hProcess) };
    if wait != WAIT_OBJECT_0 {
        return Err("ram-clear helper timed out after 60s".into());
    }

    let raw = std::fs::read_to_string(&result_path)
        .map_err(|e| format!("helper exited (code {code}) but left no result file: {e}"))?;
    let _ = std::fs::remove_file(&result_path);
    let outcome: ClearRamOutcome =
        serde_json::from_str(&raw).map_err(|e| format!("bad helper result file: {e}"))?;
    if outcome.status == "error" {
        return Err(outcome
            .error
            .unwrap_or_else(|| format!("helper failed (exit code {code})")));
    }
    Ok(outcome)
}

fn default_result_path() -> std::path::PathBuf {
    std::env::temp_dir().join("glassbar_ram_clear_result.json")
}

/// (system cache bytes, available physical bytes) from GetPerformanceInfo.
/// SystemCache = standby list + system working set — the number that drops
/// when the purge works; PhysicalAvailable = free + standby + zeroed, which
/// barely moves on a purge (standby already counts as available) but does
/// reflect the working-set trims.
fn read_mem() -> Result<(u64, u64), String> {
    let mut pi = PERFORMANCE_INFORMATION {
        cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
        ..Default::default()
    };
    unsafe {
        GetPerformanceInfo(&mut pi, pi.cb)
            .map_err(|e| format!("GetPerformanceInfo failed: {e}"))?;
    }
    let page = pi.PageSize as u64;
    Ok((
        pi.SystemCache as u64 * page,
        pi.PhysicalAvailable as u64 * page,
    ))
}

/// Enable a named privilege on the current process token. Note the
/// AdjustTokenPrivileges quirk: it returns success even when it assigned
/// nothing — the real verdict is GetLastError() == ERROR_NOT_ALL_ASSIGNED.
fn enable_privilege(name: PCWSTR) -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .map_err(|e| format!("OpenProcessToken failed: {e}"))?;

        let mut luid = LUID::default();
        if let Err(e) = LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) {
            let _ = CloseHandle(token);
            return Err(format!("LookupPrivilegeValue failed: {e}"));
        }
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let adjusted = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        let last = GetLastError();
        let _ = CloseHandle(token);
        adjusted.map_err(|e| format!("AdjustTokenPrivileges failed: {e}"))?;
        if last == ERROR_NOT_ALL_ASSIGNED {
            return Err("privilege not held by token".into());
        }
        Ok(())
    }
}

/// EmptyWorkingSet across every process we can open. Access failures are
/// expected and fine (protected processes, other sessions) — a partial
/// sweep is the design, never a reason to fail the whole operation.
fn trim_working_sets() -> u32 {
    unsafe {
        let mut pids = vec![0u32; 4096];
        let mut needed = 0u32;
        let cb = (pids.len() * size_of::<u32>()) as u32;
        if !K32EnumProcesses(pids.as_mut_ptr(), cb, &mut needed).as_bool() {
            crate::glog!("ram_clear: EnumProcesses failed — skipping working-set sweep");
            return 0;
        }
        let count = (needed as usize / size_of::<u32>()).min(pids.len());
        let mut trimmed = 0u32;
        for &pid in &pids[..count] {
            if pid == 0 {
                continue;
            }
            let Ok(h) = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_SET_QUOTA, false, pid)
            else {
                continue;
            };
            if K32EmptyWorkingSet(h).as_bool() {
                trimmed += 1;
            }
            let _ = CloseHandle(h);
        }
        trimmed
    }
}

/// Issue one SYSTEM_MEMORY_LIST_COMMAND via NtSetSystemInformation. The
/// function has no import-library entry, so it's resolved from the
/// already-mapped ntdll at call time (GetModuleHandle, not LoadLibrary).
fn nt_memory_command(command: i32) -> Result<(), String> {
    unsafe {
        let ntdll = GetModuleHandleW(w!("ntdll.dll")).map_err(|e| format!("ntdll.dll: {e}"))?;
        let addr = GetProcAddress(ntdll, s!("NtSetSystemInformation"))
            .ok_or_else(|| "NtSetSystemInformation not found in ntdll.dll".to_string())?;
        let nt_set: NtSetSystemInformationFn = std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            NtSetSystemInformationFn,
        >(addr);
        let mut cmd = command;
        let status = nt_set(
            SYSTEM_MEMORY_LIST_INFORMATION,
            (&mut cmd as *mut i32).cast(),
            size_of::<i32>() as u32,
        );
        if status != 0 {
            return Err(format!(
                "NtSetSystemInformation(SystemMemoryListInformation, {command}) -> NTSTATUS 0x{:08X}",
                status as u32
            ));
        }
        Ok(())
    }
}

/// Whether the current process token is elevated (full admin token, not
/// the UAC-filtered one).
fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}
