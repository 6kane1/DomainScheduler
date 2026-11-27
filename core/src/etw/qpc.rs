//! `System.Diagnostics.Stopwatch`: the QPC clock the private ETW sessions stamp their events with
//! (Wnode.ClientContext = 1 and PROCESS_TRACE_MODE_RAW_TIMESTAMP), so event times and `timestamp()` compare directly.

/// `Stopwatch.GetTimestamp()`.
#[cfg(windows)]
pub fn timestamp() -> i64 {
    let mut v: i64 = 0;
    // SAFETY: plain out parameter.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut v) };
    v
}

/// `Stopwatch.Frequency` (ticks per second).
#[cfg(windows)]
pub fn frequency() -> i64 {
    use std::sync::OnceLock;
    static F: OnceLock<i64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut v: i64 = 0;
        // SAFETY: plain out parameter.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut v) };
        if v <= 0 { 10_000_000 } else { v }
    })
}

/// `Stopwatch.GetTimestamp()` on the Linux test host: nanoseconds since the first call (.NET on Linux: 1 GHz).
#[cfg(not(windows))]
pub fn timestamp() -> i64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static BASE: OnceLock<Instant> = OnceLock::new();
    let b = *BASE.get_or_init(Instant::now);
    // Start well above 0 so "0 = none" checks of the callers never see a real stamp as 0.
    1_000_000_000_000 + b.elapsed().as_nanos() as i64
}

#[cfg(not(windows))]
pub fn frequency() -> i64 {
    1_000_000_000
}
