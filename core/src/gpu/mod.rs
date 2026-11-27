//! gpu: GPU overclocking (ports of NvOc.cs, AmdOc.cs, GpuProfiles.cs; the test driver FakeAdl.cs).
//!
//! # API overview
//!
//! - [`nv_oc`]: `NvClock`, `NvOc` (static class: `parse`, `clamp`, `build_set`, `build_volt`, `open`, `read`, `set`,
//!   `get_voltage`, `set_voltage`, `reset`, `temperature`, `status_text`, `error()`, `name()`), the test hooks
//!   `GET_HOOK`, `SET_HOOK`, `VOLT_GET_HOOK`, `VOLT_SET_HOOK` (C# `NvOc.GetHook` ...). nvapi64.dll is loaded on first
//!   use (Windows); elsewhere it is "not installed".
//! - [`amd_oc`]: trait `IAdl` (C# out parameters stay `&mut`), `AdlRange` (the C# `(Min, Max, Step, Def)` tuple),
//!   `AmdDomain`, `AmdState`, `AdlAdapter`, `AmdOc` (static class: `use_`, `open`, `read`, `apply`, `reset`,
//!   `temperature`, the pure parsing/building functions, `error()`, `name()`), `GpuOc` (`detect`, `error`,
//!   `NVIDIA`/`AMD`, `DETECT_HOOK`). The real ADL (atiadlxx.dll) is `adl_native::AdlNative`.
//! - [`profiles`]: `GpuProfile`, `GpuCrash`, `GpuRange`, `GpuProfiles` (gpu-profiles.json: `load`, `parse`, `save`,
//!   `to_json` = `JsonSerializer.Serialize(g, WriteIndented)`, `PATH_HOOK`), `GpuOcLogic`, `Start`.
//! - `fake_adl` (tests / feature "testsupport"): `FakeAdl` (state through `f.lock()`), `FakeAdl::rx580`, `rx6800`.
//!
//! The C# static classes lock a reentrant monitor; here each one keeps its state in a `Mutex` that the public
//! functions take once and hand to private `*_in` functions, so the same call sequences run under one lock.
//! C# exceptions inside the guarded blocks (an index outside a block the driver handed back) become `Err(message)`
//! with the .NET message, caught where C# catches them.

pub mod adl_native;
pub mod amd_oc;
#[cfg(any(test, feature = "testsupport"))]
pub mod fake_adl;
pub mod nv_oc;
pub mod profiles;

pub use amd_oc::{AdlAdapter, AdlRange, AmdDomain, AmdOc, AmdState, GpuOc, IAdl};
pub use nv_oc::{NvClock, NvOc};
pub use profiles::{GpuCrash, GpuOcLogic, GpuProfile, GpuProfiles, GpuRange, Start};

/// `BitConverter.ToInt32(b, at)` with the .NET exceptions as messages.
pub(crate) fn rd(b: &[u8], at: i32) -> Result<i32, String> {
    if at < 0 || at as usize >= b.len() {
        return Err("Index was out of range. Must be non-negative and less than the size of the collection. (Parameter 'startIndex')".into());
    }
    let at = at as usize;
    if at + 4 > b.len() {
        return Err("Destination array is not long enough to copy all the items in the collection. Check array index and length. (Parameter 'value')".into());
    }
    Ok(i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
}

/// `BitConverter.GetBytes(v).CopyTo(b, at)` with the .NET exceptions as messages.
pub(crate) fn wr(b: &mut [u8], at: i32, v: i32) -> Result<(), String> {
    if at < 0 {
        return Err("Number was less than the array's lower bound in the first dimension. (Parameter 'destinationIndex')".into());
    }
    let at = at as usize;
    if at + 4 > b.len() {
        return Err("Destination array was not long enough. Check the destination index, length, and the array's lower bounds. (Parameter 'destinationArray')".into());
    }
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    Ok(())
}

/// `array[i]` with the .NET exception as a message.
pub(crate) fn at(a: &[i32], i: i32) -> Result<i32, String> {
    if i < 0 || i as usize >= a.len() {
        return Err(INDEX_OUT.into());
    }
    Ok(a[i as usize])
}

pub(crate) const INDEX_OUT: &str = "Index was outside the bounds of the array.";
/// The message of a NullReferenceException (a driver function the C# code calls through a null delegate).
pub(crate) const NULL_REF: &str = "Object reference not set to an instance of an object.";

/// `Math.Clamp(v, lo, hi)` without the panic of `i32::clamp` (every caller has lo <= hi; C# would throw otherwise).
pub(crate) fn clampi(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

pub(crate) fn clampl(v: i64, lo: i64, hi: i64) -> i64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `Encoding.ASCII.GetString(bytes)`: every byte above 0x7F becomes '?'.
pub(crate) fn ascii(b: &[u8]) -> String {
    b.iter().map(|&c| if c < 0x80 { c as char } else { '?' }).collect()
}

/// `Encoding.ASCII.GetBytes(s)`: every char above 0x7F becomes '?'.
#[cfg(any(test, feature = "testsupport"))]
pub(crate) fn ascii_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16().map(|c| if c < 0x80 { c as u8 } else { b'?' }).collect()
}

#[cfg(test)]
mod tests;
