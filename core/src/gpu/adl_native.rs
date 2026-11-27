//! Port of `AdlNative` (in AmdOc.cs): the real ADL, atiadlxx.dll of the AMD driver, loaded on first use, functions
//! looked up by name. Every structure is a byte block in the layout of the ADL SDK headers (adl_structures.h); memory
//! the driver allocates through our callback is copied and freed here.

use super::amd_oc::IAdl;

#[cfg(windows)]
pub use imp::AdlNative;

#[cfg(not(windows))]
pub use other::AdlNative;

#[cfg(windows)]
mod imp {
    use super::IAdl;
    use crate::base::l::L;
    use crate::gpu::amd_oc::AmdOc;
    use std::ffi::c_void;
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::{HMODULE, LocalFree};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::System::Memory::{LocalAlloc, LMEM_FIXED};

    type AllocFn = unsafe extern "system" fn(i32) -> *mut c_void;
    type CreateFn = unsafe extern "C" fn(AllocFn, i32, *mut *mut c_void) -> i32;
    type CountFn = unsafe extern "C" fn(*mut c_void, *mut i32) -> i32;
    type InfoFn = unsafe extern "C" fn(*mut c_void, *mut u8, i32) -> i32;
    type Out1Fn = unsafe extern "C" fn(*mut c_void, i32, *mut i32) -> i32;
    type Out2Fn = unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut i32) -> i32;
    type Out3Fn = unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut i32, *mut i32) -> i32;
    type Od8InitFn = unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut i32, *mut *mut c_void) -> i32;
    type Od8CurFn = unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut *mut c_void) -> i32;
    type Od8SetFn = unsafe extern "C" fn(*mut c_void, i32, *mut u8, *mut u8) -> i32;
    type BlockFn = unsafe extern "C" fn(*mut c_void, i32, *mut u8) -> i32;
    type TempFn = unsafe extern "C" fn(*mut c_void, i32, i32, *mut i32) -> i32;

    /// Kept for the life of the process: the driver calls it whenever it hands memory back (Marshal.AllocHGlobal).
    unsafe extern "system" fn alloc(size: i32) -> *mut c_void {
        // SAFETY: LocalAlloc with LMEM_FIXED is what Marshal.AllocHGlobal does.
        unsafe { LocalAlloc(LMEM_FIXED, size.max(1) as usize) as *mut c_void }
    }

    fn free(p: *mut c_void) {
        // SAFETY: p came from `alloc` (Marshal.FreeHGlobal).
        unsafe { LocalFree(p as _) };
    }

    #[derive(Default)]
    struct Inner {
        lib: usize,
        ctx: usize,
    }

    pub struct AdlNative {
        inner: Mutex<Inner>,
    }

    impl Default for AdlNative {
        fn default() -> Self {
            Self::new()
        }
    }

    impl AdlNative {
        pub fn new() -> Self {
            AdlNative { inner: Mutex::new(Inner::default()) }
        }

        fn lib_ctx(&self) -> (usize, usize) {
            let i = crate::base::hooks::lock(&self.inner);
            (i.lib, i.ctx)
        }

        /// `NativeLibrary.TryGetExport(_lib, name)`; None when the library is not loaded or has no such export.
        fn export(lib: usize, name: &str) -> Option<usize> {
            if lib == 0 {
                return None;
            }
            let mut n = name.as_bytes().to_vec();
            n.push(0);
            // SAFETY: lib is a loaded module, the name is NUL-terminated.
            unsafe { GetProcAddress(lib as HMODULE, n.as_ptr()) }.map(|p| p as usize)
        }

        fn fn_<T: Copy>(&self, name: &str) -> Option<(T, *mut c_void)> {
            let (lib, ctx) = self.lib_ctx();
            let p = Self::export(lib, name)?;
            debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
            // SAFETY: T is the function pointer type of this ADL export (see adl_sdk.h).
            Some((unsafe { std::mem::transmute_copy::<usize, T>(&p) }, ctx as *mut c_void))
        }
    }

    impl IAdl for AdlNative {
        fn open(&self, error: &mut String) -> bool {
            *error = String::new();
            let mut i = crate::base::hooks::lock(&self.inner);
            if i.ctx != 0 {
                return true;
            }
            let name = crate::native::win::wide("atiadlxx.dll");
            // SAFETY: a NUL-terminated wide string; the DLL search order is hardened to System32 at start.
            let lib = unsafe { LoadLibraryW(name.as_ptr()) };
            if lib.is_null() {
                *error = L::t("No AMD driver is installed (its interface atiadlxx.dll was not found)").to_string();
                return false;
            }
            i.lib = lib as usize;
            let Some(create) = Self::export(i.lib, "ADL2_Main_Control_Create") else {
                *error = L::t("The AMD driver does not offer the needed functions").to_string();
                return false;
            };
            // SAFETY: ADL2_Main_Control_Create(ADL_MAIN_MALLOC_CALLBACK, int, ADL_CONTEXT_HANDLE*).
            let create: CreateFn = unsafe { std::mem::transmute::<usize, CreateFn>(create) };
            let mut ctx: *mut c_void = std::ptr::null_mut();
            // SAFETY: see above; alloc lives for the whole process.
            let rc = unsafe { create(alloc, 1, &mut ctx) };
            if rc != 0 || ctx.is_null() {
                i.ctx = 0;
                *error = L::t("The AMD driver did not start its interface").to_string();
                return false;
            }
            i.ctx = ctx as usize;
            true
        }

        fn adapter_count(&self, count: &mut i32) -> i32 {
            *count = 0;
            match self.fn_::<CountFn>("ADL2_Adapter_NumberOfAdapters_Get") {
                // SAFETY: ADL call with its documented arguments.
                Some((f, ctx)) => unsafe { f(ctx, count) },
                None => -8,
            }
        }

        fn adapter_info(&self, buffer: &mut [u8]) -> i32 {
            match self.fn_::<InfoFn>("ADL2_Adapter_AdapterInfo_Get") {
                // SAFETY: the buffer has the size passed.
                Some((f, ctx)) => unsafe { f(ctx, buffer.as_mut_ptr(), buffer.len() as i32) },
                None => -8,
            }
        }

        fn active(&self, adapter: i32, status: &mut i32) -> i32 {
            *status = 0;
            match self.fn_::<Out1Fn>("ADL2_Adapter_Active_Get") {
                // SAFETY: ADL call with its documented arguments.
                Some((f, ctx)) => unsafe { f(ctx, adapter, status) },
                None => -8,
            }
        }

        fn overdrive_caps(&self, adapter: i32, supported: &mut i32, enabled: &mut i32, version: &mut i32) -> i32 {
            *supported = 0;
            *enabled = 0;
            *version = 0;
            match self.fn_::<Out3Fn>("ADL2_Overdrive_Caps") {
                // SAFETY: ADL call with its documented arguments.
                Some((f, ctx)) => unsafe { f(ctx, adapter, supported, enabled, version) },
                None => -8,
            }
        }

        fn asic_type(&self, adapter: i32, types: &mut i32, valids: &mut i32) -> i32 {
            *types = 0;
            *valids = 0;
            match self.fn_::<Out2Fn>("ADL2_Adapter_ASICFamilyType_Get") {
                // SAFETY: ADL call with its documented arguments.
                Some((f, ctx)) => unsafe { f(ctx, adapter, types, valids) },
                None => -8,
            }
        }

        fn od8_init(&self, adapter: i32, caps: &mut i32, list: &mut Option<Vec<u8>>) -> i32 {
            *caps = 0;
            *list = None;
            let Some((f, ctx)) = self.fn_::<Od8InitFn>("ADL2_Overdrive8_Init_SettingX2_Get") else { return -8 };
            let mut count = AmdOc::OD8_COUNT;
            let mut p: *mut c_void = std::ptr::null_mut();
            // SAFETY: the driver allocates the list through `alloc`; it is copied and freed below.
            let rc = unsafe { f(ctx, adapter, caps, &mut count, &mut p) };
            let r = if rc != 0 {
                rc
            } else if p.is_null() || !(1..=1024).contains(&count) {
                -1
            } else {
                let n = (count * AmdOc::OD8_SINGLE_SIZE) as usize;
                // SAFETY: the driver handed back count entries of 16 bytes.
                *list = Some(unsafe { std::slice::from_raw_parts(p as *const u8, n) }.to_vec());
                0
            };
            if !p.is_null() {
                free(p);
            }
            r
        }

        fn od8_current(&self, adapter: i32, values: &mut Option<Vec<i32>>) -> i32 {
            *values = None;
            let Some((f, ctx)) = self.fn_::<Od8CurFn>("ADL2_Overdrive8_Current_SettingX2_Get") else { return -8 };
            let mut count = AmdOc::OD8_COUNT;
            let mut p: *mut c_void = std::ptr::null_mut();
            // SAFETY: as in od8_init.
            let rc = unsafe { f(ctx, adapter, &mut count, &mut p) };
            let r = if rc != 0 {
                rc
            } else if p.is_null() || !(1..=1024).contains(&count) {
                -1
            } else {
                // SAFETY: the driver handed back count ints.
                *values = Some(unsafe { std::slice::from_raw_parts(p as *const i32, count as usize) }.to_vec());
                0
            };
            if !p.is_null() {
                free(p);
            }
            r
        }

        fn od8_set(&self, adapter: i32, set: &[u8], current: &mut [u8]) -> i32 {
            let Some((f, ctx)) = self.fn_::<Od8SetFn>("ADL2_Overdrive8_Setting_Set") else { return -8 };
            // Room beyond the count in both blocks, in case the driver fills more than it was asked for.
            let count = crate::gpu::rd(set, 0).unwrap_or(0);
            let room = count.max(AmdOc::OD8_COUNT) as usize;
            let mut s = vec![0u8; set.len().max(4 + room * AmdOc::OD8_SET_ENTRY as usize) + 256];
            s[..set.len()].copy_from_slice(set);
            let mut c = vec![0u8; current.len().max(4 + room * 4) + 256];
            c[..4].copy_from_slice(&count.to_le_bytes());
            // SAFETY: both blocks are larger than the count the driver was given.
            let rc = unsafe { f(ctx, adapter, s.as_mut_ptr(), c.as_mut_ptr()) };
            let n = current.len();
            current.copy_from_slice(&c[..n]);
            rc
        }

        fn odn_caps(&self, adapter: i32, caps: &mut Option<Vec<u8>>, x2: &mut bool) -> i32 {
            let f2 = self.fn_::<BlockFn>("ADL2_OverdriveN_CapabilitiesX2_Get");
            if let Some((f2, ctx)) = f2 {
                if self.fn_::<BlockFn>("ADL2_OverdriveN_SystemClocksX2_Get").is_some() {
                    let mut b = vec![0u8; AmdOc::ODN_CAPS_X2 as usize];
                    *x2 = true;
                    // SAFETY: ADLODNCapabilitiesX2 is 208 bytes.
                    let rc = unsafe { f2(ctx, adapter, b.as_mut_ptr()) };
                    *caps = Some(b);
                    if rc == 0 {
                        return 0;
                    }
                }
            }
            *x2 = false;
            let mut b = vec![0u8; AmdOc::ODN_CAPS as usize];
            let rc = match self.fn_::<BlockFn>("ADL2_OverdriveN_Capabilities_Get") {
                // SAFETY: ADLODNCapabilities is 164 bytes.
                Some((f, ctx)) => unsafe { f(ctx, adapter, b.as_mut_ptr()) },
                None => -8,
            };
            *caps = Some(b);
            rc
        }

        fn odn_clocks(&self, adapter: i32, memory: bool, set: bool, x2: bool, levels: &mut [u8]) -> i32 {
            let name = format!("ADL2_OverdriveN_{}{}{}", if memory { "MemoryClocks" } else { "SystemClocks" }, if x2 { "X2" } else { "" }, if set { "_Set" } else { "_Get" });
            match self.fn_::<BlockFn>(&name) {
                // SAFETY: the block's size field says how many levels it holds.
                Some((f, ctx)) => unsafe { f(ctx, adapter, levels.as_mut_ptr()) },
                None => -8,
            }
        }

        fn odn_power(&self, adapter: i32, set: bool, power: &mut [u8]) -> i32 {
            match self.fn_::<BlockFn>(if set { "ADL2_OverdriveN_PowerLimit_Set" } else { "ADL2_OverdriveN_PowerLimit_Get" }) {
                // SAFETY: ADLODNPowerLimitSetting is 12 bytes.
                Some((f, ctx)) => unsafe { f(ctx, adapter, power.as_mut_ptr()) },
                None => -8,
            }
        }

        fn odn_temperature(&self, adapter: i32, value: &mut i32) -> i32 {
            *value = 0;
            match self.fn_::<TempFn>("ADL2_OverdriveN_Temperature_Get") {
                // SAFETY: ADL call with its documented arguments. 1 = the GPU edge sensor.
                Some((f, ctx)) => unsafe { f(ctx, adapter, 1, value) },
                None => -8,
            }
        }

        fn pm_log(&self, adapter: i32, output: &mut [u8]) -> i32 {
            match self.fn_::<BlockFn>("ADL2_New_QueryPMLogData_Get") {
                // SAFETY: ADLPMLogDataOutput is 4 + 256 × 8 bytes.
                Some((f, ctx)) => unsafe { f(ctx, adapter, output.as_mut_ptr()) },
                None => -8,
            }
        }
    }
}

/// Outside Windows there is no AMD driver: Open fails like `!OperatingSystem.IsWindows()` in C#.
#[cfg(not(windows))]
mod other {
    use super::IAdl;
    use crate::base::l::L;

    #[derive(Default)]
    pub struct AdlNative;

    impl AdlNative {
        pub fn new() -> Self {
            AdlNative
        }
    }

    impl IAdl for AdlNative {
        fn open(&self, error: &mut String) -> bool {
            *error = L::t("No AMD driver is installed (its interface atiadlxx.dll was not found)").to_string();
            false
        }
        fn adapter_count(&self, count: &mut i32) -> i32 {
            *count = 0;
            -8
        }
        fn adapter_info(&self, _buffer: &mut [u8]) -> i32 {
            -8
        }
        fn active(&self, _adapter: i32, status: &mut i32) -> i32 {
            *status = 0;
            -8
        }
        fn overdrive_caps(&self, _adapter: i32, s: &mut i32, e: &mut i32, v: &mut i32) -> i32 {
            *s = 0;
            *e = 0;
            *v = 0;
            -8
        }
        fn asic_type(&self, _adapter: i32, t: &mut i32, v: &mut i32) -> i32 {
            *t = 0;
            *v = 0;
            -8
        }
        fn od8_init(&self, _adapter: i32, caps: &mut i32, list: &mut Option<Vec<u8>>) -> i32 {
            *caps = 0;
            *list = None;
            -8
        }
        fn od8_current(&self, _adapter: i32, values: &mut Option<Vec<i32>>) -> i32 {
            *values = None;
            -8
        }
        fn od8_set(&self, _adapter: i32, _set: &[u8], _current: &mut [u8]) -> i32 {
            -8
        }
        fn odn_caps(&self, _adapter: i32, caps: &mut Option<Vec<u8>>, x2: &mut bool) -> i32 {
            *x2 = false;
            *caps = Some(vec![0u8; crate::gpu::amd_oc::AmdOc::ODN_CAPS as usize]);
            -8
        }
        fn odn_clocks(&self, _adapter: i32, _memory: bool, _set: bool, _x2: bool, _levels: &mut [u8]) -> i32 {
            -8
        }
        fn odn_power(&self, _adapter: i32, _set: bool, _power: &mut [u8]) -> i32 {
            -8
        }
        fn odn_temperature(&self, _adapter: i32, value: &mut i32) -> i32 {
            *value = 0;
            -8
        }
        fn pm_log(&self, _adapter: i32, _output: &mut [u8]) -> i32 {
            -8
        }
    }
}
