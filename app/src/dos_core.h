// The C ABI of the Rust core (dos_core, area `ffi`): everything the app (shell and pages) calls. Owner: ffi.
//
// Rules (PLAN.md, C ABI):
//  - UTF-8, NUL-terminated strings. Every char* the core returns is owned by the core: free it with dos_free_str
//    (NULL is allowed and means "none"). No call panics across the ABI; a failed call returns NULL / 0 / an
//    {"error": "..."} object.
//  - Big results are JSON (parse with json::Parse, app/src/json.h). 64-bit masks travel as strings "0x1F"
//    (json::Value::U64 reads them); ticks as numbers.
//  - The core runs its own threads (the Optimizer's worker and 1 s timer, the update timer, the --auto listener, the
//    show listener, ETW sessions). The app polls dos_state_version() (or waits on dos_state_event()) and redraws when
//    it grew.
//  - Every text the core returns is already translated (L.T / L.F of the C# code). Texts the C# page wrote as a fixed
//    L.T("...") are left to the page (core::L::T); everything that depends on a decision is computed here.
//  - C# "S.Did(msg)" (AppLog.Add + Shell.Refresh) of a command is done by the core: the page does NOT log it again,
//    it only redraws (the version grows).
//  - The core stops the app itself when C# called Application.Shutdown (update hand-over, factory reset, rollback):
//    it posts WM_QUIT to the thread that called dos_app_start (the UI thread); dos_quit_requested() says it too.
//
// This header declares the shell's calls again (the same as app/src/shell_core.h) and adds the pages' calls.
// The C++ helpers in app/src/core_bridge.h wrap the generic calls below in named functions for the pages.
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void dos_free_str(char* s);

// ================================================================ start / exit (App.xaml.cs)
/// App.OnStartup: L.Load, PrepareState, the command line switches (--revert-devices, --auto-mask, --auto,
/// --factory-reset / --uninstall), the single instance (Local\DomainScheduler.Instance, --after-update waits up to
/// 60 s), the long-lived objects (Hub, Configure, Optimizer, AutoCommand, UpdateService), SafetyAtStart, the show
/// listener. args_json: the command line (without the program) as a JSON array of strings.
/// Returns -1 when the window must be created, else the exit code (a switch was handled, or another instance was
/// signalled). *show: 0 when started with --autostart (the window stays in the tray). Call it on the UI thread.
int dos_app_start(const char* args_json, int* show);
/// App.ExitApp's CleanUp (revert everything through the worker, stop the parts, flush the log).
/// 1 = the app must quit now, 0 = cleanup was already done (a second call).
int dos_app_exit(void);
/// The auto-reset event "Local\DomainScheduler.Show" (a second start asks to show the window); NULL when missing.
void* dos_show_event(void);
/// 1 once the core asked the app to close (Application.Shutdown in C#); the core also posted WM_QUIT.
int dos_quit_requested(void);

// ================================================================ change notification
/// Grows whenever something the UI shows changed (the Optimizer's Evaluate after every tick, Auto.Changed, a job
/// finished, a log entry of a command, a tray notice, the hardware facts arrived, UpdateService.Changed).
uint64_t dos_state_version(void);
/// An auto-reset event set with each version bump; NULL = poll the version.
void* dos_state_event(void);

// ================================================================ language, formatting (L.cs, .NET formatting)
/// L.T(en).
char* dos_tr(const char* en);
/// L.Lang ("en", "ru").
char* dos_lang(void);
/// L.F(en, args) with .NET composite formatting in the current culture ("{0:0.0}", "{1:+0;-0;0}", "{0:t}"...).
/// args_json: a JSON array; an integral number is an int (long), any other number a double, true/false a bool,
/// a string a string, {"ticks": n} a UTC DateTime (shown in local time like C#).
char* dos_fmt(const char* en, const char* args_json);
/// v.ToString(format, CultureInfo.CurrentCulture) ("0", "0.0", "N0", "+0;-0;0").
char* dos_num(double v, const char* format);

// ================================================================ settings (Settings.cs)
char* dos_setting_get(const char* key);           // Settings.Get(name): value or Defaults; "" = not set
void dos_setting_set(const char* key, const char* value);

// ================================================================ log (AppLog.cs)
void dos_log_add(const char* msg, int sev);       // 0 Info, 1 Warn, 2 Error
/// Error entries added since the last call (AppLog.ErrorAdded, for ErrorPopup), oldest first:
/// [{"t":"HH:mm:ss" (local), "msg":"..."}]
char* dos_log_take_errors(void);
char* dos_log_shown_path(void);                   // AppLog.ShownPath

// ================================================================ shell state (Shell.RefreshCore, footer)
/// {"state":0..4 (AutoState Idle, Armed, Active, Reverting, Faulted), "on", "button_enabled",
///  "applied_age_s": seconds since Auto.AppliedAt (-1 when not applied), "status", "game_exe", "blocked",
///  "threads": Top5Mode ? Dynamic.View().Count : PlanView().Count(pinned), "mask_now":"0x..", "baseline_p99",
///  "background_count", "changed_count", "game_running": State == Active || LastGameRunning, "paused"}
char* dos_shell_state(void);
/// {"version": AppVersion.Display, "cpu": Native.CpuName(), "physical", "logical" (0 when no topology),
///  "gpu": the footer's name ("" until known), "hardware": Hardware.Facts or null until read:
///  {"cpu","cpu_vendor","board","board_vendor","gpus":[{"name","vendor","version","date","basic"}],
///   "audio":[..],"net":[..],"problems":[..],"refresh","ram_rated","ram_now"}}
/// Hardware.Detect() runs once off the UI thread at start; the version grows when it is done.
char* dos_facts(void);

// ================================================================ shell commands
/// Turn on / Turn off: H.Paused = false; Opt.Queue(Auto.Click, skipIfBusy: false, reports: true).
void dos_auto_click(void);
/// Undo everything: H.Paused = true; Opt.Run(Auto.RevertEverything(L.T("Button"))), then
/// Did(L.T("Everything was put back")).
void dos_undo_everything(void);
/// Hub.UiVisible (the window is on screen), Hub.NoteUi(ms), Hub.NoteUiSkipped().
void dos_ui_visible(int on);
void dos_note_ui(double ms);
void dos_note_ui_skipped(void);
/// Tray notices from the core since the last call: [{"kind":"info"|"error","text":"..."}]
/// ("info" = Tray.Info of UpdateService and AfterUpdate).
char* dos_tray_take(void);

// ================================================================ the pages' calls
/// A state getter: the JSON of `name` (see the contract below). args_json may be NULL ("{}").
/// Unknown name: {"error":"unknown: name"}.
char* dos_get(const char* name, const char* args_json);
/// A command that C# ran directly in the click handler (on the UI thread). Returns JSON ({} when it has nothing to
/// say; {"error":"..."} when refused). The page redraws after it.
char* dos_do(const char* name, const char* args_json);
/// A background job (C# Shell.Busy = the Optimizer's worker, Shell.BusyTask / Task.Run = a thread of its own,
/// Opt.Queue = the worker without an overlay). Returns its id for dos_job_poll; 0 = nothing was started (the C#
/// handler returned early, e.g. "a game runs") or unknown name.
uint64_t dos_start(const char* name, const char* args_json);
/// 0 = still running; 1 = done (*result = the job's JSON result, may be NULL; free it), also for an unknown id.
int dos_job_poll(uint64_t id, char** result);

// ================================================================ thread map (native-only page, core/src/ffi/thread_map.rs)
/// Starts the map's kernel trace (demo != 0: the synthetic feed, only for DOS_TM_DEMO=1 / DOS_UI_SHOT) unless it runs,
/// and returns the tm_map* of threadmap/threadmap.h (owned by the core, valid until the process ends; read it with
/// tm_snapshot_json, tm_series_json, tm_layout_*). Call it while the page is on screen; NULL = no map at all.
void* dos_tm_open(int demo);
/// Stops the trace (the page was left or the window hid); the map keeps its history.
void dos_tm_close(void);
/// The page's 10 Hz poll: feeds the batched events, sets the game pid from Auto, returns
/// {"running","demo","error" (translated, "" = none),"game_pid","game_exe","lost","reorder_ms"}.
char* dos_tm_status(void);

#ifdef __cplusplus
}
#endif

/* ===================================================================================================================
   JSON CONTRACT of dos_get / dos_do / dos_start (names, arguments, results). "text" values are translated.
   Masks are "0x.." strings. Enums are ints in the C# order. A page keeps only its own UI state (selection, a draft
   it edits); everything that decides is a call.

   ---- shared --------------------------------------------------------------------------------------------------
   get "topo"            -> null when TopoBuilder.Live() is null, else
                            {"logical","physical","groups","smt","name","full_mask","first_core","big_cache_llc"(-1 none),
                             "llc_groups":[n..], "l3_by_llc":[{"llc","bytes"}],
                             "sets":[{"id","group","lp","core","llc","eff"}] (C# Topo.Sets order),
                             "split":{"can_split","label","why","family","upper_half","game","non_game"}}
   get "ranges" {"mask"} -> {"text": Masks.Ranges(mask)}
   get "eligibility" {"exe"} -> {"text": Hub.Eligibility(exe) or null}
   get "system"          -> SystemPanel: {"windows": WindowsText(), "game_mode": bool, "facts": as dos_facts.hardware}
   do  "configure"       -> Hub.Configure()
   do  "open_link" {"url"} -> Hardware.OpenLink (https only)

   ---- home (HomeView.cs) ----------------------------------------------------------------------------------------
   get "home"            -> the 1 s Tick:
       {"state","on","button_enabled","blocked","game_exe","active","frames_ok","live_charts",
        "chart_wanted": LiveCharts && Active && frames available (the page adds "on screen"),
        "auto_sub": the Auto card text (with "\n"+Blocked when on), "auto_faulted": bool,
        "tuner":{"running","step_index","step_count","left_min","tag":"n/m","sub": the card text,"enabled",
                 "banner": 0 none / 1 offer / 2 no gain, "banner_text","pending": name or ""},
        "dev":{"done","busy","game","home": BootGuard.HomeDevices (0 Offer,1 BlockedByGame,2 AppliedRestartNeeded,
               3 AppliedActive),"title","tag": text or "","note": text or "","note_warn","enabled","back": bool},
        "stats":{"valid","low1_fps","avg_fps","low01_fps","frame_ms","hitches_per_min","median_ms","p99_ms"},
        "empty": the text over the chart ("" = none),
        "map": null or {"physical","logical","lps":[..],"load":[..],"kinds":[0 background,1 game,2 interrupt]},
        "background_count"}
   get "home_chart"      -> ChartTick (call at 4 Hz while chart_wanted and on screen; it notes its own time):
       {"max":[240], "end":[240], "median_ms", "p99_ms"}  (HudMath.Buckets of the last minute, 250 ms buckets)
   start "tuner_toggle"  -> Opt.Queue: TunerStart (msg logged) or TunerStop("stopped by the user"); {"msg": text|null}
   start "tuner_apply"   -> Opt.Queue(TunerApplyPending)
   do  "tuner_dismiss"
   start "devices_optimize" -> BusyTask: Flags devices-write open, IrqAuto.Apply(PlanNow()), InvalidateIrq, every line
                            logged, "Restart Windows: ..." logged. 0 when done/busy/a game runs.
   start "devices_put_back" -> BusyTask: Devices.RevertAll (+ the home/Devices message). 0 when busy or a game runs.
   do  "self_thread_window" -> SelfThreads.Register(L.N("window")) for the calling (UI) thread (HomeView's
                            constructor; appended by pages A)

   ---- quick (QuickPage.cs; the Boost/Reapply/Resync/Revert row is not ported) -------------------------------------
   get "quick"           -> {"topo_ok", "active",
       "domains_columns", "domains":[{"caption","l3_text","accent": bool (game domain, not full),"columns",
           "cores":[{"caption","lps":[..],"load","pill": text or "","pill_accent","text": text or "","accent_line"}]}],
       "game_text","bg_text","undo_text","layer_text",
       "frames":{"shown": bool,"note","p99","before","low1","low01","intervals":[ms.. last 60 s],
                 "baseline","applied_ago","mark_label","before_label"}}

   ---- game threads (GameThreadsPage.cs) -------------------------------------------------------------------------
   get "game_threads"    -> {"key": changes when the rows change, "top5_mode","sub","how","moves",
       "mask_on","mask_button","mask_tip","slot_header": "Slot"/"No." translated, "share_column": bool,
       "rows":[{"n","lead","name","detail","share": number or null,"core","prio","prio_dim"}],
       "empty": text ("" = rows shown), "empty_warn", "log":[..], "log_empty": text or ""}
   do  "mask_toggle"     -> AutoMask flipped as the C# button does, Did(msg); {"msg"}

   ---- devices (DevicesPage.cs) ------------------------------------------------------------------------------
   start "devices_plan"  -> Task: IrqAuto.PlanNow() (kept for "devices_apply_plan"):
       {"error": text|null, "empty", "in_place", "in_place_text", "flag_open", "has_backup",
        "confirm_title","confirm_text": the OK/Cancel question, "lines":[PlanLine..]}
   do  "devices_allow"   -> Flags.Set("devices-write", true)
   start "devices_apply_plan" -> BusyTask: IrqAuto.Apply(the plan of the last devices_plan), lines logged, restart note
   start "devices_restore" {"from":"layout"|"device"} -> BusyTask: Devices.RevertAll, the page's message logged
   start "devices_list"  -> BusyTask: {"rows":[{"name","msi","pinned": ranges text,"prio": text,"assign":"0x.."}]}
                            (sorted by name; kept for device_facts / device_write by index)
   start "device_facts" {"index"} -> {"index","name","driver","location","boot","restarted",
       "resources":[text..], "mine":"0x..","others":"0x..", "write": 0 allowed / 1 flag closed / 2 game runs /
       3 storage / 4 no MSI, "write_note": text, "lp_options":[{"key","label"}], "prio_options":[{"key","label"}]}
   start "device_write" {"index","lp": "" or "n","prio": "" or "1".."3"} -> Devices.Write, message logged

   ---- masks (MasksPage.cs) ------------------------------------------------------------------------------------
   get "masks_stored"    -> LoadStored: {"spec","irq","nosmt","nocore0","add":"0x..","drop":"0x.."}
   get "masks_preview" {"spec","irq","nosmt","nocore0","add","drop"} -> Info() and the Result card:
       {"options":[{"key","label"}], "irq_options":[{"key","label"}], "split_text","mask_text","background_text",
        "warn":[..], "notes":[..], "base","final","irq_drop","game","can_split","legend_game": text,"click_note"}
   do  "masks_apply" {same} -> Apply(): settings, Configure, Did; {"msg"}
   get "reservation"     -> {"now","refusals": text or "","can_apply","can_revert"}
   start "reservation_apply" / "reservation_revert" -> BusyTask, message logged

   ---- processes (ProcessesPage.cs) ---------------------------------------------------------------------------
   start "processes_list" -> Shell.Busy (worker): {"rows":[{"pid","exe","path","watched","denied","ok","changed",
       "class_text","affinity":"0x.."}]} (C# order: watched first, then name, then pid)
   do  "process_watch" {"exe","on"} -> Hub.SetWatched
   start "process_apply" {"exe","mask"} -> SaveAffRule, Configure, Paused=false, then the worker: Sys.Pass; Did
   start "process_reset" {"pid","exe"} -> SaveAffRule(""), Configure, worker: Engine.Revert(pid); Did

   ---- gpu (GpuPage.cs): the page's values live in the core ------------------------------------------------------
   start "gpu_read"      -> Shown(): Hardware.Detect, vendor, profiles, the driver's values; result = get "gpu"
   get "gpu"             -> {"loaded","flag_open","vendor","amd","error","gpus":[{"name","driver","nv_link","maker_url"}],
       "no_gpu","oc": 0 closed / 1 no vendor / 2 no clocks / 3 sliders, "oc_text": the note for 0..2,
       "tiles":[{"caption","value","unit","sub","temp"}], "skipped": text or "", "amd_note": text or "","now_text",
       "items":[{"slider":{"key","label","unit","min","max","step","value","value_text","span","hint","lo","hi",
                 "zero"}} | {"note": text}], "limits","stay","risk",
       "profiles":[{"name","core","mem","last","at_start","tip","selected"}], "last_header": "P"|"U",
       "selected": name or "", "actions": null or {"at_start","at_start_label","note": text or "","note_warn"}}
   get "gpu_temp"        -> {"value": °C or 0}
   do  "gpu_set" {"key","value"} -> the slider value snapped to its range; {"value","value_text"}
   do  "gpu_select" {"name"} -> selects (or unselects) a profile row
   start "gpu_apply" {"profile": name or null} -> GpuOcApply with the page's values (or the profile's), lines logged,
                            then the page is read again; result = get "gpu"
   start "gpu_reset"     -> Reset to stock (+ "apply at start off"); result = get "gpu"
   do  "gpu_skipped_ok" / "gpu_profile_save" {"name"} / "gpu_profile_load" / "gpu_profile_rename" {"to"} /
       "gpu_profile_delete" -> as the C# buttons (messages logged); {"ok"}
   do  "gpu_profile_at_start" {"confirmed": bool} -> {"ask": question, "title"} when the values crashed Windows before
       and not confirmed yet (show Yes/No, default No; call again with confirmed true, or with "declined": true)

   ---- settings (SettingsPage.cs) --------------------------------------------------------------------------------
   get "settings"        -> {"sys_presets","home_charts","reapply":"10000","reapply_options":[{"key","label"}],
       "lang","languages":[{"key","label"}], "functions":[{"key","label","hint","value"}] (the Functions toggles in
       order), "flags":[{"key","label","open"}], "watch":[exe..] (sorted), "known":[{"key","label"}],
       "check_updates"}
   start "autostart_read" -> {"exists"}
   start "autostart_set" {"value"} -> {"exists"} (message logged)
   do  "setting_toggle" {"key","value"} -> Toggle(): SetBool, Configure, Did(on/off), the follow-up of BgOn/TimerGuard
   do  "sys_presets" {"value"} / "reapply_set" {"ms"} / "language_set" {"code"} / "home_charts" {"value"} /
       "watch_set" {"exe","on"} / "flag_set" {"key","open"} / "check_updates" {"value"}
   start "tuner_start" / "tuner_stop" -> Opt.Queue
   start "resync"        -> worker: TopoBuilder.Invalidate, InvalidateIrq, Tables.Reload, LoadPresets, Configure,
                            Did("Resynced: ...")
   do  "factory_reset"   -> revert, Maintenance.FactoryReset, ExitApp (the app quits)

   ---- updates (UpdateService.cs, UpdatePage.cs, UpdateCard) ----------------------------------------------------
   get "update"          -> {"show_home": the home card is visible, "head","this_text","toolbox": null or
       {"title","notes","can_update","note"}, "playbook": null or {"title","notes","file": path or null,"note"},
       "busy": text or null, "error": text or null, "when", "prev": bool, "in_game", "enabled",
       "tester":{"key","on","state","channel_button","channel_visible"}}
   do  "update_check_now" / "update_install" / "playbook_download" / "playbook_show" / "playbook_site" /
       "update_rollback" / "tester_channel" / "tester_key_remove"
   start "tester_key_save" {"key"} -> {"ok","state": text}

   ---- check (CheckPage.cs) ------------------------------------------------------------------------------------
   get "check"           -> {"running","game","game_found","state","state_brush": "Text"|"Accent"|"Warn",
                             "results":[{"head","frames","waits"}]}
   do  "check_start" -> {"started"}; do "check_stop"

   ---- black box (BlackBoxPage.cs) -----------------------------------------------------------------------------
   get "blackbox"        -> {"state","count","summary","empty_note","reports":[{"when","badge","brush","text","advice"}]}
   do  "blackbox_open"

   ---- drivers, power, changes, about ------------------------------------------------------------------------------
   start "drivers"       -> {"facts": ok, "steps":[{"name","state","bad","why","how":[..],"links":[{"text","url"}]}]}
   get "drivers_guide"   -> {"exists"};   do "drivers_guide_open"
   start "power_list"    -> {"error": text or null,"plans":[{"guid","name","active","explain"}]}
   start "power_set" {"guid","name"} -> message logged
   get "changes"         -> {"groups":[{"title","items":[{"id","title","state":0 off/1 on/2 mixed,"pill","button",
                             "primary","what","why","cost","changes"}]}]}
   do  "tweak_toggle" {"id"} / "tweaks_recommended"
   get "about"           -> {"cpu","topology": text or "","version","build","program","data","log","settings",
                             "own_load","self_test"}
   start "about_wmi"     -> {"gpu","ram","board"}
   =================================================================================================================== */
