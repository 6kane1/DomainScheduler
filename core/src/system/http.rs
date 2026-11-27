//! The HTTP client of the updater (what C# gets from `HttpClient` + `HttpClientHandler`), over WinHTTP on Windows.
//! GET only. Redirects are followed (WinHTTP's default policy, like HttpClient: never from https to http) unless
//! switched off; the system proxy is used; no automatic decompression (HttpClientHandler's default).
//!
//! The error kinds keep the difference the C# code relies on: an `HttpRequestException` (with the status code when
//! there was a response) is [`HttpError::Request`]; the client's timeout (`TaskCanceledException`) is
//! [`HttpError::Timeout`], which the feed fetch does not swallow.

use crate::base::hooks::Hook;
use crate::base::time::TimeSpan;
use std::io::Read;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum HttpError {
    /// `HttpRequestException`: status = the response's status code (None: no response, e.g. no network).
    Request { status: Option<u16>, message: String },
    /// `TaskCanceledException` from `HttpClient.Timeout`.
    Timeout(String),
}

impl HttpError {
    pub fn message(&self) -> String {
        match self {
            HttpError::Request { message, .. } => message.clone(),
            HttpError::Timeout(m) => m.clone(),
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            HttpError::Request { status, .. } => *status,
            HttpError::Timeout(_) => None,
        }
    }

    pub(crate) fn status_error(code: u16, reason: &str) -> HttpError {
        HttpError::Request { status: Some(code), message: format!("Response status code does not indicate success: {} ({}).", code, reason) }
    }

    pub(crate) fn timeout(t: TimeSpan) -> HttpError {
        HttpError::Timeout(format!("The request was canceled due to the configured HttpClient.Timeout of {} seconds elapsing.", t.total_seconds()))
    }
}

/// A response whose headers were read (`HttpCompletionOption.ResponseHeadersRead`); the body is read from `body`.
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub content_length: Option<u64>,
    /// The Location header (redirects that were not followed).
    pub location: Option<String>,
    pub body: Box<dyn Read + Send>,
}

impl Response {
    pub fn success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The transport under the client (WinHTTP in the product; tests may set [`TRANSPORT_HOOK`]).
pub trait Transport: Send + Sync {
    fn send(&self, url: &str, headers: &[(&str, &str)], follow_redirects: bool, timeout: TimeSpan, user_agent: &str) -> Result<Response, HttpError>;
}

/// Tests: replaces WinHTTP.
pub static TRANSPORT_HOOK: Hook<dyn Transport> = Hook::new();

/// `HttpClient` with its handler settings.
pub struct Client {
    pub timeout: TimeSpan,
    /// `MaxResponseContentBufferSize` (buffered reads only).
    pub max_buffer: u64,
    pub follow_redirects: bool,
    pub user_agent: String,
}

impl Client {
    fn transport(&self) -> Arc<dyn Transport> {
        if let Some(t) = TRANSPORT_HOOK.get() {
            return t;
        }
        #[cfg(windows)]
        {
            Arc::new(win::WinHttp)
        }
        #[cfg(not(windows))]
        {
            Arc::new(NoTransport)
        }
    }

    /// `SendAsync(..., ResponseHeadersRead)`: the response, whatever its status.
    pub fn send(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, HttpError> {
        self.transport().send(url, headers, self.follow_redirects, self.timeout, &self.user_agent)
    }

    /// `GetByteArrayAsync`: a success status and at most `max_buffer` bytes, else an error.
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        let started = std::time::Instant::now();
        let mut r = self.send(url, &[])?;
        if !r.success() {
            return Err(HttpError::status_error(r.status, &r.reason));
        }
        let too_large = || HttpError::Request {
            status: None,
            message: format!("Cannot write more bytes to the buffer than the configured maximum buffer size: {}.", self.max_buffer),
        };
        if let Some(cl) = r.content_length {
            if cl > self.max_buffer {
                return Err(too_large());
            }
        }
        let mut out = Vec::new();
        let mut chunk = vec![0u8; 16384];
        loop {
            if started.elapsed() > self.timeout.to_std() {
                return Err(HttpError::timeout(self.timeout));
            }
            let n = match r.body.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Err(HttpError::timeout(self.timeout)),
                Err(e) => return Err(HttpError::Request { status: None, message: e.to_string() }),
            };
            if n == 0 {
                break;
            }
            if out.len() as u64 + n as u64 > self.max_buffer {
                return Err(too_large());
            }
            out.extend_from_slice(&chunk[..n]);
        }
        Ok(out)
    }

    /// `GetStringAsync`: the bytes as UTF-8 (a BOM is skipped).
    pub fn get_string(&self, url: &str) -> Result<String, HttpError> {
        self.get_bytes(url).map(|b| super::util::decode_text(&b))
    }
}

#[cfg(not(windows))]
struct NoTransport;

#[cfg(not(windows))]
impl Transport for NoTransport {
    fn send(&self, _url: &str, _h: &[(&str, &str)], _f: bool, _t: TimeSpan, _ua: &str) -> Result<Response, HttpError> {
        Err(HttpError::Request { status: None, message: "No network access on this platform.".into() })
    }
}

/// Resolves a Location header against the request URL (`new Uri(new Uri(url), loc)`, the cases a server sends).
pub fn resolve(base: &str, loc: &str) -> String {
    if loc.contains("://") {
        return loc.to_string();
    }
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    let scheme = &base[..scheme_end];
    let rest = &base[scheme_end..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_end];
    if let Some(l) = loc.strip_prefix("//") {
        return format!("{}{}", scheme, l);
    }
    if loc.starts_with('/') {
        return format!("{}{}{}", scheme, authority, loc);
    }
    let path = &rest[host_end..];
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "/",
    };
    format!("{}{}{}{}", scheme, authority, dir, loc)
}

#[cfg(windows)]
mod win {
    use super::{HttpError, Response, Transport};
    use crate::base::time::TimeSpan;
    use crate::native::win::wide;
    use std::ffi::c_void;
    use std::io::Read;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    struct H(*mut c_void);
    // SAFETY: WinHTTP handles may be used from any thread (one at a time here).
    unsafe impl Send for H {}
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn last_error(timeout: TimeSpan) -> HttpError {
        let code = unsafe { GetLastError() };
        if code == ERROR_WINHTTP_TIMEOUT {
            return HttpError::timeout(timeout);
        }
        HttpError::Request { status: None, message: error_text(code) }
    }

    /// The WinHTTP message for an error code (from winhttp.dll's message table).
    fn error_text(code: u32) -> String {
        use windows_sys::Win32::System::Diagnostics::Debug::{FormatMessageW, FORMAT_MESSAGE_FROM_HMODULE, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS};
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let m = wide("winhttp.dll");
        let module = unsafe { GetModuleHandleW(m.as_ptr()) };
        let mut buf = vec![0u16; 512];
        let n = unsafe {
            FormatMessageW(
                FORMAT_MESSAGE_FROM_HMODULE | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
                module as *const c_void,
                code,
                0,
                buf.as_mut_ptr(),
                buf.len() as u32,
                std::ptr::null(),
            )
        };
        let text = String::from_utf16_lossy(&buf[..n as usize]).trim().to_string();
        if text.is_empty() {
            format!("An error occurred while sending the request (WinHTTP error {}).", code)
        } else {
            format!("An error occurred while sending the request: {} (WinHTTP error {}).", text, code)
        }
    }

    fn query_string(req: *mut c_void, level: u32) -> Option<String> {
        let mut len: u32 = 0;
        unsafe { WinHttpQueryHeaders(req, level, std::ptr::null(), std::ptr::null_mut(), &mut len, std::ptr::null_mut()) };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize) / 2 + 1];
        let mut got = (buf.len() * 2) as u32;
        let ok = unsafe { WinHttpQueryHeaders(req, level, std::ptr::null(), buf.as_mut_ptr() as *mut c_void, &mut got, std::ptr::null_mut()) };
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..(got as usize) / 2]))
    }

    pub struct WinHttp;

    struct Body {
        req: H,
        _conn: H,
        _session: H,
        timeout: TimeSpan,
    }

    impl Read for Body {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if out.is_empty() {
                return Ok(0);
            }
            let mut got: u32 = 0;
            let want = out.len().min(u32::MAX as usize) as u32;
            let ok = unsafe { WinHttpReadData(self.req.0, out.as_mut_ptr() as *mut c_void, want, &mut got) };
            if ok == 0 {
                let e = last_error(self.timeout);
                let kind = if matches!(e, HttpError::Timeout(_)) { std::io::ErrorKind::TimedOut } else { std::io::ErrorKind::Other };
                return Err(std::io::Error::new(kind, e.message()));
            }
            Ok(got as usize)
        }
    }

    impl Transport for WinHttp {
        fn send(&self, url: &str, headers: &[(&str, &str)], follow_redirects: bool, timeout: TimeSpan, user_agent: &str) -> Result<Response, HttpError> {
            let wurl = wide(url);
            let mut uc: URL_COMPONENTS = unsafe { std::mem::zeroed() };
            uc.dwStructSize = std::mem::size_of::<URL_COMPONENTS>() as u32;
            uc.dwSchemeLength = u32::MAX;
            uc.dwHostNameLength = u32::MAX;
            uc.dwUrlPathLength = u32::MAX;
            uc.dwExtraInfoLength = u32::MAX;
            if unsafe { WinHttpCrackUrl(wurl.as_ptr(), 0, 0, &mut uc) } == 0 {
                return Err(HttpError::Request { status: None, message: format!("An invalid request URI was provided: {}", url) });
            }
            let piece = |p: *mut u16, n: u32| -> String {
                if p.is_null() || n == 0 {
                    String::new()
                } else {
                    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n as usize) })
                }
            };
            let host = piece(uc.lpszHostName, uc.dwHostNameLength);
            let mut path = piece(uc.lpszUrlPath, uc.dwUrlPathLength);
            path.push_str(&piece(uc.lpszExtraInfo, uc.dwExtraInfoLength));
            if path.is_empty() {
                path.push('/');
            }
            let secure = uc.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS;

            let ua = wide(user_agent);
            let session = H(unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0) });
            if session.0.is_null() {
                return Err(last_error(timeout));
            }
            let ms = timeout.total_ms().clamp(1.0, i32::MAX as f64) as i32;
            unsafe { WinHttpSetTimeouts(session.0, ms, ms, ms, ms) };
            let whost = wide(&host);
            let conn = H(unsafe { WinHttpConnect(session.0, whost.as_ptr(), uc.nPort, 0) });
            if conn.0.is_null() {
                return Err(last_error(timeout));
            }
            let verb = wide("GET");
            let wpath = wide(&path);
            let req = H(unsafe {
                WinHttpOpenRequest(conn.0, verb.as_ptr(), wpath.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), if secure { WINHTTP_FLAG_SECURE } else { 0 })
            });
            if req.0.is_null() {
                return Err(last_error(timeout));
            }
            if !follow_redirects {
                let v: u32 = WINHTTP_DISABLE_REDIRECTS;
                unsafe { WinHttpSetOption(req.0, WINHTTP_OPTION_DISABLE_FEATURE, &v as *const u32 as *const c_void, 4) };
            }
            let mut head = String::new();
            for (k, v) in headers {
                head.push_str(k);
                head.push_str(": ");
                head.push_str(v);
                head.push_str("\r\n");
            }
            let whead = wide(&head);
            let (hp, hl) = if head.is_empty() { (std::ptr::null(), 0) } else { (whead.as_ptr(), u32::MAX) };
            if unsafe { WinHttpSendRequest(req.0, hp, hl, std::ptr::null(), 0, 0, 0) } == 0 {
                return Err(last_error(timeout));
            }
            if unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) } == 0 {
                return Err(last_error(timeout));
            }
            let mut status: u32 = 0;
            let mut len: u32 = 4;
            unsafe {
                WinHttpQueryHeaders(
                    req.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    std::ptr::null(),
                    &mut status as *mut u32 as *mut c_void,
                    &mut len,
                    std::ptr::null_mut(),
                )
            };
            let reason = query_string(req.0, WINHTTP_QUERY_STATUS_TEXT).unwrap_or_default();
            let content_length = query_string(req.0, WINHTTP_QUERY_CONTENT_LENGTH).and_then(|s| s.trim().parse::<u64>().ok());
            let location = query_string(req.0, WINHTTP_QUERY_LOCATION);
            Ok(Response {
                status: status as u16,
                reason,
                content_length,
                location,
                body: Box::new(Body { req, _conn: conn, _session: session, timeout }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn locations_resolve_against_the_request() {
        assert_eq!(resolve("https://api.github.com/repos/a/b", "https://objects.example/x?sig=1"), "https://objects.example/x?sig=1");
        assert_eq!(resolve("https://api.github.com/repos/a/b", "/x/y"), "https://api.github.com/x/y");
        assert_eq!(resolve("https://api.github.com/repos/a/b", "//cdn.example/z"), "https://cdn.example/z");
        assert_eq!(resolve("https://api.github.com/repos/a/b?q=1", "c"), "https://api.github.com/repos/a/c");
    }
}
