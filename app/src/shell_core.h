// The calls the app shell (window, header, status column, tray, error popup) makes into the core.
// Owner: app shell. These are the shell's REQUIREMENTS of the `ffi` area: when the real dos_core.h provides them
// (same names, same meaning), this header only declares them again. Until then core_stub.cpp (built with
// -DDOS_CORE_STUB, see build.sh) implements them with fixed values so the exe builds and runs.
//
// Rules (PLAN.md, C ABI): UTF-8, NUL-terminated; every char* returned is owned by the core and freed with
// dos_free_str (NULL is allowed and means "none"). No call blocks the UI thread for long.
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void dos_free_str(char* s);

// ---- start / exit (App.xaml.cs, owned by ffi)
/// Start-up exactly as App.OnStartup up to "new Shell": single instance, command line switches, settings migration,
/// L.Load, the long-lived objects (Hub.Configure). args_json: the command line as a JSON array of strings.
/// Returns -1 when the window must be created, else the process exit code (the switch was handled, or another
/// instance was signalled). *show: 0 when started with --autostart (the window stays in the tray).
int dos_app_start(const char* args_json, int* show);
/// App.ExitApp's CleanUp (revert everything, stop the parts). 1 = the app must quit now, 0 = already done.
int dos_app_exit(void);
/// The auto-reset event "Local\DomainScheduler.Show" (a second start asks to show the window); NULL when missing.
void* dos_show_event(void);

// ---- change notification
/// Grows whenever something the UI shows changed (the Optimizer's Evaluate, Auto.Changed, a job finished, a log
/// error, a tray notice, the hardware facts arrived).
uint64_t dos_state_version(void);
/// An auto-reset event set together with each version bump, so the UI can block on it; NULL = poll the version.
void* dos_state_event(void);

// ---- language (L.cs)
/// L.T(en): the translation of the English text, or the text itself.
char* dos_tr(const char* en);
/// L.Lang ("en", "ru").
char* dos_lang(void);

// ---- settings (Settings.cs: HKCU\Software\Domain OS\Toolbox)
char* dos_setting_get(const char* key);           // Settings.Get(name): value or Defaults; NULL/"" = not set
void dos_setting_set(const char* key, const char* value);

// ---- log (AppLog.cs)
void dos_log_add(const char* msg, int sev);       // 0 Info, 1 Warn, 2 Error (AppLog.Sev order)
/// The error entries added since the last call (AppLog.ErrorAdded, for ErrorPopup), oldest first:
/// [{"t":"HH:mm:ss" (local time of Entry.Utc), "msg":"..."}]
char* dos_log_take_errors(void);
char* dos_log_shown_path(void);                   // AppLog.ShownPath

// ---- Hub / Auto state for the header pill and the status column (Shell.RefreshCore)
/// {"state":0..4 (AutoState Idle,Armed,Active,Reverting,Faulted), "on":bool, "button_enabled":bool,
///  "applied_age_s": seconds since Auto.AppliedAt (-1 when AppliedAt is default or in the future),
///  "status": Auto.Status, "game_exe": Auto.GameExe, "blocked": Auto.Blocked,
///  "threads": Top5Mode ? Dynamic.View().Count : PlanView().Count(pinned),
///  "mask_now": "0x.." (Auto.MaskNow), "baseline_p99": Auto.BaselineP99,
///  "background_count": Engine.BackgroundCount, "changed_count": Engine.ChangedPids().Count,
///  "game_running": Auto.State == Active || Auto.LastGameRunning}
char* dos_shell_state(void);
/// {"version": AppVersion.Display, "cpu": Native.CpuName(), "physical": n, "logical": n (0 when TopoBuilder.Live()
///  is null), "gpu": the name picked in Shell's constructor from Hardware.Detect() ("" until known)}
/// The core reads Hardware.Detect() once, off the UI thread, and bumps the version when done.
char* dos_facts(void);

// ---- commands (Shell)
/// The status column's Turn on / Turn off: H.Paused = false; Opt.Queue(Auto.Click, skipIfBusy: false, reports: true).
void dos_auto_click(void);
/// Shell.UndoEverything: H.Paused = true; Opt.Run(Auto.RevertEverything(L.T("Button"))), then
/// Did(L.T("Everything was put back")) (AppLog.Add Info + a version bump).
void dos_undo_everything(void);

// ---- UI bookkeeping (Hub.UiVisible, NoteUi, NoteUiSkipped)
void dos_ui_visible(int on);
void dos_note_ui(double ms);
void dos_note_ui_skipped(void);

// ---- tray notices from the core (UpdateService's info notices)
/// [{"kind":"info"|"error","text":"..."}] since the last call.
char* dos_tray_take(void);

// ---- background jobs (Shell.Busy / BusyTask overlays): a page starts a job with its own dos_ call that returns
/// an id; the shell polls it. 0 = still running, 1 = done (*result = the job's JSON result, may be NULL).
int dos_job_poll(uint64_t id, char** result);

#ifdef __cplusplus
}
#endif
