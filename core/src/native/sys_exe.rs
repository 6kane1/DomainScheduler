//! Child processes of the program (1.15.0 Test 1.3, port of `SysExe.cs`). The program runs with administrator rights,
//! and its environment comes from the user's account, so nothing is looked up by name: every tool is started from
//! System32 by its full path, PowerShell never reads a profile and only sees the modules of Windows itself (a module
//! of the same name in the user's Documents is never loaded), and the environment variables that make a .NET or
//! PowerShell process load extra code (profilers, startup hooks) are taken out.

use crate::base::ci::{self, CiMap, CiMapExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// `System.Diagnostics.ProcessStartInfo` (the parts the program uses). `environment` starts as a copy of this
/// process's environment (keys case-insensitive, as on Windows) and is passed whole to the child.
#[derive(Clone, Debug)]
pub struct ProcessStartInfo {
    pub file_name: String,
    pub arguments: Vec<String>,
    pub environment: CiMap<String>,
    pub working_directory: Option<PathBuf>,
    pub use_shell_execute: bool,
    pub create_no_window: bool,
    pub redirect_standard_output: bool,
    pub redirect_standard_error: bool,
}

impl ProcessStartInfo {
    pub fn new(file_name: &str) -> Self {
        let mut env = CiMap::new();
        for (k, v) in std::env::vars_os() {
            env.ci_insert(&k.to_string_lossy(), v.to_string_lossy().to_string());
        }
        ProcessStartInfo {
            file_name: file_name.to_string(),
            arguments: Vec::new(),
            environment: env,
            working_directory: None,
            use_shell_execute: false,
            create_no_window: false,
            redirect_standard_output: false,
            redirect_standard_error: false,
        }
    }

    /// `psi.ArgumentList.Add(a)`.
    pub fn arg(&mut self, a: &str) -> &mut Self {
        self.arguments.push(a.to_string());
        self
    }

    /// The child as a `Command` (environment cleared and set to `environment`, no window when asked, output piped
    /// when redirected). Arguments are quoted by the MSVCRT rules, as .NET's ArgumentList does.
    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.file_name);
        c.args(&self.arguments);
        c.env_clear();
        for (k, v) in &self.environment {
            c.env(k.as_str(), v);
        }
        if let Some(d) = &self.working_directory {
            c.current_dir(d);
        }
        c.stdout(if self.redirect_standard_output { Stdio::piped() } else { Stdio::inherit() });
        c.stderr(if self.redirect_standard_error { Stdio::piped() } else { Stdio::inherit() });
        #[cfg(windows)]
        if self.create_no_window {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c
    }
}

/// `SysExe` (static class).
pub struct SysExe;

impl SysExe {
    /// `Path.Combine(Environment.SystemDirectory, exe)`.
    pub fn system32(exe: &str) -> String {
        let d = super::sys::system_directory();
        if d.as_os_str().is_empty() {
            return exe.to_string();
        }
        let d = d.to_string_lossy().to_string();
        let sep = if d.ends_with('\\') || d.ends_with('/') { "" } else if cfg!(windows) { "\\" } else { "/" };
        format!("{}{}{}", d, sep, exe)
    }

    pub fn power_shell_path() -> String {
        Self::system32(r"WindowsPowerShell\v1.0\powershell.exe")
    }

    /// The modules folder of Windows PowerShell itself (admin-only).
    pub fn modules_path() -> String {
        Self::system32(r"WindowsPowerShell\v1.0\Modules")
    }

    /// Variables that would make a child load code chosen by whoever set them.
    pub fn unsafe_var(name: &str) -> bool {
        ci::starts_with(name, "COR_")
            || ci::starts_with(name, "CORECLR_")
            || ci::starts_with(name, "COMPlus_")
            || ci::starts_with(name, "DOTNET_")
            || ci::eq(name, "PSModulePath")
    }

    fn clean(psi: &mut ProcessStartInfo) {
        let bad: Vec<String> = psi.environment.keys().filter(|k| Self::unsafe_var(k.as_str())).map(|k| k.0.clone()).collect();
        for k in bad {
            psi.environment.ci_remove(&k);
        }
        psi.environment.ci_insert("PSModulePath", Self::modules_path());
    }

    /// A System32 tool by its full path, hidden, output read by the caller; add the arguments with `arg`.
    pub fn start(exe: &str, redirect: bool) -> ProcessStartInfo {
        let file = if exe.contains('\\') || exe.contains('/') { exe.to_string() } else { Self::system32(exe) };
        let mut psi = ProcessStartInfo::new(&file);
        psi.use_shell_execute = false;
        psi.create_no_window = true;
        psi.redirect_standard_output = redirect;
        psi.redirect_standard_error = redirect;
        Self::clean(&mut psi);
        psi
    }

    /// Windows PowerShell with a fixed command: -NoProfile -NonInteractive, the module path set to Windows' own folder
    /// (also inside the command, before anything is loaded). Values never go into the command text: pass them in env
    /// (read as $env:NAME in the command).
    pub fn power_shell(command: &str, env: &[(&str, &str)]) -> ProcessStartInfo {
        let mut psi = Self::start(&Self::power_shell_path(), true);
        psi.environment.ci_insert("DOMAINOS_PSMODULES", Self::modules_path());
        for (k, v) in env {
            psi.environment.ci_insert(k, v.to_string());
        }
        let full = format!("$env:PSModulePath = $env:DOMAINOS_PSMODULES; {}", command);
        for a in ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", full.as_str()] {
            psi.arg(a);
        }
        psi
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;

    /// SecurityTests.ChildProcessesGetNoProfilerOrUserModules
    #[test]
    fn child_processes_get_no_profiler_or_user_modules() {
        let _g = static_hooks();
        std::env::set_var("COR_ENABLE_PROFILING", "1");
        std::env::set_var("DOTNET_STARTUP_HOOKS", "evil.dll");
        let psi = SysExe::start("schtasks.exe", true);
        std::env::remove_var("COR_ENABLE_PROFILING");
        std::env::remove_var("DOTNET_STARTUP_HOOKS");
        assert!(!psi.environment.ci_contains("COR_ENABLE_PROFILING"));
        assert!(!psi.environment.ci_contains("DOTNET_STARTUP_HOOKS"));
        assert_eq!(&SysExe::modules_path(), psi.environment.ci_get("PSModulePath").unwrap());
        assert_eq!(SysExe::system32("schtasks.exe"), psi.file_name); // System32's own, by its full path on Windows
        let ps = SysExe::power_shell("Get-Date", &[("X", "1")]);
        assert_eq!(ps.arguments.len(), 7);
        assert_eq!(ps.arguments[6], "$env:PSModulePath = $env:DOMAINOS_PSMODULES; Get-Date");
        assert_eq!(ps.environment.ci_get("X").unwrap(), "1");
        assert!(SysExe::unsafe_var("complus_x") && SysExe::unsafe_var("psmodulepath") && !SysExe::unsafe_var("PATH"));
    }
}
