//! WMI `root\WMI` `MSPower_DeviceEnable` (what IrqAuto.cs does through System.Management: `ManagementObjectSearcher`,
//! `o["Enable"] = value; o.Put()`), with plain COM calls (the vtables of IWbemLocator, IWbemServices,
//! IEnumWbemClassObject and IWbemClassObject).

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

const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;
const CIM_BOOLEAN: i32 = 11;

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

/// The n-th method of a COM object's vtable.
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

fn hr(what: &str, h: HRESULT) -> Result<(), String> {
    if h < 0 { Err(format!("{} 0x{:08X}", what, h as u32)) } else { Ok(()) }
}

/// Every MSPower_DeviceEnable instance: `f(InstanceName, Enable)` returns the new Enable value to write (Put), or None
/// to leave the instance as it is.
pub fn device_enable(mut f: impl FnMut(&str, Option<bool>) -> Option<bool>) -> Result<(), String> {
    // SAFETY: plain COM calls on the objects created here; every one is released.
    unsafe {
        let init = CoInitializeEx(std::ptr::null(), 0 /* COINIT_MULTITHREADED */);
        let uninit = init >= 0;
        let r = (|| -> Result<(), String> {
            let mut loc: *mut c_void = std::ptr::null_mut();
            hr("CoCreateInstance", CoCreateInstance(&CLSID_WBEM_LOCATOR, std::ptr::null_mut(), 1 /* CLSCTX_INPROC_SERVER */, &IID_IWBEM_LOCATOR, &mut loc))?;
            let mut svc: *mut c_void = std::ptr::null_mut();
            let ns = Bstr::new(r"root\WMI");
            let connect: unsafe extern "system" fn(*mut c_void, *mut u16, *mut u16, *mut u16, *mut u16, i32, *mut u16, *mut c_void, *mut *mut c_void) -> HRESULT =
                std::mem::transmute(method(loc, 3));
            let rc = connect(loc, ns.0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), &mut svc);
            release(loc);
            hr("ConnectServer", rc)?;
            let out = (|| -> Result<(), String> {
                // RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, EOAC_NONE
                hr("CoSetProxyBlanket", CoSetProxyBlanket(svc, 10, 0, std::ptr::null(), 3, 3, std::ptr::null(), 0))?;
                let wql = Bstr::new("WQL");
                let q = Bstr::new("SELECT * FROM MSPower_DeviceEnable");
                let exec: unsafe extern "system" fn(*mut c_void, *mut u16, *mut u16, i32, *mut c_void, *mut *mut c_void) -> HRESULT = std::mem::transmute(method(svc, 20));
                let mut en: *mut c_void = std::ptr::null_mut();
                hr("ExecQuery", exec(svc, wql.0, q.0, 0x10 /* WBEM_FLAG_RETURN_IMMEDIATELY */, std::ptr::null_mut(), &mut en))?;
                let _ = CoSetProxyBlanket(en, 10, 0, std::ptr::null(), 3, 3, std::ptr::null(), 0);
                let next: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void, *mut u32) -> HRESULT = std::mem::transmute(method(en, 4));
                let res = (|| -> Result<(), String> {
                    loop {
                        let mut obj: *mut c_void = std::ptr::null_mut();
                        let mut got: u32 = 0;
                        let rc = next(en, -1 /* WBEM_INFINITE */, 1, &mut obj, &mut got);
                        hr("Next", rc)?;
                        if got == 0 || obj.is_null() {
                            return Ok(());
                        }
                        let one = (|| -> Result<(), String> {
                            let get: unsafe extern "system" fn(*mut c_void, *const u16, i32, *mut Variant, *mut i32, *mut i32) -> HRESULT = std::mem::transmute(method(obj, 4));
                            let mut v: Variant = std::mem::zeroed();
                            let name_w = wide("InstanceName");
                            hr("Get", get(obj, name_w.as_ptr(), 0, &mut v, std::ptr::null_mut(), std::ptr::null_mut()))?;
                            let key = if v.vt == VT_BSTR && v.val[0] != 0 {
                                let p = v.val[0] as *const u16;
                                String::from_utf16_lossy(std::slice::from_raw_parts(p, SysStringLen(p) as usize))
                            } else {
                                String::new()
                            };
                            VariantClear(&mut v);
                            let mut e: Variant = std::mem::zeroed();
                            let en_w = wide("Enable");
                            let enable = if get(obj, en_w.as_ptr(), 0, &mut e, std::ptr::null_mut(), std::ptr::null_mut()) >= 0 && e.vt == VT_BOOL {
                                Some(e.val[0] as u16 != 0)
                            } else {
                                None
                            };
                            VariantClear(&mut e);
                            if let Some(new) = f(&key, enable) {
                                let put: unsafe extern "system" fn(*mut c_void, *const u16, i32, *mut Variant, i32) -> HRESULT = std::mem::transmute(method(obj, 5));
                                let mut nv: Variant = std::mem::zeroed();
                                nv.vt = VT_BOOL;
                                nv.val[0] = if new { 0xFFFF } else { 0 };
                                hr("Put", put(obj, en_w.as_ptr(), 0, &mut nv, CIM_BOOLEAN))?;
                                let put_inst: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut c_void, *mut *mut c_void) -> HRESULT = std::mem::transmute(method(svc, 14));
                                hr("PutInstance", put_inst(svc, obj, 0, std::ptr::null_mut(), std::ptr::null_mut()))?;
                            }
                            Ok(())
                        })();
                        release(obj);
                        one?;
                    }
                })();
                release(en);
                res
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
