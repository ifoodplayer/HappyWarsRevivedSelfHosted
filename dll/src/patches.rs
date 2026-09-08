//! Memory patches applied to fixed RVAs in main exe

use windows::Win32::{
    System::{
        Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS},
        Threading::GetCurrentProcess,
        LibraryLoader::GetModuleHandleW,
        Diagnostics::Debug::FlushInstructionCache,
    },
};

const RVA_ACCESS_FLAG:      usize = 0x0009_F195;
const RVA_SET_ACCESS:       usize = 0x00DD_1191;

// GameServers
const RVA_HWAPI_STRING:     usize = 0x011B_17F8;
const RVA_TELEMETRY_URL:    usize = 0x011F_C9B8;

// Happy Tickets
const RVA_TICKET_ENABLED:   usize = 0x00AC_BCF0;
const RVA_FLAG_TICKET_INIT: usize = 0x0162_4D8C;
const RVA_FLAG_TICKET_SLOT: usize = 0x0162_4D8D;
const RVA_FLAG_LICENSE_OK:  usize = 0x038F_66BB;

const HWAPI_URL:     &[u8] = b"http://127.0.0.1:8354/v12/api\0";
const TELEMETRY_URL: &[u8] = b"http://127.0.0.1:8354\0";

/// Writes `bytes` to `addr`, temporarily lifting memory protection.
fn patch_bytes(addr: *mut u8, bytes: &[u8]) -> bool {
    if addr.is_null() || bytes.is_empty() { return false; }
    let len = bytes.len();
    let mut old = PAGE_PROTECTION_FLAGS::default();
    unsafe {
        if VirtualProtect(addr as _, len, PAGE_EXECUTE_READWRITE, &mut old).is_err() {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr, len);
        let _ = VirtualProtect(addr as _, len, old, &mut old);
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(addr as _), len);
    }
    true
}

fn patch_safe(base: usize, rva: usize, bytes: &[u8]) -> bool {
    patch_bytes(base.wrapping_add(rva) as *mut u8, bytes)
}

pub fn apply() {
    let base = unsafe {
        match GetModuleHandleW(None) {
            Ok(h) => h.0 as usize,
            Err(_) => return,
        }
    };

    patch_safe(base, RVA_ACCESS_FLAG,      &[0xCC, 0xCC, 0xCC, 0xCC]);
    patch_safe(base, RVA_SET_ACCESS,       &[0xC3]);
    patch_safe(base, RVA_FLAG_TICKET_INIT, &[0x01]);
    patch_safe(base, RVA_FLAG_TICKET_SLOT, &[0x01]);
    patch_safe(base, RVA_FLAG_LICENSE_OK,  &[0x01]);
    patch_safe(base, RVA_TICKET_ENABLED,   &[0xB0, 0x01, 0xC3]);
    patch_safe(base, RVA_HWAPI_STRING,     HWAPI_URL);
    patch_safe(base, RVA_TELEMETRY_URL,    TELEMETRY_URL);
}