// C++ helpers over the core's page calls (dos_core.h): JSON arguments, parsed results, and one named function per
// getter / command of the contract in dos_core.h, so a page calls bridge::Home() or bridge::TunerToggle() instead of
// spelling names. Owner: ffi. Results that are jobs return the id for Shell::Busy (0 = nothing was started).
#pragma once
#include <cstdint>
#include <string>
#include <string_view>

#include "dos_core.h"
#include "json.h"

namespace bridge {

/// A JSON object of arguments: Args().Add("exe", name).Add("on", true).
class Args {
public:
	Args& Add(const char* key, std::string_view v);
	Args& Add(const char* key, const char* v) { return Add(key, std::string_view(v ? v : "")); }
	Args& Add(const char* key, const std::string& v) { return Add(key, std::string_view(v)); }
	Args& Add(const char* key, bool v);
	Args& Add(const char* key, int v) { return Add(key, (int64_t)v); }
	Args& Add(const char* key, int64_t v);
	Args& Add(const char* key, double v);
	/// A 64-bit mask as "0x..".
	Args& Mask(const char* key, uint64_t v);
	/// The key with JSON null.
	Args& Null(const char* key);
	std::string Json() const { return "{" + body_ + "}"; }

private:
	void Key(const char* key);
	std::string body_;
};

/// One argument of Fmt (a C# object for string.Format).
struct FmtArg {
	enum Kind { Str, Int, Dbl, Bool } kind;
	std::string s;
	int64_t i = 0;
	double d = 0;
	FmtArg(const char* v) : kind(Str), s(v ? v : "") {}
	FmtArg(const std::string& v) : kind(Str), s(v) {}
	FmtArg(int v) : kind(Int), i(v) {}
	FmtArg(int64_t v) : kind(Int), i(v) {}
	FmtArg(uint32_t v) : kind(Int), i(v) {}
	FmtArg(double v) : kind(Dbl), d(v) {}
	FmtArg(bool v) : kind(Bool), i(v ? 1 : 0) {}
};

json::Value Get(const char* name, const Args& a = Args());
json::Value Do(const char* name, const Args& a = Args());
uint64_t Start(const char* name, const Args& a = Args());
/// L.F(en, args) with .NET formatting in the current culture: Fmt("{0:0.0} ms", {12.34}).
std::string Fmt(const char* en, std::initializer_list<FmtArg> args);
/// v.ToString(format, CurrentCulture).
std::string Num(double v, const char* format);

// ---- shared
inline json::Value Topo() { return Get("topo"); }
inline std::string Ranges(uint64_t mask) { return Get("ranges", Args().Mask("mask", mask))["text"].Str(); }
inline json::Value Eligibility(const std::string& exe) { return Get("eligibility", Args().Add("exe", exe))["text"]; }
inline json::Value SystemInfo() { return Get("system"); }
inline void Configure() { Do("configure"); }
inline void OpenLink(const std::string& url) { Do("open_link", Args().Add("url", url)); }

// ---- home
inline json::Value Home() { return Get("home"); }
inline json::Value HomeChart() { return Get("home_chart"); }
inline uint64_t TunerToggle() { return Start("tuner_toggle"); }
inline uint64_t TunerApply() { return Start("tuner_apply"); }
inline void TunerDismiss() { Do("tuner_dismiss"); }
inline uint64_t DevicesOptimize() { return Start("devices_optimize"); }
inline uint64_t DevicesPutBack() { return Start("devices_put_back"); }

// ---- quick, game threads
inline json::Value Quick() { return Get("quick"); }
inline json::Value GameThreads() { return Get("game_threads"); }
inline json::Value MaskToggle() { return Do("mask_toggle"); }

// ---- devices
inline uint64_t DevicesPlan() { return Start("devices_plan"); }
inline void DevicesAllow() { Do("devices_allow"); }
inline uint64_t DevicesApplyPlan() { return Start("devices_apply_plan"); }
/// from: "layout" (the interrupt layout card, home) or "device" (the device panel).
inline uint64_t DevicesRestore(const char* from) { return Start("devices_restore", Args().Add("from", from)); }
inline uint64_t DevicesList() { return Start("devices_list"); }
inline uint64_t DeviceFacts(int index) { return Start("device_facts", Args().Add("index", index)); }
inline uint64_t DeviceWrite(int index, const std::string& lp, const std::string& prio) { return Start("device_write", Args().Add("index", index).Add("lp", lp).Add("prio", prio)); }

// ---- masks
inline json::Value MasksStored() { return Get("masks_stored"); }
inline Args MaskDraft(const std::string& spec, const std::string& irq, bool nosmt, bool nocore0, uint64_t add, uint64_t drop)
{
	return Args().Add("spec", spec).Add("irq", irq).Add("nosmt", nosmt).Add("nocore0", nocore0).Mask("add", add).Mask("drop", drop);
}
inline json::Value MasksPreview(const Args& draft) { return Get("masks_preview", draft); }
inline json::Value MasksApply(const Args& draft) { return Do("masks_apply", draft); }
inline json::Value Reservation() { return Get("reservation"); }
inline uint64_t ReservationApply() { return Start("reservation_apply"); }
inline uint64_t ReservationRevert() { return Start("reservation_revert"); }

// ---- processes
inline uint64_t ProcessesList() { return Start("processes_list"); }
inline void ProcessWatch(const std::string& exe, bool on) { Do("process_watch", Args().Add("exe", exe).Add("on", on)); }
inline uint64_t ProcessApply(const std::string& exe, uint64_t mask) { return Start("process_apply", Args().Add("exe", exe).Mask("mask", mask)); }
inline uint64_t ProcessReset(uint32_t pid, const std::string& exe) { return Start("process_reset", Args().Add("pid", (int64_t)pid).Add("exe", exe)); }

// ---- gpu
inline uint64_t GpuRead() { return Start("gpu_read"); }
inline json::Value Gpu() { return Get("gpu"); }
inline int GpuTemp() { return Get("gpu_temp")["value"].Int(); }
inline json::Value GpuSet(const char* key, int value) { return Do("gpu_set", Args().Add("key", key).Add("value", value)); }
inline void GpuSelect(const std::string& name) { Do("gpu_select", Args().Add("name", name)); }
/// profile: "" = the sliders' values.
inline uint64_t GpuApply(const std::string& profile)
{
	Args a;
	if (profile.empty()) a.Null("profile"); else a.Add("profile", profile);
	return Start("gpu_apply", a);
}
inline uint64_t GpuReset() { return Start("gpu_reset"); }
inline void GpuSkippedOk() { Do("gpu_skipped_ok"); }
inline json::Value GpuProfileSave(const std::string& name) { return Do("gpu_profile_save", Args().Add("name", name)); }
inline json::Value GpuProfileLoad() { return Do("gpu_profile_load"); }
inline json::Value GpuProfileAtStart(bool confirmed, bool declined = false) { return Do("gpu_profile_at_start", Args().Add("confirmed", confirmed).Add("declined", declined)); }
inline json::Value GpuProfileRename(const std::string& to) { return Do("gpu_profile_rename", Args().Add("to", to)); }
inline json::Value GpuProfileDelete() { return Do("gpu_profile_delete"); }

// ---- settings
inline json::Value SettingsState() { return Get("settings"); }
inline uint64_t AutostartRead() { return Start("autostart_read"); }
inline uint64_t AutostartSet(bool v) { return Start("autostart_set", Args().Add("value", v)); }
inline void SettingToggle(const char* key, bool v) { Do("setting_toggle", Args().Add("key", key).Add("value", v)); }
inline void SysPresets(bool v) { Do("sys_presets", Args().Add("value", v)); }
inline void ReapplySet(const std::string& ms) { Do("reapply_set", Args().Add("ms", ms)); }
inline void LanguageSet(const std::string& code) { Do("language_set", Args().Add("code", code)); }
inline void HomeCharts(bool v) { Do("home_charts", Args().Add("value", v)); }
inline void WatchSet(const std::string& exe, bool on) { Do("watch_set", Args().Add("exe", exe).Add("on", on)); }
inline void FlagSet(const char* key, bool open) { Do("flag_set", Args().Add("key", key).Add("open", open)); }
inline void CheckUpdates(bool v) { Do("check_updates", Args().Add("value", v)); }
inline uint64_t TunerStart() { return Start("tuner_start"); }
inline uint64_t TunerStop() { return Start("tuner_stop"); }
/// The native Settings button "Reload hardware and presets" (QuickPage.Resync).
inline uint64_t Resync() { return Start("resync"); }
inline void FactoryReset() { Do("factory_reset"); }

// ---- updates
inline json::Value Update() { return Get("update"); }
inline void UpdateCheckNow() { Do("update_check_now"); }
inline void UpdateInstall() { Do("update_install"); }
inline void PlaybookDownload() { Do("playbook_download"); }
inline void PlaybookShow() { Do("playbook_show"); }
inline void PlaybookSite() { Do("playbook_site"); }
inline void UpdateRollback() { Do("update_rollback"); }
inline void TesterChannel() { Do("tester_channel"); }
inline void TesterKeyRemove() { Do("tester_key_remove"); }
inline uint64_t TesterKeySave(const std::string& key) { return Start("tester_key_save", Args().Add("key", key)); }

// ---- check, black box
inline json::Value Check() { return Get("check"); }
inline bool CheckStart() { return Do("check_start")["started"].Bool(); }
inline void CheckStop() { Do("check_stop"); }
inline json::Value BlackBox() { return Get("blackbox"); }
inline void BlackBoxOpen() { Do("blackbox_open"); }

// ---- drivers, power, changes, about
inline uint64_t Drivers() { return Start("drivers"); }
inline bool DriversGuideExists() { return Get("drivers_guide")["exists"].Bool(); }
inline void DriversGuideOpen() { Do("drivers_guide_open"); }
inline uint64_t PowerList() { return Start("power_list"); }
inline uint64_t PowerSet(const std::string& guid, const std::string& name) { return Start("power_set", Args().Add("guid", guid).Add("name", name)); }
inline json::Value Changes() { return Get("changes"); }
inline void TweakToggle(const std::string& id) { Do("tweak_toggle", Args().Add("id", id)); }
inline void TweaksRecommended() { Do("tweaks_recommended"); }
inline json::Value About() { return Get("about"); }
inline uint64_t AboutWmi() { return Start("about_wmi"); }

} // namespace bridge
