//! `ManagementObjectSearcher("SELECT props FROM cls").Get()` of Hardware.cs and AboutPage.cs: a WQL query in
//! root\cimv2 with plain COM calls (the vtables of IWbemLocator, IWbemServices, IEnumWbemClassObject and
//! IWbemClassObject, as etw/wmi.rs does for root\WMI). Every failure gives an empty list (C# catches everything).

/// A property value as `o[p]` gave it (C# object): a string, a number, a bool or null.
#[derive(Clone, Debug, PartialEq)]
pub enum WVal {
    Null,
    S(String),
    I(i64),
    B(bool),
}

impl WVal {
    /// `o?.ToString()?.Trim() ?? ""`.
    pub fn str(&self) -> String {
        match self {
            WVal::Null => String::new(),
            WVal::S(s) => s.trim().to_string(),
            WVal::I(i) => i.to_string(),
            WVal::B(b) => if *b { "True".into() } else { "False".into() },
        }
    }

    /// `(int)Math.Min(int.MaxValue, Convert.ToInt64(o))`; null = 0. A string that is no number counts as 0 (C# throws,
    /// which the caller does not catch for this property; the WMI classes used give numbers).
    pub fn int(&self) -> i32 {
        let v = match self {
            WVal::Null => return 0,
            WVal::S(s) => s.trim().parse::<i64>().unwrap_or(0),
            WVal::I(i) => *i,
            WVal::B(b) => *b as i64,
        };
        v.min(i32::MAX as i64) as i32
    }

    pub fn i64(&self) -> Option<i64> {
        match self {
            WVal::S(s) => s.trim().parse::<i64>().ok(),
            WVal::I(i) => Some(*i),
            _ => None,
        }
    }
}

/// One row per object: the values of `props` in their order.
#[cfg(windows)]
pub fn query(cls: &str, props: &[&str]) -> Vec<Vec<WVal>> {
    imp::query(&format!("SELECT {} FROM {}", props.join(","), cls), props).unwrap_or_default()
}

#[cfg(not(windows))]
pub fn query(_cls: &str, _props: &[&str]) -> Vec<Vec<WVal>> {
    Vec::new()
}

#[cfg(windows)]
mod imp {
    use super::WVal;
    use crate::native::win::wide;
    use core::ffi::c_void;

    type HRESULT = i32;
    type Fp = *const c_void;

    #[repr(C)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }

    const CLSID_WBEM_LOCATOR: Guid = Guid { d1: 0x4590f811, d2: 0x1d3a, d3: 0x11d0, d4: [0x89, 0x1f, 0x00, 0xaa, 0x00, 0x4b, 0x2e, 0x24] };
    const IID_IWBEM_LOCATOR: Guid = Guid { d1: 0xdc12a687, d2: 0x737f, d3: 0x11cf, d4: [0x88, 0x4d, 0x00, 0xaa, 0x00, 0x4b, 0x2e, 0x24] };

    #[repr(C)]
    struct Variant {
        vt: u16,
        r1: u16,
        r2: u16,
        r3: u16,
        val: [u64; 2],
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *const c_void, coinit: u32) -> HRESULT;
        fn CoUninitialize();
        fn CoCreateInstance(clsid: *const Guid, outer: *mut c_void, ctx: u32, iid: *const Guid, out: *mut *mut c_void) -> HRESULT;
        fn CoSetProxyBlanket(proxy: *mut c_void, authn: u32, authz: u32, principal: *const u16, level: u32, imp: u32, auth: *const c_void, caps: u32) -> HRESULT;
    }

    #[link(name = "oleaut32")]
    extern "system" {
        fn SysAllocString(s: *const u16) -> *mut u16;
        fn SysFreeString(s: *mut u16);
        fn SysStringLen(s: *const u16) -> u32;
        fn VariantClear(v: *mut Variant) -> HRESULT;
    }

    unsafe fn method(obj: *mut c_void, n: usize) -> Fp {
        let vtbl = *(obj as *const *const Fp);
        *vtbl.add(n)
    }

    unsafe fn release(obj: *mut c_void) {
        if !obj.is_null() {
            let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(method(obj, 2));
            f(obj);
        }
    }

    struct Bstr(*mut u16);
    impl Bstr {
        fn new(s: &str) -> Bstr {
            let w = wide(s);
            // SAFETY: a NUL-terminated wide string.
            Bstr(unsafe { SysAllocString(w.as_ptr()) })
        }
    }
    impl Drop for Bstr {
        fn drop(&mut self) {
            // SAFETY: from SysAllocString.
            unsafe { SysFreeString(self.0) };
        }
    }

    fn ok(h: HRESULT) -> Result<(), ()> {
        if h < 0 { Err(()) } else { Ok(()) }
    }

    /// VARIANT -> the C# object of `o[p]`.
    unsafe fn value(v: &Variant) -> WVal {
        match v.vt {
            0 | 1 => WVal::Null,
            8 => {
                let p = v.val[0] as *const u16;
                if p.is_null() {
                    WVal::S(String::new())
                } else {
                    WVal::S(String::from_utf16_lossy(std::slice::from_raw_parts(p, SysStringLen(p) as usize)))
                }
            }
            11 => WVal::B(v.val[0] as u16 != 0),
            2 => WVal::I(v.val[0] as u16 as i16 as i64),
            3 => WVal::I(v.val[0] as u32 as i32 as i64),
            16 => WVal::I(v.val[0] as u8 as i8 as i64),
            17 => WVal::I(v.val[0] as u8 as i64),
            18 => WVal::I(v.val[0] as u16 as i64),
            19 => WVal::I(v.val[0] as u32 as i64),
            20 => WVal::I(v.val[0] as i64),
            21 => WVal::I(v.val[0] as i64),
            _ => WVal::Null,
        }
    }

    pub fn query(wql: &str, props: &[&str]) -> Result<Vec<Vec<WVal>>, ()> {
        // SAFETY: plain COM calls on the objects created here; every one is released.
        unsafe {
            let init = CoInitializeEx(std::ptr::null(), 0 /* COINIT_MULTITHREADED */);
            let uninit = init >= 0;
            let r = (|| -> Result<Vec<Vec<WVal>>, ()> {
                let mut loc: *mut c_void = std::ptr::null_mut();
                ok(CoCreateInstance(&CLSID_WBEM_LOCATOR, std::ptr::null_mut(), 1, &IID_IWBEM_LOCATOR, &mut loc))?;
                let mut svc: *mut c_void = std::ptr::null_mut();
                let ns = Bstr::new(r"root\cimv2");
                let connect: unsafe extern "system" fn(*mut c_void, *mut u16, *mut u16, *mut u16, *mut u16, i32, *mut u16, *mut c_void, *mut *mut c_void) -> HRESULT =
                    std::mem::transmute(method(loc, 3));
                let rc = connect(loc, ns.0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), &mut svc);
                release(loc);
                ok(rc)?;
                let out = (|| -> Result<Vec<Vec<WVal>>, ()> {
                    ok(CoSetProxyBlanket(svc, 10, 0, std::ptr::null(), 3, 3, std::ptr::null(), 0))?;
                    let lang = Bstr::new("WQL");
                    let q = Bstr::new(wql);
                    let exec: unsafe extern "system" fn(*mut c_void, *mut u16, *mut u16, i32, *mut c_void, *mut *mut c_void) -> HRESULT = std::mem::transmute(method(svc, 20));
                    let mut en: *mut c_void = std::ptr::null_mut();
                    // WBEM_FLAG_RETURN_IMMEDIATELY | WBEM_FLAG_FORWARD_ONLY
                    ok(exec(svc, lang.0, q.0, 0x10 | 0x20, std::ptr::null_mut(), &mut en))?;
                    let _ = CoSetProxyBlanket(en, 10, 0, std::ptr::null(), 3, 3, std::ptr::null(), 0);
                    let next: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void, *mut u32) -> HRESULT = std::mem::transmute(method(en, 4));
                    let mut rows = Vec::new();
                    let names: Vec<Vec<u16>> = props.iter().map(|p| wide(p)).collect();
                    loop {
                        let mut obj: *mut c_void = std::ptr::null_mut();
                        let mut got: u32 = 0;
                        if next(en, -1, 1, &mut obj, &mut got) < 0 || got == 0 || obj.is_null() {
                            break;
                        }
                        let get: unsafe extern "system" fn(*mut c_void, *const u16, i32, *mut Variant, *mut i32, *mut i32) -> HRESULT = std::mem::transmute(method(obj, 4));
                        let mut row = Vec::with_capacity(props.len());
                        let mut good = true;
                        for n in &names {
                            let mut v: Variant = std::mem::zeroed();
                            if get(obj, n.as_ptr(), 0, &mut v, std::ptr::null_mut(), std::ptr::null_mut()) < 0 {
                                good = false; // C#: o[p] threw, the row is skipped
                                break;
                            }
                            row.push(value(&v));
                            VariantClear(&mut v);
                        }
                        release(obj);
                        if good {
                            rows.push(row);
                        }
                    }
                    release(en);
                    Ok(rows)
                })();
                release(svc);
                out
            })();
            if uninit {
                CoUninitialize();
            }
            r
        }
    }
}
