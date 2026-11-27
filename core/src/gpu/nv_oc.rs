//! Port of the NVIDIA part of `NvOc.cs`: clock offsets through the driver's performance-state table.

use super::{ascii, clampi, rd, wr, NULL_REF};
use crate::base::flags::Flags;
use crate::base::hooks::{lock, Hook};
use crate::base::l::L;
use crate::lf;
use std::sync::Mutex;

/// One clock domain of an NVIDIA card: its base frequency, the current offset and the range the driver allows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NvClock {
    /// 0 = GPU core, 4 = memory
    pub domain: i32,
    pub editable: bool,
    pub base_khz: i32,
    pub delta_khz: i32,
    pub min_delta_khz: i32,
    pub max_delta_khz: i32,
}

/// `NvOc.GetHook` (test hook: replaces the table read).
pub static GET_HOOK: Hook<dyn Fn() -> Option<Vec<u8>> + Send + Sync> = Hook::new();
/// `NvOc.SetHook` (test hook: replaces the table write).
pub static SET_HOOK: Hook<dyn Fn(&[u8]) -> i32 + Send + Sync> = Hook::new();
/// `NvOc.VoltGetHook`.
pub static VOLT_GET_HOOK: Hook<dyn Fn() -> Option<Vec<u8>> + Send + Sync> = Hook::new();
/// `NvOc.VoltSetHook`.
pub static VOLT_SET_HOOK: Hook<dyn Fn(&[u8]) -> i32 + Send + Sync> = Hook::new();

/// The static fields of NvOc (behind `_gate`).
struct State {
    gpu: isize,
    tried: bool,
    changed: bool,
    get: Option<drv::TableFn>,
    set: Option<drv::TableFn>,
    thermal: Option<drv::ThermalFn>,
    volt_get: Option<drv::TableFn>,
    volt_set: Option<drv::TableFn>,
    table_version: i32,
    error: String,
    name: String,
}

static GATE: Mutex<State> = Mutex::new(State {
    gpu: 0,
    tried: false,
    changed: false,
    get: None,
    set: None,
    thermal: None,
    volt_get: None,
    volt_set: None,
    table_version: NvOc::VERSION3,
    error: String::new(),
    name: String::new(),
});

/// How a call into nvapi64.dll can fail (C#: DllNotFoundException or another exception).
enum QiErr {
    DllNotFound,
    #[cfg_attr(not(windows), allow(dead_code))]
    Other(String),
}

/// Clock offsets for NVIDIA cards through the driver's own performance-state table (the mechanism
/// overclocking tools use). Deliberately narrow:
///  - the core and memory clock OFFSET and the core voltage boost in percent (0–100); no power limit, no fan;
///  - clocks only inside the range the driver reports and inside a cap of our own;
///  - the table is sanity-checked after reading, every write is read back;
///  - nothing is reset when the program exits: the values stay in the driver until a restart, and
///    the application re-applies saved values at start only when the last session with them survived.
/// The table layout is fixed by the driver: 20 bytes of header, 16 performance states of 456 bytes,
/// each with 8 clocks of 44 bytes. The byte work is separated from the driver calls so it can be tested.
pub struct NvOc;

const HEADER: i32 = 20;
#[allow(dead_code)]
const PSTATE: i32 = 456;
const CLOCKS_AT: i32 = 8;
const CLOCK: i32 = 44;

impl NvOc {
    pub const SIZE: i32 = 7316;
    pub const VERSION: i32 = Self::SIZE | (1 << 16);
    /// Newer drivers use versions 2 and 3: the same body plus 100 bytes of over-voltage entries at the end.
    pub const SIZE_V2: i32 = 7416;
    pub const VERSION2: i32 = Self::SIZE_V2 | (2 << 16);
    pub const VERSION3: i32 = Self::SIZE_V2 | (3 << 16);
    pub const VOLT_SIZE: i32 = 40;
    pub const VOLT_VERSION: i32 = Self::VOLT_SIZE | (1 << 16);
    pub const DOMAIN_CORE: i32 = 0;
    pub const DOMAIN_MEMORY: i32 = 4;
    /// Our own caps, tighter than what most drivers allow: +300 MHz core, +1500 MHz memory, never below −300 MHz.
    pub const CAP_CORE_KHZ: i32 = 300_000;
    pub const CAP_MEMORY_KHZ: i32 = 1_500_000;
    pub const FLOOR_KHZ: i32 = -300_000;

    /// `NvOc.Error`.
    pub fn error() -> String {
        lock(&GATE).error.clone()
    }

    /// `NvOc.Name`.
    pub fn name() -> String {
        lock(&GATE).name.clone()
    }

    /// Reads the clocks of the first performance state. Returns None when the table does not look like one.
    pub fn parse(table: Option<&[u8]>) -> Option<Vec<NvClock>> {
        let table = table?;
        if table.len() < Self::SIZE as usize {
            return None;
        }
        let i = |at: i32| rd(table, at).unwrap_or(0);
        let version = i(0);
        if version != Self::VERSION && version != Self::VERSION2 && version != Self::VERSION3 {
            return None;
        }
        let (pstates, clocks) = (i(8), i(12));
        if !(1..=16).contains(&pstates) || !(1..=8).contains(&clocks) {
            return None;
        }
        let mut list = Vec::new();
        for j in 0..clocks {
            let at = HEADER + CLOCKS_AT + j * CLOCK;
            let mut c = NvClock {
                domain: i(at),
                editable: (i(at + 8) & 1) != 0,
                delta_khz: i(at + 12),
                min_delta_khz: i(at + 16),
                max_delta_khz: i(at + 20),
                base_khz: 0,
            };
            // typeId 0: one frequency; 1: a range (min, max) — the maximum is the frequency the offset moves.
            c.base_khz = if i(at + 4) == 0 { i(at + 24) } else { i(at + 28) };
            // Plausibility: a real clock is between 100 MHz and 20 GHz and its offset range brackets zero.
            if c.base_khz < 100_000 || c.base_khz > 20_000_000 || c.min_delta_khz > 0 || c.max_delta_khz < 0 || c.max_delta_khz > 3_000_000 || c.min_delta_khz < -3_000_000 {
                return None;
            }
            list.push(c);
        }
        Some(list)
    }

    /// The offset that will really be requested: inside the driver's range and inside our cap.
    pub fn clamp(c: &NvClock, want_khz: i32) -> i32 {
        let cap = if c.domain == Self::DOMAIN_MEMORY { Self::CAP_MEMORY_KHZ } else { Self::CAP_CORE_KHZ };
        let hi = c.max_delta_khz.min(cap);
        let lo = c.min_delta_khz.max(Self::FLOOR_KHZ);
        clampi(want_khz, lo, hi)
    }

    /// A table that changes one clock offset of the first performance state and nothing else.
    pub fn build_set(domain: i32, delta_khz: i32, version: i32) -> Vec<u8> {
        let mut b = vec![0u8; if version == Self::VERSION { Self::SIZE } else { Self::SIZE_V2 } as usize];
        let _ = wr(&mut b, 0, version);
        let _ = wr(&mut b, 8, 1); // one performance state
        let _ = wr(&mut b, 12, 1); // one clock
        let at = HEADER + CLOCKS_AT;
        let _ = wr(&mut b, at, domain);
        let _ = wr(&mut b, at + 12, delta_khz);
        b
    }

    // ---- the driver ---------------------------------------------------------------------------------

    /// Finds the first NVIDIA card. False (with Error) when there is none or the driver does not answer.
    pub fn open() -> bool {
        open_in(&mut lock(&GATE))
    }

    /// The driver's status codes in words.
    pub fn status_text(rc: i32) -> String {
        match rc {
            -3 | -104 | -136 => L::t("this graphics card or driver does not support it").to_string(),
            -137 => L::t("administrator rights are needed").to_string(),
            -9 => L::t("the driver uses another version of this interface").to_string(),
            _ => lf!("driver code {0}", rc),
        }
    }

    /// The clocks as the driver reports them now; None when they cannot be read or do not pass the checks.
    pub fn read() -> Option<Vec<NvClock>> {
        read_in(&mut lock(&GATE))
    }

    /// Requests an offset for one domain. Returns the offset in force after reading it back, or None (with Error).
    pub fn set(domain: i32, want_khz: i32) -> Option<i32> {
        let st = &mut *lock(&GATE);
        if Flags::refuse(Flags::GPU_OC, L::t("GPU clock offset")) {
            st.error = L::t("The feature flag gpu-oc is closed").to_string();
            return None;
        }
        let clocks = read_in(st);
        let c = clocks.as_ref().and_then(|l| l.iter().find(|x| x.domain == domain).cloned());
        let Some(c) = c else {
            if clocks.is_some() {
                st.error = L::t("This card does not have that clock").to_string();
            }
            return None;
        };
        if !c.editable {
            st.error = L::t("The driver does not allow changing this clock").to_string();
            return None;
        }
        let delta = Self::clamp(&c, want_khz);
        let r: Result<Option<i32>, String> = (|| {
            let mut table = Self::build_set(domain, delta, st.table_version);
            let rc = call_set(st, &mut table)?;
            if rc != 0 {
                st.error = lf!("The driver refused the offset: {0}", Self::status_text(rc));
                return Ok(None);
            }
            st.changed = true;
            let after = read_in(st).and_then(|l| l.into_iter().find(|x| x.domain == domain));
            match after {
                Some(a) if a.delta_khz == delta => {}
                other => {
                    st.error = L::t("The offset read back differs from the request").to_string();
                    return Ok(other.map(|a| a.delta_khz));
                }
            }
            st.error = String::new();
            Ok(Some(delta))
        })();
        r.unwrap_or_else(|e| {
            st.error = e;
            None
        })
    }

    // ---- core voltage boost, percent ----------------------------------------------------------------

    pub fn build_volt(percent: i32) -> Vec<u8> {
        let mut b = vec![0u8; Self::VOLT_SIZE as usize];
        let _ = wr(&mut b, 0, Self::VOLT_VERSION);
        let _ = wr(&mut b, 4, clampi(percent, 0, 100));
        b
    }

    /// The voltage boost in force (0–100), or None when the card or driver does not offer it.
    pub fn get_voltage() -> Option<i32> {
        get_voltage_in(&mut lock(&GATE))
    }

    /// Requests a voltage boost of 0–100 percent; returns what is in force after reading back, or None (with Error).
    pub fn set_voltage(percent: i32) -> Option<i32> {
        let st = &mut *lock(&GATE);
        if Flags::refuse(Flags::GPU_OC, L::t("GPU voltage boost")) {
            st.error = L::t("The feature flag gpu-oc is closed").to_string();
            return None;
        }
        let percent = clampi(percent, 0, 100);
        if get_voltage_in(st).is_none() {
            st.error = L::t("This graphics card or driver does not offer a voltage boost").to_string();
            return None;
        }
        let r: Result<Option<i32>, String> = (|| {
            let mut b = Self::build_volt(percent);
            let rc;
            if let Some(h) = VOLT_SET_HOOK.get() {
                rc = h(&b);
            } else {
                if st.volt_set.is_none() {
                    st.volt_set = fn_table(0xB930_6D9B).map_err(qi_message)?;
                }
                let Some(f) = st.volt_set else {
                    st.error = L::t("This graphics card or driver does not offer a voltage boost").to_string();
                    return Ok(None);
                };
                rc = drv::call_table(f, st.gpu, &mut b);
            }
            if rc != 0 {
                st.error = lf!("The driver refused the voltage boost: {0}", Self::status_text(rc));
                return Ok(None);
            }
            st.changed = true;
            let after = get_voltage_in(st);
            if after != Some(percent) {
                st.error = L::t("The voltage boost read back differs from the request").to_string();
                return Ok(after);
            }
            st.error = String::new();
            Ok(Some(percent))
        })();
        r.unwrap_or_else(|e| {
            st.error = e;
            None
        })
    }

    /// Clock offsets and the voltage boost back to stock. Only on the user's "Reset" (never by itself at exit).
    pub fn reset(force: bool) -> bool {
        let st = &mut *lock(&GATE);
        if !st.changed && !force {
            return false;
        }
        let mut ok = true;
        let r: Result<(), String> = (|| {
            if matches!(get_voltage_in(st), Some(v) if v > 0) {
                let mut v = NvOc::build_volt(0);
                let rc = if let Some(h) = VOLT_SET_HOOK.get() {
                    h(&v)
                } else {
                    if st.volt_set.is_none() {
                        st.volt_set = fn_table(0xB930_6D9B).map_err(qi_message)?;
                    }
                    match st.volt_set {
                        Some(f) => drv::call_table(f, st.gpu, &mut v),
                        None => -1,
                    }
                };
                ok &= rc == 0;
            }
            Ok(())
        })();
        if r.is_err() {
            ok = false;
        }
        for domain in [Self::DOMAIN_CORE, Self::DOMAIN_MEMORY] {
            let r: Result<(), String> = (|| {
                let c = read_in(st).and_then(|l| l.into_iter().find(|x| x.domain == domain));
                match c {
                    Some(c) if c.delta_khz != 0 => {}
                    _ => return Ok(()),
                }
                let mut table = Self::build_set(domain, 0, st.table_version);
                ok &= call_set(st, &mut table)? == 0;
                Ok(())
            })();
            if r.is_err() {
                ok = false;
            }
        }
        st.changed = false;
        ok
    }

    /// The GPU temperature in °C, or -1 when the driver does not say.
    pub fn temperature() -> i32 {
        let st = &mut *lock(&GATE);
        if GET_HOOK.is_set() || !open_in(st) {
            return -1;
        }
        let Some(thermal) = st.thermal else { return -1 };
        // NV_GPU_THERMAL_SETTINGS v2: version, count, then sensors of 20 bytes (controller, min, max, current, target).
        let mut b = vec![0u8; 4 + 4 + 3 * 20];
        let len = b.len() as i32;
        let _ = wr(&mut b, 0, len | (2 << 16));
        if drv::call_thermal(thermal, st.gpu, 15, &mut b) != 0 || rd(&b, 4).unwrap_or(0) < 1 {
            return -1;
        }
        let t = rd(&b, 8 + 12).unwrap_or(0);
        if t > 0 && t < 130 {
            t
        } else {
            -1
        }
    }
}

fn qi_message(e: QiErr) -> String {
    match e {
        QiErr::DllNotFound => drv::DLL_NOT_FOUND_MESSAGE.to_string(),
        QiErr::Other(m) => m,
    }
}

fn fn_table(id: u32) -> Result<Option<drv::TableFn>, QiErr> {
    drv::query(id).map(|p| p.map(drv::as_table))
}

/// `SetHook != null ? SetHook(table) : _set(_gpu, table)` (a missing `_set` is a NullReferenceException in C#).
fn call_set(st: &State, table: &mut [u8]) -> Result<i32, String> {
    if let Some(h) = SET_HOOK.get() {
        return Ok(h(table));
    }
    match st.set {
        Some(f) => Ok(drv::call_table(f, st.gpu, table)),
        None => Err(NULL_REF.into()),
    }
}

fn open_in(st: &mut State) -> bool {
    if GET_HOOK.is_set() {
        return true;
    }
    if st.gpu != 0 {
        return true;
    }
    if st.tried {
        return false;
    }
    st.tried = true;
    let r: Result<bool, QiErr> = (|| {
        let init = drv::query(0x0150_E828)?;
        let enumerate = drv::query(0xE5AC_921F)?;
        st.get = drv::query(0x6FF8_1213)?.map(drv::as_table);
        st.set = drv::query(0x0F4D_AE6B)?.map(drv::as_table);
        st.thermal = drv::query(0xE364_0A56)?.map(drv::as_thermal);
        let name = drv::query(0xCEEE_8E9F)?;
        let (Some(init), Some(enumerate)) = (init, enumerate) else {
            st.error = L::t("The NVIDIA driver does not offer the needed functions").to_string();
            return Ok(false);
        };
        if st.get.is_none() || st.set.is_none() {
            st.error = L::t("The NVIDIA driver does not offer the needed functions").to_string();
            return Ok(false);
        }
        if drv::call_init(init) != 0 {
            st.error = L::t("The NVIDIA driver did not start its interface").to_string();
            return Ok(false);
        }
        let mut handles = [0isize; 64];
        let mut n = 0i32;
        if drv::call_enum(enumerate, &mut handles, &mut n) != 0 || n < 1 {
            st.error = L::t("No NVIDIA graphics card was found").to_string();
            return Ok(false);
        }
        st.gpu = handles[0];
        let mut nm = [0u8; 64];
        if let Some(name) = name {
            if drv::call_name(name, st.gpu, &mut nm) == 0 {
                st.name = ascii(&nm).trim_end_matches('\0').to_string();
            }
        }
        Ok(true)
    })();
    match r {
        Ok(v) => v,
        Err(QiErr::DllNotFound) => {
            st.error = L::t("No NVIDIA driver is installed").to_string();
            false
        }
        Err(QiErr::Other(m)) => {
            st.error = m;
            false
        }
    }
}

fn read_table(st: &mut State) -> Result<Option<Vec<u8>>, String> {
    if let Some(h) = GET_HOOK.get() {
        let hooked = h();
        if let Some(t) = &hooked {
            if t.len() >= 4 {
                st.table_version = rd(t, 0)?;
            }
        }
        return Ok(hooked);
    }
    // The driver says which version of the table it speaks: -9 means "another version".
    let mut versions: Vec<i32> = Vec::new();
    for v in [st.table_version, NvOc::VERSION3, NvOc::VERSION2, NvOc::VERSION] {
        if !versions.contains(&v) {
            versions.push(v);
        }
    }
    for version in versions {
        let mut b = vec![0u8; if version == NvOc::VERSION { NvOc::SIZE } else { NvOc::SIZE_V2 } as usize];
        wr(&mut b, 0, version)?;
        let Some(get) = st.get else { return Err(NULL_REF.into()) };
        let rc = drv::call_table(get, st.gpu, &mut b);
        if rc == 0 {
            st.table_version = version;
            return Ok(Some(b));
        }
        if rc != -9 {
            st.error = NvOc::status_text(rc);
            return Ok(None);
        }
    }
    Ok(None)
}

fn read_in(st: &mut State) -> Option<Vec<NvClock>> {
    if !open_in(st) {
        return None;
    }
    match read_table(st) {
        Ok(t) => {
            let list = NvOc::parse(t.as_deref());
            if list.is_none() {
                st.error = L::t("The driver's clock table is not in the expected form: overclocking is not offered for this card").to_string();
            }
            list
        }
        Err(e) => {
            st.error = e;
            None
        }
    }
}

fn get_voltage_in(st: &mut State) -> Option<i32> {
    let r: Result<Option<i32>, QiErr> = (|| {
        let b: Option<Vec<u8>>;
        if let Some(h) = VOLT_GET_HOOK.get() {
            b = h();
        } else {
            if !open_in(st) {
                return Ok(None);
            }
            if st.volt_get.is_none() {
                st.volt_get = fn_table(0x9DF2_3CA1)?;
            }
            let Some(f) = st.volt_get else { return Ok(None) };
            let mut v = NvOc::build_volt(0);
            if drv::call_table(f, st.gpu, &mut v) != 0 {
                return Ok(None);
            }
            b = Some(v);
        }
        let Some(b) = b else { return Ok(None) };
        if b.len() < NvOc::VOLT_SIZE as usize {
            return Ok(None);
        }
        let p = rd(&b, 4).unwrap_or(-1);
        Ok(if (0..=100).contains(&p) { Some(p) } else { None })
    })();
    r.unwrap_or(None)
}

/// Lets a test start from a fresh NvOc (the C# tests rely on the hooks only; the static state stays as it is).
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let st = &mut *lock(&GATE);
    st.changed = false;
    st.error.clear();
    st.table_version = NvOc::VERSION3;
}

// ---- nvapi64.dll ------------------------------------------------------------------------------------

#[cfg(windows)]
mod drv {
    use super::QiErr;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    pub type InitFn = unsafe extern "C" fn() -> i32;
    pub type EnumFn = unsafe extern "C" fn(*mut isize, *mut i32) -> i32;
    pub type TableFn = unsafe extern "C" fn(isize, *mut u8) -> i32;
    pub type NameFn = unsafe extern "C" fn(isize, *mut u8) -> i32;
    pub type ThermalFn = unsafe extern "C" fn(isize, i32, *mut u8) -> i32;
    type QueryFn = unsafe extern "C" fn(u32) -> *mut c_void;

    pub const DLL_NOT_FOUND_MESSAGE: &str = "Unable to load DLL 'nvapi64.dll' or one of its dependencies: The specified module could not be found.";

    /// `[DllImport("nvapi64.dll", EntryPoint = "nvapi_QueryInterface")]`: the library is loaded on the first call
    /// (through the process's DLL search order, hardened to System32 at start) and kept.
    static QUERY: OnceLock<usize> = OnceLock::new();

    fn query_fn() -> Result<QueryFn, QiErr> {
        if let Some(&p) = QUERY.get() {
            // SAFETY: p is the address of nvapi_QueryInterface, found below.
            return Ok(unsafe { std::mem::transmute::<usize, QueryFn>(p) });
        }
        let name = crate::native::win::wide("nvapi64.dll");
        // SAFETY: a NUL-terminated wide string.
        let lib = unsafe { LoadLibraryW(name.as_ptr()) };
        if lib.is_null() {
            return Err(QiErr::DllNotFound);
        }
        // SAFETY: lib is a loaded module; the name is NUL-terminated.
        let p = unsafe { GetProcAddress(lib, b"nvapi_QueryInterface\0".as_ptr()) };
        let Some(p) = p else {
            return Err(QiErr::Other("Unable to find an entry point named 'nvapi_QueryInterface' in DLL 'nvapi64.dll'.".into()));
        };
        let addr = p as usize;
        let _ = QUERY.set(addr);
        // SAFETY: the export has the signature of nvapi_QueryInterface.
        Ok(unsafe { std::mem::transmute::<usize, QueryFn>(addr) })
    }

    /// `QueryInterface(id)`: the function's address, None when the driver does not offer it.
    pub fn query(id: u32) -> Result<Option<usize>, QiErr> {
        let q = query_fn()?;
        // SAFETY: nvapi_QueryInterface takes an id and returns a pointer or null.
        let p = unsafe { q(id) };
        Ok(if p.is_null() { None } else { Some(p as usize) })
    }

    pub fn as_table(p: usize) -> TableFn {
        // SAFETY: the id asked for has this signature.
        unsafe { std::mem::transmute::<usize, TableFn>(p) }
    }
    pub fn as_thermal(p: usize) -> ThermalFn {
        // SAFETY: as above.
        unsafe { std::mem::transmute::<usize, ThermalFn>(p) }
    }

    pub fn call_init(p: usize) -> i32 {
        // SAFETY: NvAPI_Initialize takes no arguments.
        unsafe { std::mem::transmute::<usize, InitFn>(p)() }
    }
    pub fn call_enum(p: usize, handles: &mut [isize; 64], n: &mut i32) -> i32 {
        // SAFETY: NvAPI_EnumPhysicalGPUs fills at most 64 handles.
        unsafe { std::mem::transmute::<usize, EnumFn>(p)(handles.as_mut_ptr(), n) }
    }
    pub fn call_name(p: usize, gpu: isize, name: &mut [u8; 64]) -> i32 {
        // SAFETY: NvAPI_GPU_GetFullName writes a 64-byte NvAPI_ShortString.
        unsafe { std::mem::transmute::<usize, NameFn>(p)(gpu, name.as_mut_ptr()) }
    }
    /// The table calls take the block in and out ([In, Out] byte[] in C#): reads come back in the same block.
    pub fn call_table(f: TableFn, gpu: isize, table: &mut [u8]) -> i32 {
        // SAFETY: the block has the size its version field states.
        unsafe { f(gpu, table.as_mut_ptr()) }
    }
    pub fn call_thermal(f: ThermalFn, gpu: isize, sensor: i32, b: &mut [u8]) -> i32 {
        // SAFETY: the block has the size its version field states.
        unsafe { f(gpu, sensor, b.as_mut_ptr()) }
    }
}

#[cfg(not(windows))]
mod drv {
    use super::QiErr;

    pub type TableFn = fn(isize, *mut u8) -> i32;
    pub type ThermalFn = fn(isize, i32, *mut u8) -> i32;
    pub const DLL_NOT_FOUND_MESSAGE: &str = "Unable to load shared library 'nvapi64.dll' or one of its dependencies.";

    /// There is no nvapi64.dll outside Windows (C#: DllNotFoundException).
    pub fn query(_id: u32) -> Result<Option<usize>, QiErr> {
        Err(QiErr::DllNotFound)
    }
    pub fn as_table(_p: usize) -> TableFn {
        |_, _| -1
    }
    pub fn as_thermal(_p: usize) -> ThermalFn {
        |_, _, _| -1
    }
    pub fn call_init(_p: usize) -> i32 {
        -1
    }
    pub fn call_enum(_p: usize, _h: &mut [isize; 64], _n: &mut i32) -> i32 {
        -1
    }
    pub fn call_name(_p: usize, _g: isize, _n: &mut [u8; 64]) -> i32 {
        -1
    }
    pub fn call_table(_f: TableFn, _g: isize, _t: &mut [u8]) -> i32 {
        -1
    }
    pub fn call_thermal(_f: ThermalFn, _g: isize, _s: i32, _b: &mut [u8]) -> i32 {
        -1
    }
}
