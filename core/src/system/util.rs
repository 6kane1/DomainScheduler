//! Small helpers of the system area: the file calls of .NET (`File.ReadAllText`, `File.WriteAllText`, `File.Move(a, b,
//! true)`), the date format of System.Text.Json, random names, the OrdinalIgnoreCase order, and an insertion-ordered
//! JSON object writer (C# dictionaries serialize in insertion order).

use crate::base::ci::upper_char;
use crate::base::time::DateTime;
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::cmp::Ordering;
use std::path::Path;

/// `File.ReadAllText`: UTF-8 (a UTF-8 BOM is skipped, UTF-16 with a BOM is decoded), invalid bytes become U+FFFD.
pub fn read_all_text(path: &Path) -> std::io::Result<String> {
    let b = std::fs::read(path)?;
    Ok(decode_text(&b))
}

/// The text decoding of `File.ReadAllText` / `Encoding.UTF8.GetString` with BOM detection.
pub fn decode_text(b: &[u8]) -> String {
    if b.len() >= 3 && b[0] == 0xEF && b[1] == 0xBB && b[2] == 0xBF {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    if b.len() >= 2 && b[0] == 0xFF && b[1] == 0xFE {
        let w: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&w);
    }
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let w: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&w);
    }
    String::from_utf8_lossy(b).into_owned()
}

/// `File.WriteAllText`: UTF-8 without a BOM.
pub fn write_all_text(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text.as_bytes())
}

/// `File.Move(from, to, overwrite: true)`.
pub fn move_overwrite(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// `Directory.CreateDirectory(Path.GetDirectoryName(path))`.
pub fn create_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => std::fs::create_dir_all(d),
        _ => Ok(()),
    }
}

/// `RandomNumberGenerator.GetBytes(n)` (the OS generator).
pub fn random_bytes(n: usize) -> Vec<u8> {
    use aes_gcm::aead::rand_core::RngCore;
    let mut b = vec![0u8; n];
    aes_gcm::aead::OsRng.fill_bytes(&mut b);
    b
}

/// Lower-case hex of bytes (`Convert.ToHexString(b).ToLowerInvariant()`).
pub fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// Upper-case hex of bytes (`Convert.ToHexString(b)`).
pub fn hex_upper(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02X}", x)).collect()
}

/// `Convert.FromHexString`: an even number of hex digits, else None.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 || !b.iter().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..b.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// `Guid.NewGuid().ToString("N")`: 32 lower-case hex digits of a random (version 4) GUID.
pub fn guid_n() -> String {
    let mut b = random_bytes(16);
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    hex_lower(&b)
}

/// `StringComparer.OrdinalIgnoreCase.Compare`.
pub fn ci_cmp(a: &str, b: &str) -> Ordering {
    let mut x = a.chars().map(upper_char);
    let mut y = b.chars().map(upper_char);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let c = (p as u32).cmp(&(q as u32));
                if c != Ordering::Equal {
                    return c;
                }
            }
        }
    }
}

/// A DateTime as System.Text.Json writes it: `yyyy-MM-ddTHH:mm:ss`, the fraction without trailing zeros (none when 0),
/// then `Z` (Utc), the offset (Local) or nothing (Unspecified).
pub fn stj_date(d: DateTime) -> String {
    let o = d.to_o(); // yyyy-MM-ddTHH:mm:ss.fffffffK
    if o.len() < 27 {
        return o;
    }
    let mut s = o[..19].to_string();
    let frac = o[20..27].trim_end_matches('0');
    if !frac.is_empty() {
        s.push('.');
        s.push_str(frac);
    }
    s.push_str(&o[27..]);
    s
}

/// A DateTime as System.Text.Json reads it (ISO 8601; `Z` = Utc, an offset = Local, none = Unspecified).
pub fn parse_stj_date(s: &str) -> Option<DateTime> {
    DateTime::parse_iso(s, false)
}

/// Serializes `(key, value)` pairs as a JSON object in the given order (a C# `Dictionary` keeps insertion order).
pub struct Ordered<'a, V: Serialize>(pub Vec<(&'a str, &'a V)>);

impl<V: Serialize> Serialize for Ordered<'_, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// `JsonSerializer.Serialize(value, new JsonSerializerOptions { WriteIndented = true })`: two spaces, `"key": value`.
pub fn to_json_indented<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// `JsonSerializer.Serialize(value)` (compact).
pub fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// The message of a JSON error (C# `JsonException.Message`; the wording differs from .NET's).
pub fn json_err(e: serde_json::Error) -> String {
    e.to_string()
}


/// The result of a child process run by [`run_capture`].
pub struct RunResult {
    /// None: it had not ended when the wait ran out (it is left running, like C#'s `WaitForExit(ms)`).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// `Process.Start(psi)`, both redirected outputs read to the end, then `WaitForExit(timeout_ms)`. The outputs are read
/// on their own threads; when the wait runs out the output read so far is returned.
pub fn run_capture(psi: &crate::native::sys_exe::ProcessStartInfo, timeout_ms: u64) -> std::io::Result<RunResult> {
    use std::io::Read;
    let mut cmd = psi.command();
    cmd.stdin(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().map(|mut o| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = o.read_to_end(&mut b);
            b
        })
    });
    let err = child.stderr.take().map(|mut o| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = o.read_to_end(&mut b);
            b
        })
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let code = loop {
        match child.try_wait()? {
            Some(st) => break Some(st.code().unwrap_or(-1)),
            None if std::time::Instant::now() >= deadline => break None,
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    let text = |h: Option<std::thread::JoinHandle<Vec<u8>>>| -> String {
        match h {
            Some(h) if code.is_some() => h.join().map(|b| decode_console(&b)).unwrap_or_default(),
            _ => String::new(),
        }
    };
    Ok(RunResult { code, stdout: text(out), stderr: text(err) })
}

/// Console output of a Windows tool: UTF-8 when it is valid UTF-8, else the OEM code page (as .NET decodes a
/// redirected stream with the console's encoding).
pub fn decode_console(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => {
            #[cfg(windows)]
            {
                use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP};
                if b.is_empty() {
                    return String::new();
                }
                let n = unsafe { MultiByteToWideChar(CP_OEMCP, 0, b.as_ptr(), b.len() as i32, std::ptr::null_mut(), 0) };
                if n > 0 {
                    let mut w = vec![0u16; n as usize];
                    unsafe { MultiByteToWideChar(CP_OEMCP, 0, b.as_ptr(), b.len() as i32, w.as_mut_ptr(), n) };
                    return String::from_utf16_lossy(&w);
                }
            }
            String::from_utf8_lossy(b).into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::time::Kind;

    #[test]
    fn dates_like_system_text_json() {
        let d = DateTime::from_parts(2026, 5, 1, 10, 0, 0, Kind::Utc).unwrap();
        assert_eq!(stj_date(d), "2026-05-01T10:00:00Z");
        assert_eq!(stj_date(d.add_ticks(1_230_000)), "2026-05-01T10:00:00.123Z");
        assert_eq!(stj_date(DateTime::MIN), "0001-01-01T00:00:00");
        assert_eq!(parse_stj_date("2026-05-01T10:00:00Z").unwrap(), d);
        assert_eq!(parse_stj_date("0001-01-01T00:00:00").unwrap(), DateTime::MIN);
        assert_eq!(ci_cmp("a.exe", "B.exe"), Ordering::Less);
        assert_eq!(from_hex("0aFF"), Some(vec![10, 255]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(guid_n().len(), 32);
    }
}
