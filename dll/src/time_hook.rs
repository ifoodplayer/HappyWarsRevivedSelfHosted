//! IAT hook for GetSystemTimeAsFileTime, scoped to the game's own exe.
//! Patches only the game's IAT slot (not kernelbase.dll itself), so other
//! modules/processes keep seeing the real time. Year is rewritten to
//! TARGET_YEAR; month/day/hour/min/sec/ms pass through untouched.

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use windows::Win32::{
    Foundation::{FILETIME, SYSTEMTIME},
    System::{
        LibraryLoader::GetModuleHandleW,
        Memory::{VirtualProtect, PAGE_READWRITE, PAGE_PROTECTION_FLAGS},
        Time::FileTimeToSystemTime,
    },
};

static IAT_SLOT:    AtomicUsize = AtomicUsize::new(0);
static ORIG_FN:     AtomicUsize = AtomicUsize::new(0);
static TARGET_YEAR: AtomicI32   = AtomicI32::new(2016);

const DEFAULT_YEAR: i32 = 2016;
const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;

type PfnGstaft = unsafe extern "system" fn(*mut FILETIME);

#[inline]
unsafe fn read_u16(addr: usize) -> u16 { (addr as *const u16).read_unaligned() }
#[inline]
unsafe fn read_u32(addr: usize) -> u32 { (addr as *const u32).read_unaligned() }
#[inline]
unsafe fn read_u64(addr: usize) -> u64 { (addr as *const u64).read_unaligned() }

unsafe fn read_cstr(addr: usize) -> String {
    let mut out = Vec::new();
    let mut p = addr as *const u8;
    loop {
        let b = p.read();
        if b == 0 || out.len() > 512 { break; }
        out.push(b);
        p = p.add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Walks the PE import table and returns (iat_slot_addr, current_value)
/// for `target_name`, matched via the INT since the IAT holds addresses,
/// not names.
unsafe fn find_iat_slot(base: usize, target_name: &str) -> Option<(usize, u64)> {
    let e_lfanew = read_u32(base + 0x3C) as usize;
    let nt = base + e_lfanew;
    if read_u32(nt) != 0x0000_4550 { return None; }

    let opt_header = nt + 4 + 20;
    if read_u16(opt_header) != 0x20B { return None; } // x64 only

    let data_dir = opt_header + 112;
    let import_entry = data_dir + IMAGE_DIRECTORY_ENTRY_IMPORT * 8;
    let import_rva = read_u32(import_entry) as usize;
    if import_rva == 0 { return None; }

    let mut descriptor = base + import_rva;
    loop {
        let orig_first_thunk = read_u32(descriptor) as usize;
        let first_thunk      = read_u32(descriptor + 16) as usize;
        if orig_first_thunk == 0 && first_thunk == 0 { break; }

        if orig_first_thunk != 0 {
            let mut i = 0usize;
            loop {
                let int_entry = read_u64(base + orig_first_thunk + i * 8);
                if int_entry == 0 { break; }
                if int_entry & 0x8000_0000_0000_0000 == 0 {
                    let name_addr = base + int_entry as usize + 2;
                    if read_cstr(name_addr) == target_name {
                        let slot_addr = base + first_thunk + i * 8;
                        return Some((slot_addr, read_u64(slot_addr)));
                    }
                }
                i += 1;
            }
        }
        descriptor += 20;
    }
    None
}

fn fields_to_filetime(year: i32, month: i32, day: i32,
                      hour: i32, min: i32, sec: i32, ms: i32) -> u64 {
    let (mut y, mut m) = (year, month);
    if m <= 2 { y -= 1; m += 9; } else { m -= 3; }
    let era: i64 = if y >= 0 { y as i64 } else { (y - 399) as i64 } / 400;
    let yoe = (y as i64 - era * 400) as i32;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146_097 + doe as i64 - 719_468 + 134_774) as u64;
    days            * 864_000_000_000
        + hour as u64 *  36_000_000_000
        + min  as u64 *     600_000_000
        + sec  as u64 *      10_000_000
        + ms   as u64 *          10_000
}

unsafe extern "system" fn hook_gstaft(lpft: *mut FILETIME) {
    let orig = ORIG_FN.load(Ordering::Acquire);
    if orig == 0 { return; }
    let orig_fn: PfnGstaft = std::mem::transmute(orig as *const ());
    orig_fn(lpft);

    let target_year = TARGET_YEAR.load(Ordering::Relaxed);

    let mut st = SYSTEMTIME::default();
    if FileTimeToSystemTime(lpft as _, &mut st).is_err() { return; }
    if st.wYear as i32 == target_year { return; }

    let is_leap = |y: i32| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let (month, day) = if st.wMonth == 2 && st.wDay == 29 && !is_leap(target_year) {
        (3, 1)
    } else {
        (st.wMonth as i32, st.wDay as i32)
    };

    let ticks = fields_to_filetime(
        target_year, month, day,
        st.wHour as i32, st.wMinute as i32, st.wSecond as i32,
        st.wMilliseconds as i32,
    );
    (*lpft).dwLowDateTime  = (ticks & 0xFFFF_FFFF) as u32;
    (*lpft).dwHighDateTime = (ticks >> 32) as u32;
}

/// Idempotent — safe to call repeatedly. Returns false if the import
/// isn't resolvable yet (caller retries).
pub fn install() -> bool {
    TARGET_YEAR.store(DEFAULT_YEAR, Ordering::Relaxed);

    unsafe {
        let base = match GetModuleHandleW(None) {
            Ok(h) => h.0 as usize,
            Err(_) => return false,
        };

        let (slot_addr, orig_ptr) = match find_iat_slot(base, "GetSystemTimeAsFileTime") {
            Some(v) => v,
            None => return false,
        };

        let hook_addr = hook_gstaft as *const () as usize as u64;
        if orig_ptr == hook_addr {
            IAT_SLOT.store(slot_addr, Ordering::Release);
            return true;
        }

        let mut old = PAGE_PROTECTION_FLAGS::default();
        if VirtualProtect(slot_addr as _, 8, PAGE_READWRITE, &mut old).is_err() {
            return false;
        }
        (slot_addr as *mut u64).write_unaligned(hook_addr);
        let _ = VirtualProtect(slot_addr as _, 8, old, &mut old);

        IAT_SLOT.store(slot_addr, Ordering::Release);
        ORIG_FN.store(orig_ptr as usize, Ordering::Release);
        true
    }
}

/// Safety net for install(), which already runs synchronously from
/// DllMain. Wakes up every 2s; retries a few times if the import wasn't
/// resolvable yet, then re-checks the slot forever in case it gets
/// overwritten.
pub fn watchdog() {
    let mut retries_left = 5u32;

    loop {
        let slot = IAT_SLOT.load(Ordering::Acquire);
        let ok = slot != 0 && unsafe { (slot as *const u64).read_unaligned() } == hook_gstaft as *const () as usize as u64;

        if !ok {
            if retries_left == 0 { return; }
            retries_left -= 1;
            install();
        }

        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

pub fn remove() {
    unsafe {
        let slot_addr = IAT_SLOT.load(Ordering::Acquire);
        let orig_ptr  = ORIG_FN.load(Ordering::Acquire);
        if slot_addr == 0 || orig_ptr == 0 { return; }

        let mut old = PAGE_PROTECTION_FLAGS::default();
        if VirtualProtect(slot_addr as _, 8, PAGE_READWRITE, &mut old).is_err() {
            return;
        }
        (slot_addr as *mut u64).write_unaligned(orig_ptr as u64);
        let _ = VirtualProtect(slot_addr as _, 8, old, &mut old);

        IAT_SLOT.store(0, Ordering::Release);
        ORIG_FN.store(0, Ordering::Release);
    }
}