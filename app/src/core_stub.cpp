// A stand-in for the core's C ABI (shell_core.h) so the exe builds and runs before the `ffi` area exists.
// Built only with -DDOS_CORE_STUB (build.sh adds it when the Rust library does not export the shell's calls).
// It decides nothing: fixed texts, an in-memory settings map, a 1 s version tick like the Optimizer's timer.
// Owner: app shell.
#ifdef DOS_CORE_STUB

#include <windows.h>

#include <atomic>
#include <cstdlib>
#include <cstring>
#include <map>
#include <mutex>
#include <string>
#include <thread>

#include "shell_core.h"
#include "threadmap.h"

namespace {

std::mutex g_lock;
// Settings.Defaults the shell reads: "Advanced" is true by default (1.16.0, the owner's setting).
std::map<std::string, std::string> g_settings{{"Advanced", "true"}};
std::atomic<uint64_t> g_version{1};
HANDLE g_stateEvent = nullptr;
HANDLE g_showEvent = nullptr;
bool g_on = false;
int g_state = 0;
ULONGLONG g_armedAt = 0;

char* Dup(const std::string& s)
{
	char* p = (char*)std::malloc(s.size() + 1);
	if (p) std::memcpy(p, s.c_str(), s.size() + 1);
	return p;
}

void Bump()
{
	g_version++;
	if (g_stateEvent) SetEvent(g_stateEvent);
}

std::string Esc(const std::string& s)
{
	std::string o;
	for (char c : s)
	{
		if (c == '"' || c == '\\') o += '\\';
		if ((unsigned char)c < 0x20) continue;
		o += c;
	}
	return o;
}

std::string CpuName()
{
	wchar_t buf[256] = {};
	DWORD size = sizeof buf;
	if (RegGetValueW(HKEY_LOCAL_MACHINE, L"HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", L"ProcessorNameString", RRF_RT_REG_SZ, nullptr, buf, &size) != ERROR_SUCCESS)
		return "";
	int n = WideCharToMultiByte(CP_UTF8, 0, buf, -1, nullptr, 0, nullptr, nullptr);
	std::string o((size_t)(n > 0 ? n - 1 : 0), '\0');
	if (n > 1) WideCharToMultiByte(CP_UTF8, 0, buf, -1, o.data(), n, nullptr, nullptr);
	size_t a = o.find_first_not_of(' ');
	return a == std::string::npos ? "" : o.substr(a);
}

int PhysicalCores()
{
	DWORD len = 0;
	GetLogicalProcessorInformationEx(RelationProcessorCore, nullptr, &len);
	if (len == 0) return 0;
	std::string buf(len, '\0');
	auto* p = (SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX*)buf.data();
	if (!GetLogicalProcessorInformationEx(RelationProcessorCore, p, &len)) return 0;
	int n = 0;
	for (DWORD off = 0; off < len;)
	{
		auto* e = (SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX*)(buf.data() + off);
		if (e->Relationship == RelationProcessorCore) n++;
		off += e->Size;
	}
	return n;
}

void Ticker()
{
	std::thread([] {
		for (;;)
		{
			Sleep(1000);
			Bump();
		}
	}).detach();
}

} // namespace

extern "C" {

void dos_free_str(char* s) { std::free(s); }

int dos_app_start(const char* args_json, int* show)
{
	if (show) *show = (args_json && std::strstr(args_json, "\"--autostart\"")) ? 0 : 1;
	g_stateEvent = CreateEventW(nullptr, FALSE, FALSE, nullptr);
	g_showEvent = CreateEventW(nullptr, FALSE, FALSE, L"Local\\DomainScheduler.Show");
	Ticker();
	return -1;
}

int dos_app_exit(void) { return 1; }

void* dos_show_event(void) { return g_showEvent; }

uint64_t dos_state_version(void) { return g_version.load(); }

void* dos_state_event(void) { return g_stateEvent; }

char* dos_tr(const char* en) { return Dup(en ? en : ""); }

char* dos_lang(void) { return Dup("en"); }

char* dos_setting_get(const char* key)
{
	std::lock_guard<std::mutex> g(g_lock);
	auto it = g_settings.find(key ? key : "");
	return it == g_settings.end() ? nullptr : Dup(it->second);
}

void dos_setting_set(const char* key, const char* value)
{
	std::lock_guard<std::mutex> g(g_lock);
	g_settings[key ? key : ""] = value ? value : "";
}

void dos_log_add(const char* msg, int sev)
{
	std::string line = std::string(sev == 2 ? "[error] " : sev == 1 ? "[warn] " : "[info] ") + (msg ? msg : "") + "\n";
	OutputDebugStringA(line.c_str());
}

char* dos_log_take_errors(void) { return nullptr; }

char* dos_log_shown_path(void) { return Dup("%ProgramData%\\Domain OS Toolbox\\log.txt"); }

char* dos_shell_state(void)
{
	// Armed for 5 s after Turn on, then Active (a fake game), so the pill and the steps can be looked at.
	if (g_on && g_state == 1 && GetTickCount64() - g_armedAt > 5000) g_state = 2;
	double age = g_state == 2 ? (double)(GetTickCount64() - g_armedAt - 5000) / 1000.0 : -1;
	std::string j = "{\"state\":" + std::to_string(g_state) + ",\"on\":" + (g_on ? "true" : "false") +
	                ",\"button_enabled\":true,\"applied_age_s\":" + std::to_string(age) +
	                ",\"status\":\"" + (g_on ? "Stub core: no logic is running" : "Stub core") + "\"" +
	                ",\"game_exe\":\"" + (g_state == 2 ? "game.exe" : "") + "\",\"blocked\":\"\",\"threads\":" + (g_state == 2 ? "5" : "0") +
	                ",\"mask_now\":\"" + (g_state == 2 ? "0xFF" : "0x0") + "\",\"baseline_p99\":0,\"background_count\":0,\"changed_count\":0,\"game_running\":false}";
	return Dup(j);
}

char* dos_facts(void)
{
	static std::string cpu = Esc(CpuName());
	static int logical = (int)GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
	static int physical = PhysicalCores();
	return Dup("{\"version\":\"1.16.2\",\"cpu\":\"" + cpu + "\",\"physical\":" + std::to_string(physical) + ",\"logical\":" + std::to_string(logical) + ",\"gpu\":\"\",\"hardware\":null}");
}

void dos_auto_click(void)
{
	g_on = !g_on;
	g_state = g_on ? 1 : 0;
	g_armedAt = GetTickCount64();
	Bump();
}

void dos_undo_everything(void)
{
	g_on = false;
	g_state = 0;
	Bump();
}

void dos_ui_visible(int) {}
void dos_note_ui(double) {}
void dos_note_ui_skipped(void) {}

char* dos_tray_take(void) { return nullptr; }

int dos_job_poll(uint64_t, char** result)
{
	if (result) *result = nullptr;
	return 1;
}

// The Thread map: no core, no map (the page says so).
void* dos_tm_open(int) { return nullptr; }
void dos_tm_close(void) {}
char* dos_tm_status(void) { return Dup("{\"running\":false,\"demo\":false,\"error\":\"no core\",\"game_pid\":0}"); }
char* tm_snapshot_json(tm_map*, const char*) { return nullptr; }
char* tm_series_json(tm_map*) { return nullptr; }
void tm_free_str(char* s) { std::free(s); }
float tm_layout_step(tm_map*, float) { return 0; }
float tm_layout_warm(tm_map*, uint32_t) { return 0; }
size_t tm_layout_count(tm_map*) { return 0; }
size_t tm_layout_positions(tm_map*, tm_pos*, size_t) { return 0; }
int32_t tm_layout_drag(tm_map*, uint64_t, float, float) { return TM_ERR_NULL; }
int32_t tm_layout_relayout(tm_map*) { return TM_ERR_NULL; }

} // extern "C"

#endif // DOS_CORE_STUB
