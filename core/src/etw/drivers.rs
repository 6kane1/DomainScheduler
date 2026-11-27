//! Kernel driver load addresses (C# `Drivers()` / `DriverOf` of EtwIrq.cs and BlackBox.cs, the same code in both).

/// (load address, base name) of every loaded kernel driver (EnumDeviceDrivers); empty when Windows refuses.
#[cfg(windows)]
pub fn drivers() -> Vec<(u64, String)> {
    use windows_sys::Win32::System::ProcessStatus::{EnumDeviceDrivers, GetDeviceDriverBaseNameW};
    let mut list = Vec::new();
    let mut arr: Vec<*mut core::ffi::c_void> = vec![std::ptr::null_mut(); 4096];
    let mut needed: u32 = 0;
    let ptr_size = std::mem::size_of::<usize>() as u32;
    // SAFETY: arr has room for 4096 pointers.
    if unsafe { EnumDeviceDrivers(arr.as_mut_ptr(), arr.len() as u32 * ptr_size, &mut needed) } == 0 {
        return list;
    }
    let n = ((needed / ptr_size) as usize).min(arr.len());
    let mut buf = [0u16; 260];
    for a in arr.iter().take(n) {
        if a.is_null() {
            continue;
        }
        // SAFETY: buf holds 260 chars.
        let len = unsafe { GetDeviceDriverBaseNameW(*a, buf.as_mut_ptr(), buf.len() as u32) };
        list.push((*a as u64, if len > 0 { String::from_utf16_lossy(&buf[..len as usize]) } else { "?".to_string() }));
    }
    list
}

#[cfg(not(windows))]
pub fn drivers() -> Vec<(u64, String)> {
    Vec::new()
}

/// The driver an address belongs to: the highest load address at or below it, "?" when none or more than 128 MB away.
pub fn driver_of(list: &[(u64, String)], addr: u64) -> String {
    let mut best = "?";
    let mut best_base = 0u64;
    for (b, n) in list {
        if *b <= addr && *b >= best_base {
            best_base = *b;
            best = n;
        }
    }
    if addr.wrapping_sub(best_base) > 0x8000000 { "?".to_string() } else { best.to_string() }
}
