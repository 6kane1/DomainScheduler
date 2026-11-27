//! Port of Hardware.cs: what the PC is (WMI), the makers' official driver pages, opening a link, running a system
//! tool with a timeout.

use super::wmi::{self, WVal};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci;
use crate::base::l::L;
use crate::lf;
use crate::native::sys_exe::SysExe;
use serde::Serialize;
use std::io::Read;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub date: String,
    pub basic: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Facts {
    pub cpu: String,
    pub cpu_vendor: String,
    pub board: String,
    pub board_vendor: String,
    pub gpus: Vec<Gpu>,
    pub audio: Vec<String>,
    pub net: Vec<String>,
    pub problems: Vec<String>,
    pub refresh: i32,
    pub ram_rated: i32,
    pub ram_now: i32,
}

impl Facts {
    /// `f.Gpus.FirstOrDefault(g => !g.Basic) ?? f.Gpus.FirstOrDefault()`.
    pub fn main_gpu(&self) -> Option<&Gpu> {
        self.gpus.iter().find(|g| !g.basic).or_else(|| self.gpus.first())
    }
}

pub const NV_CLEANSTALL: &str = "https://www.techpowerup.com/download/techpowerup-nvcleanstall/";

pub fn chipset_link(vendor: &str) -> &'static str {
    match vendor {
        "amd" => "https://www.amd.com/en/support/download/drivers.html",
        "intel" => "https://www.intel.com/content/www/us/en/support/detect.html",
        _ => "",
    }
}

pub fn gpu_link(vendor: &str) -> &'static str {
    match vendor {
        "nvidia" => "https://www.nvidia.com/en-us/drivers/",
        "amd" => "https://www.amd.com/en/support/download/drivers.html",
        "intel" => "https://www.intel.com/content/www/us/en/download-center/home.html",
        _ => "",
    }
}

pub fn board_link(vendor: &str) -> &'static str {
    match vendor {
        "asus" => "https://www.asus.com/support/download-center/",
        "msi" => "https://www.msi.com/support/download",
        "gigabyte" => "https://www.gigabyte.com/Support",
        "asrock" => "https://www.asrock.com/support/index.asp",
        "biostar" => "https://www.biostar.com.tw/app/en/support/download.php",
        "lenovo" => "https://pcsupport.lenovo.com/",
        "hp" => "https://support.hp.com/drivers",
        "dell" => "https://www.dell.com/support/home/",
        "acer" => "https://www.acer.com/support",
        "huawei" => "https://consumer.huawei.com/en/support/",
        "samsung" => "https://www.samsung.com/support/",
        "razer" => "https://mysupport.razer.com/",
        "microsoft" => "https://support.microsoft.com/surface",
        _ => "",
    }
}

/// The first map entry one of whose '|' parts the text contains (ignoring case).
pub fn vendor(text: &str, map: &[(&str, &str)]) -> String {
    for (k, v) in map {
        if k.split('|').any(|part| ci::contains(text, part)) {
            return v.to_string();
        }
    }
    String::new()
}

fn first(rows: &[Vec<WVal>]) -> Option<&Vec<WVal>> {
    rows.first()
}

/// The facts from the WMI rows (the pure part of Detect; `q(class, props)` is the query).
pub fn detect_with(q: &dyn Fn(&str, &[&str]) -> Vec<Vec<WVal>>) -> Facts {
    let mut f = Facts::default();
    let cpu = q("Win32_Processor", &["Name", "Manufacturer"]);
    if let Some(d) = first(&cpu) {
        f.cpu = d[0].str();
        f.cpu_vendor = vendor(&format!("{} {}", d[1].str(), f.cpu), &[("AMD", "amd"), ("Intel", "intel")]);
    }
    let bb = q("Win32_BaseBoard", &["Manufacturer", "Product"]);
    let cs = q("Win32_ComputerSystem", &["Manufacturer", "Model"]);
    let d2 = first(&bb);
    let d3 = first(&cs);
    let text = d3.map(|d| d[0].str()).unwrap_or_default();
    let text2 = d2.map(|d| d[0].str()).unwrap_or_default();
    let flag = text.is_empty() || ci::contains(&text, "System manufacturer") || ci::contains(&text, "To be filled") || ci::contains(&text, "Default string");
    f.board = if flag {
        format!("{} {}", text2, d2.map(|d| d[1].str()).unwrap_or_default()).trim().to_string()
    } else {
        format!("{} {}", text, d3.map(|d| d[1].str()).unwrap_or_default()).trim().to_string()
    };
    f.board_vendor = vendor(
        &format!("{} {}", text, text2),
        &[
            ("ASUS", "asus"),
            ("Micro-Star|MSI", "msi"),
            ("Gigabyte|GIGA-BYTE", "gigabyte"),
            ("ASRock", "asrock"),
            ("BIOSTAR", "biostar"),
            ("LENOVO", "lenovo"),
            ("HP|Hewlett", "hp"),
            ("Dell|Alienware", "dell"),
            ("Acer|Gateway", "acer"),
            ("HUAWEI|HONOR", "huawei"),
            ("SAMSUNG", "samsung"),
            ("Razer", "razer"),
            ("Microsoft", "microsoft"),
        ],
    );
    for item in q("Win32_VideoController", &["Name", "AdapterCompatibility", "PNPDeviceID", "DriverVersion", "DriverDate", "CurrentRefreshRate"]) {
        let text3 = item[0].str();
        f.refresh = f.refresh.max(item[5].int());
        if !text3.is_empty() && !ci::contains(&text3, "Remote") && !ci::contains(&text3, "Virtual") {
            let text4 = item[4].str();
            let u: Vec<u16> = text4.encode_utf16().collect();
            let date = if u.len() >= 8 {
                let s = |a: usize, b: usize| String::from_utf16_lossy(&u[a..b]);
                format!("{}-{}-{}", s(0, 4), s(4, 6), s(6, 8))
            } else {
                String::new()
            };
            f.gpus.push(Gpu {
                version: item[3].str(),
                date,
                basic: ci::contains(&text3, "Basic Display") || ci::contains(&text3, "Basic Render"),
                vendor: vendor(
                    &format!("{} {} {}", text3, item[1].str(), item[2].str()),
                    &[("NVIDIA|VEN_10DE", "nvidia"), ("AMD|Radeon|ATI|VEN_1002", "amd"), ("Intel|VEN_8086", "intel")],
                ),
                name: text3,
            });
        }
    }
    for item2 in q("Win32_PhysicalMemory", &["Speed", "ConfiguredClockSpeed"]) {
        f.ram_rated = f.ram_rated.max(item2[0].int());
        let num = item2[1].int();
        if num > 0 {
            f.ram_now = if f.ram_now == 0 { num } else { f.ram_now.min(num) };
        }
    }
    f.audio = q("Win32_SoundDevice", &["Name"]).iter().map(|a| a[0].str()).filter(|n| !n.is_empty() && !ci::contains(n, "Virtual")).collect();
    f.net = q("Win32_NetworkAdapter WHERE PhysicalAdapter = TRUE", &["Name"])
        .iter()
        .map(|a| a[0].str())
        .filter(|n| !n.is_empty() && !ci::contains(n, "Virtual") && !ci::contains(n, "Bluetooth"))
        .collect();
    f.problems = q("Win32_PnPEntity WHERE ConfigManagerErrorCode <> 0 AND ConfigManagerErrorCode <> 22", &["Name", "DeviceID"])
        .iter()
        .map(|p| if p[0].str().is_empty() { p[1].str() } else { p[0].str() })
        .collect();
    f
}

pub fn detect() -> Facts {
    detect_with(&|c, p| wmi::query(c, p))
}

/// Only official https pages are opened (the browser of the user, ShellExecute).
pub fn open_link(url: &str) {
    if url.is_empty() || !url.starts_with("https://") {
        return;
    }
    if let Err(e) = super::win::shell_open(url, None) {
        AppLog::add_sev(lf!("Link: {0}", e), Sev::Warn);
    }
}

/// Runs a system tool (a bare name is started from System32 by its full path, never looked up in PATH) and returns
/// its exit code and output (stdout, a new line, stderr, trimmed). -1 and the reason when it fails or times out.
pub fn run(exe: &str, args: &str, timeout_ms: u64) -> (i32, String) {
    let mut psi = SysExe::start(exe, true);
    for a in split_args(args) {
        psi.arg(&a);
    }
    let mut cmd = psi.command();
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (-1, e.to_string()),
    };
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let to = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut s);
        }
        s
    });
    let te = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(o) = err.as_mut() {
            let _ = o.read_to_end(&mut s);
        }
        s
    });
    let end = Instant::now() + Duration::from_millis(timeout_ms);
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if Instant::now() >= end {
                    return (-1, L::t("timed out").to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return (-1, e.to_string()),
        }
    };
    let o = to.join().unwrap_or_default();
    let e = te.join().unwrap_or_default();
    (code, format!("{}\n{}", decode(&o), decode(&e)).trim().to_string())
}

/// The console output of a Windows tool (the OEM code page for powercfg; UTF-8 when it is valid).
fn decode(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => oem_to_string(b),
    }
}

#[cfg(windows)]
fn oem_to_string(b: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};
    // SAFETY: the sizes are passed; the output buffer is sized by the first call.
    unsafe {
        let cp = GetOEMCP();
        let n = MultiByteToWideChar(cp, 0, b.as_ptr(), b.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(b).into_owned();
        }
        let mut w = vec![0u16; n as usize];
        MultiByteToWideChar(cp, 0, b.as_ptr(), b.len() as i32, w.as_mut_ptr(), n);
        String::from_utf16_lossy(&w)
    }
}

#[cfg(not(windows))]
fn oem_to_string(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The C# argument string split the way the C runtime does it for these simple calls (no quotes are used).
fn split_args(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}
