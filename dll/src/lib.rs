#![allow(non_snake_case, non_upper_case_globals, linker_messages)]

mod patches;
mod time_hook;
mod proxy;

use std::sync::atomic::{AtomicBool, Ordering};

use windows::{
    core::PCSTR,
    Win32::{
        Foundation::{BOOL, HMODULE, TRUE},
        System::{
            LibraryLoader::{DisableThreadLibraryCalls, GetProcAddress, LoadLibraryW},
            SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH},
        },
    },
};

static PATCHED:  AtomicBool  = AtomicBool::new(false);

static mut G_ORIG: HMODULE = HMODULE(std::ptr::null_mut() as _);

/// Loads and caches the real system d3d11.dll.
pub(crate) fn get_orig() -> HMODULE {
    unsafe {
        let current = std::ptr::addr_of!(G_ORIG).read();
        if current.is_invalid() {
            let mut path = [0u16; 260];
            windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut path));
            let suffix: Vec<u16> = "\\d3d11.dll\0".encode_utf16().collect();
            let base_len = path.iter().position(|&c| c == 0).unwrap_or(0);
            path[base_len..base_len + suffix.len()].copy_from_slice(&suffix);
            if let Ok(h) = LoadLibraryW(windows::core::PCWSTR(path.as_ptr())) {
                std::ptr::addr_of_mut!(G_ORIG).write(h);
                return h;
            }
            return current;
        }
        current
    }
}

/// Returns a raw proc address from the real d3d11.dll.
pub(crate) fn get_proc_raw(name: &[u8]) -> Option<unsafe extern "system" fn()> {
    let h = get_orig();
    if h.is_invalid() { return None; }
    unsafe { GetProcAddress(h, PCSTR(name.as_ptr())).map(|f| std::mem::transmute(f)) }
}

/// Applies memory patches exactly once using an atomic guard.
pub(crate) fn ensure_patched() {
    if PATCHED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        patches::apply();
    }
}

#[no_mangle]
pub extern "system" fn DllMain(hModule: HMODULE, reason: u32, _reserved: *mut ()) -> BOOL {
    match reason {
        DLL_PROCESS_ATTACH => {
            unsafe { DisableThreadLibraryCalls(hModule).ok(); }
            get_orig();
            // Hook synchronously so it's live before TRUE returns; watchdog is just the safety net.
            time_hook::install();
            std::thread::spawn(|| {
                time_hook::watchdog();
            });
        }
        DLL_PROCESS_DETACH => {
            time_hook::remove();
        }
        _ => {}
    }
    TRUE
}