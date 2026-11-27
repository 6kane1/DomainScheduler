//! A small safe registry API over advapi32 (what C# gets from `Microsoft.Win32.Registry`), for every area.
//! 64-bit view (the exe is 64-bit), values read without expanding environment names
//! (`RegistryValueOptions.DoNotExpandEnvironmentNames`).

use super::win::{from_wide, wide};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegDeleteTreeW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_BINARY,
    REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_QWORD, REG_SZ,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
    ClassesRoot,
    Users,
}

impl Hive {
    fn raw(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            Hive::ClassesRoot => HKEY_CLASSES_ROOT,
            Hive::Users => HKEY_USERS,
        }
    }
}

/// A registry value with its kind (C# `GetValue` + `GetValueKind`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Sz(String),
    ExpandSz(String),
    MultiSz(Vec<String>),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
    /// Any other kind: (type, raw bytes).
    Other(u32, Vec<u8>),
}

impl RegValue {
    /// `Convert.ToString(GetValue(...), InvariantCulture)`: strings as they are, a DWORD as a signed int (C# reads it
    /// as `int`), a QWORD as a signed long, and the array kinds as their .NET type name.
    pub fn to_csharp_string(&self) -> String {
        match self {
            RegValue::Sz(s) | RegValue::ExpandSz(s) => s.clone(),
            RegValue::Dword(v) => (*v as i32).to_string(),
            RegValue::Qword(v) => (*v as i64).to_string(),
            RegValue::MultiSz(_) => "System.String[]".into(),
            RegValue::Binary(_) | RegValue::Other(_, _) => "System.Byte[]".into(),
        }
    }

    fn kind(&self) -> u32 {
        match self {
            RegValue::Sz(_) => REG_SZ,
            RegValue::ExpandSz(_) => REG_EXPAND_SZ,
            RegValue::MultiSz(_) => REG_MULTI_SZ,
            RegValue::Dword(_) => REG_DWORD,
            RegValue::Qword(_) => REG_QWORD,
            RegValue::Binary(_) => REG_BINARY,
            RegValue::Other(k, _) => *k,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let w = |s: &str| -> Vec<u8> { wide(s).iter().flat_map(|c| c.to_le_bytes()).collect() };
        match self {
            RegValue::Sz(s) | RegValue::ExpandSz(s) => w(s),
            RegValue::MultiSz(list) => {
                let mut v: Vec<u16> = Vec::new();
                for s in list {
                    v.extend(s.encode_utf16());
                    v.push(0);
                }
                v.push(0);
                v.iter().flat_map(|c| c.to_le_bytes()).collect()
            }
            RegValue::Dword(d) => d.to_le_bytes().to_vec(),
            RegValue::Qword(q) => q.to_le_bytes().to_vec(),
            RegValue::Binary(b) | RegValue::Other(_, b) => b.clone(),
        }
    }

    fn from_raw(kind: u32, data: &[u8]) -> RegValue {
        let u16s = || -> Vec<u16> { data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
        match kind {
            REG_SZ => RegValue::Sz(from_wide(&u16s())),
            REG_EXPAND_SZ => RegValue::ExpandSz(from_wide(&u16s())),
            REG_MULTI_SZ => {
                let w = u16s();
                let mut list = Vec::new();
                for part in w.split(|c| *c == 0) {
                    if part.is_empty() {
                        break;
                    }
                    list.push(String::from_utf16_lossy(part));
                }
                RegValue::MultiSz(list)
            }
            REG_DWORD if data.len() >= 4 => RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
            REG_QWORD if data.len() >= 8 => RegValue::Qword(u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]))),
            REG_BINARY => RegValue::Binary(data.to_vec()),
            k => RegValue::Other(k, data.to_vec()),
        }
    }
}

/// An open key; closed on drop.
pub struct Key(HKEY);

// SAFETY: a registry key handle can be used from any thread.
unsafe impl Send for Key {}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    /// `hive.OpenSubKey(path, writable)`; None when it does not exist or cannot be opened.
    pub fn open(hive: Hive, path: &str, writable: bool) -> Option<Key> {
        Self::open_raw(hive.raw(), path, writable).ok()
    }

    /// Like `open`, with the Win32 error code on failure.
    pub fn open_err(hive: Hive, path: &str, writable: bool) -> Result<Key, u32> {
        Self::open_raw(hive.raw(), path, writable)
    }

    fn open_raw(parent: HKEY, path: &str, writable: bool) -> Result<Key, u32> {
        let p = wide(path);
        let mut k: HKEY = std::ptr::null_mut();
        let sam = KEY_READ | KEY_WOW64_64KEY | if writable { KEY_WRITE } else { 0 };
        let r = unsafe { RegOpenKeyExW(parent, p.as_ptr(), 0, sam, &mut k) };
        if r == ERROR_SUCCESS { Ok(Key(k)) } else { Err(r) }
    }

    /// `hive.CreateSubKey(path, true)`.
    pub fn create(hive: Hive, path: &str) -> Result<Key, String> {
        Self::create_raw(hive.raw(), path)
    }

    fn create_raw(parent: HKEY, path: &str) -> Result<Key, String> {
        let p = wide(path);
        let mut k: HKEY = std::ptr::null_mut();
        let r = unsafe {
            RegCreateKeyExW(parent, p.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY, std::ptr::null(), &mut k, std::ptr::null_mut())
        };
        if r == ERROR_SUCCESS { Ok(Key(k)) } else { Err(error_text(r)) }
    }

    /// A subkey of this key.
    pub fn open_sub(&self, path: &str, writable: bool) -> Option<Key> {
        Self::open_raw(self.0, path, writable).ok()
    }

    pub fn create_sub(&self, path: &str) -> Result<Key, String> {
        Self::create_raw(self.0, path)
    }

    /// `GetValue(name, null, DoNotExpandEnvironmentNames)` with its kind; None when absent.
    pub fn get(&self, name: &str) -> Option<RegValue> {
        let n = wide(name);
        let mut kind: u32 = 0;
        let mut len: u32 = 0;
        let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut len) };
        if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
            return None;
        }
        loop {
            let mut buf = vec![0u8; len as usize + 2];
            let mut got = buf.len() as u32;
            let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr(), &mut got) };
            if r == ERROR_MORE_DATA {
                len = got;
                continue;
            }
            if r != ERROR_SUCCESS {
                return None;
            }
            buf.truncate(got as usize);
            return Some(RegValue::from_raw(kind, &buf));
        }
    }

    /// The value as a string (`GetValue(name) as string`): only REG_SZ / REG_EXPAND_SZ.
    pub fn get_string(&self, name: &str) -> Option<String> {
        match self.get(name)? {
            RegValue::Sz(s) | RegValue::ExpandSz(s) => Some(s),
            _ => None,
        }
    }

    /// `GetValue(name) as byte[]`: only REG_BINARY.
    pub fn get_binary(&self, name: &str) -> Option<Vec<u8>> {
        match self.get(name)? {
            RegValue::Binary(b) => Some(b),
            _ => None,
        }
    }

    /// A REG_DWORD value.
    pub fn get_dword(&self, name: &str) -> Option<u32> {
        match self.get(name)? {
            RegValue::Dword(d) => Some(d),
            _ => None,
        }
    }

    /// `SetValue(name, value, kind)`.
    pub fn set(&self, name: &str, value: &RegValue) -> Result<(), String> {
        let n = wide(name);
        let data = value.bytes();
        let r = unsafe { RegSetValueExW(self.0, n.as_ptr(), 0, value.kind(), data.as_ptr(), data.len() as u32) };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(error_text(r)) }
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<(), String> {
        self.set(name, &RegValue::Sz(value.to_string()))
    }

    /// `DeleteValue(name, throwOnMissingValue)`.
    pub fn delete_value(&self, name: &str, throw_on_missing: bool) -> Result<(), String> {
        let n = wide(name);
        let r = unsafe { RegDeleteValueW(self.0, n.as_ptr()) };
        if r == ERROR_SUCCESS || (r == ERROR_FILE_NOT_FOUND && !throw_on_missing) { Ok(()) } else { Err(error_text(r)) }
    }

    /// `DeleteSubKey(name, false)` (an empty subkey).
    pub fn delete_subkey(&self, name: &str) -> Result<(), String> {
        let n = wide(name);
        let r = unsafe { RegDeleteKeyExW(self.0, n.as_ptr(), KEY_WOW64_64KEY, 0) };
        if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(error_text(r)) }
    }

    /// `DeleteSubKeyTree(name, false)`.
    pub fn delete_subkey_tree(&self, name: &str) -> Result<(), String> {
        let n = wide(name);
        let r = unsafe { RegDeleteTreeW(self.0, n.as_ptr()) };
        if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(error_text(r)) }
    }

    /// `GetValueNames()`.
    pub fn value_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let mut name = vec![0u16; 16384];
            let mut len = name.len() as u32;
            let r = unsafe {
                RegEnumValueW(self.0, i, name.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
        out
    }

    /// `GetSubKeyNames()`.
    pub fn subkey_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let mut name = vec![0u16; 256];
            let mut len = name.len() as u32;
            let r = unsafe {
                RegEnumKeyExW(self.0, i, name.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
        out
    }
}

/// A Win32 error code as text (what the C# exception message says).
pub fn error_text(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}
