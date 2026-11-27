#include "shell.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>

#include "app.h"
#include "chrome.h"

namespace {

Shell* g_shell = nullptr;

constexpr const char* kSections[] = {"Scheduler", "System", "Hardware", "Settings"};

std::string SectionTitle(const std::string& section)
{
	if (section == "Scheduler") return "DomainScheduler";
	if (section == "System") return L::T("System");
	if (section == "Hardware") return L::T("Hardware");
	return L::T("Settings");
}

/// Endpoint arc -> points (the WPF / SVG arc: radii, large-arc flag, sweep 1 = clockwise on screen).
void ArcTo(std::vector<ImVec2>& pts, ImVec2 a, ImVec2 b, float r, bool large, bool sweep)
{
	float dx = (a.x - b.x) / 2, dy = (a.y - b.y) / 2;
	float d2 = dx * dx + dy * dy;
	float rr = r;
	if (d2 > rr * rr) rr = std::sqrt(d2);
	float k = std::sqrt(std::max(0.0f, (rr * rr - d2) / d2));
	if (large == sweep) k = -k;
	ImVec2 c((a.x + b.x) / 2 + k * (-dy) * -1.0f, (a.y + b.y) / 2 + k * dx * -1.0f);
	// Recompute with the standard formula: c' = k * (rx*y1'/ry, -ry*x1'/rx) with x1' = dx, y1' = dy.
	c = ImVec2(k * dy + (a.x + b.x) / 2, -k * dx + (a.y + b.y) / 2);
	float t1 = std::atan2(a.y - c.y, a.x - c.x), t2 = std::atan2(b.y - c.y, b.x - c.x);
	float dt = t2 - t1;
	if (sweep && dt < 0) dt += 6.2831853f;
	if (!sweep && dt > 0) dt -= 6.2831853f;
	int n = std::max(8, (int)(std::fabs(dt) / 0.12f));
	for (int i = 1; i <= n; i++)
	{
		float t = t1 + dt * (float)i / (float)n;
		pts.emplace_back(c.x + rr * std::cos(t), c.y + rr * std::sin(t));
	}
}

/// The settings gear of the mockup: a ring with eight short rays, thin strokes (20x20 Uniform, scaled 0.75).
void Gear(ImDrawList* dl, ImRect inner, ImU32 col)
{
	std::vector<std::vector<ImVec2>> figs;
	std::vector<ImVec2> ring;
	ring.emplace_back(10, 6.5f);
	ArcTo(ring, ImVec2(10, 6.5f), ImVec2(9.99f, 6.5f), 3.5f, true, true);
	figs.push_back(ring);
	const float rays[8][4] = {{10, 0.5f, 10, 3.5f}, {10, 16.5f, 10, 19.5f}, {0.5f, 10, 3.5f, 10}, {16.5f, 10, 19.5f, 10},
	                          {3.3f, 3.3f, 5.4f, 5.4f}, {14.6f, 14.6f, 16.7f, 16.7f}, {3.3f, 16.7f, 5.4f, 14.6f}, {14.6f, 5.4f, 16.7f, 3.3f}};
	for (const auto& r : rays) figs.push_back({ImVec2(r[0], r[1]), ImVec2(r[2], r[3])});
	// Stretch.Uniform into 20x20 minus the 1.4 pen, then the 0.75 layout scale (the pen scales too).
	ImVec2 mn(1e9f, 1e9f), mx(-1e9f, -1e9f);
	for (auto& f : figs)
		for (ImVec2 p : f) { mn.x = std::min(mn.x, p.x); mn.y = std::min(mn.y, p.y); mx.x = std::max(mx.x, p.x); mx.y = std::max(mx.y, p.y); }
	float sc = std::min((20 - 1.4f) / (mx.x - mn.x), (20 - 1.4f) / (mx.y - mn.y));
	ImVec2 off((20 - (mx.x - mn.x) * sc) / 2 - mn.x * sc, (20 - (mx.y - mn.y) * sc) / 2 - mn.y * sc);
	ImVec2 o(std::round(inner.GetCenter().x - ui::S(7.5f)), std::round(inner.GetCenter().y - ui::S(7.5f)));
	for (auto& f : figs)
	{
		std::vector<ImVec2> pts;
		for (ImVec2 p : f) pts.push_back(o + ImVec2(ui::S((off.x + p.x * sc) * 0.75f), ui::S((off.y + p.y * sc) * 0.75f)));
		dl->AddPolyline(pts.data(), (int)pts.size(), col, ImDrawFlags_None, ui::S(1.4f * 0.75f));
	}
}

/// The undo arrow of the simple footer: "M1,2 L1,7 L6,7 M1.5,7 A6,6 0 1 1 3.2,10.8", 14x14 Uniform, pen 1.6.
void UndoIcon(ImDrawList* dl, ImVec2 o, ImU32 col)
{
	std::vector<std::vector<ImVec2>> figs;
	figs.push_back({ImVec2(1, 2), ImVec2(1, 7), ImVec2(6, 7)});
	std::vector<ImVec2> arc{ImVec2(1.5f, 7)};
	ArcTo(arc, ImVec2(1.5f, 7), ImVec2(3.2f, 10.8f), 6, true, true);
	figs.push_back(arc);
	ui::DrawPath(dl, o, figs, col, 1.6f, true, ImVec2(14, 14));
}

double NowSec() { return app::Now(); }

} // namespace

Shell* TheShell() { return g_shell; }
void SetTheShell(Shell* s) { g_shell = s; }

// ================================================================ construction

Shell::Shell(HWND hwnd) : hwnd_(hwnd)
{
	for (int i = 0; i < (int)PageId::Count; i++)
	{
		pages_.push_back(pages::Make((PageId)i));
		pages_.back()->S = this;
	}
	sysAdv_ = pages::MakeSystemPanel();
	home_ = pages::MakeHome();
	home_->S = this;
	lang_ = L::Lang();
	state_ = core::TakeJson(dos_shell_state());
	facts_ = core::TakeJson(dos_facts());
	Texts();
	Show(0);
	SetSimple(!core::SettingBool("Advanced"), false);
	OnCoreChanged();
	Refresh();
}

Shell::~Shell() = default;

bool Shell::OnScreen() const { return app::OnScreen(); }

float Shell::DpiScaleX() const { return ui::Dpi(); }

bool Shell::AutoOn() const { return state_["on"].Bool(); }

bool Shell::Maximized() const { return IsZoomed(hwnd_) != FALSE; }

ui::Thick Shell::Padding() const
{
	if (Maximized()) return chrome::MaximizedPadding(hwnd_);
	return ui::Thick(0);
}

float Shell::CaptionPx() const
{
	ui::Thick p = Padding();
	float border = (!Maximized() && !dwmBorder_) ? 1.0f : 0.0f;
	return ui::S(p.t) + border + ui::S(chrome::CaptionHeight);
}

void Shell::OnCreated() { dwmBorder_ = chrome::Dwm(hwnd_); }

int Shell::FindPage(PageId id) const { return (int)id; }

Page* Shell::Get(PageId id) { return pages_[(size_t)id].get(); }

// ================================================================ texts

void Shell::Texts()
{
	// The status column's captions and rows are drawn from L::T each frame; the dot is repainted.
	if (sysAdv_) sysAdv_->Texts();
	dotState_ = -1;
}

void Shell::SimpleTexts()
{
	std::string version = facts_["version"].Str();
	std::string cpu = facts_["cpu"].Str();
	int physical = facts_["physical"].Int(), logical = facts_["logical"].Int();
	bool topo = facts_["logical"].Int() > 0;
	std::string gpu = facts_["gpu"].Str();
	std::vector<std::string> parts{L::F("Version {0}", {version}), cpu};
	if (topo) parts.push_back(L::F("{0} cores / {1} threads", {std::to_string(physical), std::to_string(logical)}));
	if (!gpu.empty()) parts.push_back(gpu);
	std::string joined, tip;
	for (size_t i = 0; i < parts.size(); i++)
	{
		bool blank = parts[i].find_first_not_of(" \t\r\n") == std::string::npos;
		if (!blank)
		{
			if (!joined.empty()) joined += "   ";
			joined += parts[i];
		}
		if (i > 0) tip += " \xC2\xB7 ";
		tip += parts[i];
	}
	simpleFoot_ = ui::Upper(joined);
	simpleFootTip_ = tip;
	foot_ = L::F("Version {0} · user mode, no driver", {version}) + "\n" + cpu +
	        (topo ? "\n" + L::F("{0} cores / {1} threads", {std::to_string(physical), std::to_string(logical)}) : "") +
	        (!gpu.empty() ? "\n" + gpu : "");
	if (home_) home_->Texts();
}

void Shell::OnLanguage()
{
	// Every part is rebuilt on its own: one failing part must not leave the rest (the open settings page above
	// all) in the old language until the user leaves it. The settings page goes first because it is on screen.
	auto Step = [](const char* what, const std::function<void()>& a) {
		try
		{
			a();
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Language switch, {0}: {1}", {what, ex.what()}), core::Sev::Warn);
		}
	};
	if (simple_ && simpleSettings_) Step("settings", [&] { ShowSimpleSettings(true); });
	else Step("page", [&] { Show(current_); });
	Step("texts", [&] { Texts(); });
	Step("home", [&] { SimpleTexts(); });
	if (simple_ && !simpleSettings_) Step("home", [&] { ShowSimpleSettings(false); });
	Step("refresh", [&] { Refresh(); });
}

// ================================================================ modes and pages

void Shell::ResizeDip(float w, float h)
{
	RECT rc;
	GetWindowRect(hwnd_, &rc);
	SetWindowPos(hwnd_, nullptr, 0, 0, (int)std::lround(w * ui::Dpi()), (int)std::lround(h * ui::Dpi()), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
}

void Shell::SetSimple(bool simple, bool resize)
{
	simple_ = simple;
	core::SetSettingBool("Advanced", !simple);
	bool normal = !IsZoomed(hwnd_) && !IsIconic(hwnd_);
	if (resize && normal) ResizeDip(simple ? 1000.0f : 1340.0f, simple ? 700.0f : 840.0f);
	else if (!resize && simple) ResizeDip(1000.0f, 700.0f);
	else
	{
		// The new MinWidth / MinHeight apply at once (WM_GETMINMAXINFO).
		RECT rc;
		GetWindowRect(hwnd_, &rc);
		ImVec2 mn = MinSizeDip();
		int w = std::max<int>(rc.right - rc.left, (int)std::lround(mn.x * ui::Dpi()));
		int h = std::max<int>(rc.bottom - rc.top, (int)std::lround(mn.y * ui::Dpi()));
		if (normal && (w != rc.right - rc.left || h != rc.bottom - rc.top))
			SetWindowPos(hwnd_, nullptr, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
	}
	if (simple)
	{
		HidePage(current_);
		ShowSimpleSettings(false);
	}
	else
	{
		if (home_) home_->SetVisible(false);
		Show(current_);
	}
	Refresh();
	app::Redraw();
}

void Shell::ShowSimpleSettings(bool on)
{
	simpleSettings_ = on;
	if (on)
	{
		if (home_) home_->SetVisible(false);
		int i = FindPage(PageId::Settings);
		pageError_.clear();
		try
		{
			pages_[(size_t)i]->Build();
			pages_[(size_t)i]->Shown();
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Page {0}: {1}", {pages_[(size_t)i]->Title(), ex.what()}), core::Sev::Error);
			pageError_ = ex.what();
		}
	}
	app::Redraw();
}

void Shell::Show(int index)
{
	if (index < 0 || index >= (int)pages_.size()) index = 0;
	if (pages_[(size_t)index]->ClosedInAuto() && AutoOn()) index = 0;
	if (index != current_) HidePage(current_);
	current_ = index;
	pageBuiltFor_++;   // a new element: scroll positions start at the top again
	pageError_.clear();
	try
	{
		pages_[(size_t)index]->Build();
		pages_[(size_t)index]->Shown();
	}
	catch (const std::exception& ex)
	{
		core::Log(L::F("Page {0}: {1}", {pages_[(size_t)index]->Title(), ex.what()}), core::Sev::Error);
		pageError_ = ex.what();
	}
	app::Redraw();
}

void Shell::HidePage(int index)
{
	if (index < 0 || index >= (int)pages_.size()) return;
	try
	{
		pages_[(size_t)index]->Hidden();
	}
	catch (const std::exception& ex)
	{
		core::Log(L::F("Page {0}: {1}", {pages_[(size_t)index]->Title(), ex.what()}), core::Sev::Error);
	}
}

void Shell::ShowPage(PageId id)
{
	if (simple_ && simpleSettings_ && id == PageId::Settings)
	{
		ShowSimpleSettings(true);
		return;
	}
	Show(FindPage(id));
}

void Shell::UndoEverything() { dos_undo_everything(); }

void Shell::Busy(const std::string& text, uint64_t job, std::function<void(const json::Value&)> done)
{
	overlayText_ = text;
	overlay_ = true;
	jobs_.push_back(Job{job, std::move(done)});
	app::WakeIn(50);
	app::Redraw();
}

void Shell::PollJobs()
{
	if (jobs_.empty()) return;
	for (size_t i = 0; i < jobs_.size();)
	{
		char* res = nullptr;
		if (dos_job_poll(jobs_[i].id, &res) == 0)
		{
			i++;
			continue;
		}
		json::Value v = core::TakeJson(res);
		auto done = std::move(jobs_[i].done);
		jobs_.erase(jobs_.begin() + (long)i);
		overlay_ = false;
		try
		{
			if (done) done(v);
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Error: {0}", {ex.what()}), core::Sev::Error);
		}
		Refresh();
		app::Redraw();
	}
	if (!jobs_.empty()) app::WakeIn(50);
}

void Shell::Did(const std::string& message, core::Sev sev)
{
	core::Log(message, sev);
	Refresh();
}

// ================================================================ refresh

void Shell::OnCoreChanged()
{
	if (L::Reload() || lang_ != L::Lang())
	{
		bool first = lang_.empty();
		lang_ = L::Lang();
		if (!first) OnLanguage();
	}
	facts_ = core::TakeJson(dos_facts());
	std::string key = facts_["version"].Str() + "|" + facts_["cpu"].Str() + "|" + facts_["gpu"].Str() + "|" +
	                  std::to_string(facts_["logical"].Int()) + "|" + (facts_["hardware"].IsNull() ? "0" : "1");
	if (key != factsKey_)
	{
		bool hw = !facts_["hardware"].IsNull();
		factsKey_ = key;
		if (hw)
		{
			if (sysAdv_) sysAdv_->SetFacts(facts_);
			if (home_ && home_->SysPanel()) home_->SysPanel()->SetFacts(facts_);
		}
		SimpleTexts();
	}
}

void Shell::OnWindowState()
{
	dos_ui_visible(OnScreen() ? 1 : 0);
	if (!OnScreen() && home_) home_->SetVisible(false);
	if (!OnScreen()) HidePage(current_);
	if (OnScreen()) Refresh();
}

void Shell::Refresh()
{
	if (!OnScreen())
	{
		dos_note_ui_skipped();
		return;
	}
	auto t0 = std::chrono::steady_clock::now();
	RefreshCore();
	double ms = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
	dos_note_ui(ms);
	app::Redraw();
}

void Shell::Pill()
{
	int s = state_["state"].Int();
	if (s != dotState_)
	{
		dotState_ = s;
		const char* color = s == 1 ? "Warn" : s == 2 ? "Accent" : s == 3 ? "Dim" : s == 4 ? "Error" : "Faint";
		dotColor_ = color;
		haloColor_ = s == 2 ? "AccentSoft" : "None";
		wordColor_ = s == 0 ? "Dim" : color;
		pillWord_ = ui::Spaced(s == 1 ? L::T("Waiting for the game") : s == 2 ? L::T("Working") : s == 3 ? L::T("Putting things back") : s == 4 ? L::T("Error") : L::T("turned off"));
		pillLine_ = s == 2 ? "AccentLine" : "Line";
		if (s == 2) activeSince_ = NowSec();
	}
	std::string time;
	if (s == 2)
	{
		double age = state_["applied_age_s"].Num(-1);
		if (age < 0) age = NowSec() - activeSince_;   // AppliedAt is default or in the future: since we saw Active
		time = ui::Elapsed(age);
	}
	pillSep_ = time.empty() ? "" : "  |  ";
	pillTime_ = time;
	pillTip_ = state_["status"].Str();
}

void Shell::RefreshCore()
{
	state_ = core::TakeJson(dos_shell_state());
	Pill();
	bool autoOn = AutoOn();
	if (autoOn != closedShown_)
	{
		closedShown_ = autoOn;
		if (pages_[(size_t)current_]->ClosedInAuto() && autoOn) Show(0);
	}
	if (simple_)
	{
		if (simpleSettings_)
		{
			try
			{
				pages_[(size_t)PageId::Settings]->Tick();
			}
			catch (const std::exception& ex)
			{
				core::Log(L::F("Page {0}: {1}", {L::T("Settings"), ex.what()}), core::Sev::Error);
			}
		}
		else if (home_)
			home_->Tick();
		return;
	}
	int st = state_["state"].Int();
	stateBig_ = st == 1 ? L::T("Waiting") : st == 2 ? L::T("Active") : st == 3 ? L::T("Reverting") : st == 4 ? L::T("Error") : L::T("Off");
	stateBigBrush_ = st == 2 ? "AccentText" : st == 4 ? "Error" : "Strong";
	stateSub_ = st == 2 ? state_["game_exe"].Str() : st == 1 ? L::T("Start the game and keep it in front") : L::T("Press Turn on, then start the game");
	toggleOn_ = state_["on"].Bool();
	toggleEnabled_ = state_["button_enabled"].Bool(true);
	if (!toggleOn_) toggleSemi_ = true;   // Ui.Primary(_toggle, on: true) sets SemiBold and nothing clears it
	bool flag = st == 2;
	std::string blocked = state_["blocked"].Str();
	int num = state_["threads"].Int();
	bool flag2 = flag && state_["mask_now"].U64() != 0;
	bool flag3 = state_["background_count"].Int() > 0;
	bool flag4 = flag && state_["baseline_p99"].Num() > 0.0;
	steps_.clear();
	steps_.push_back({L::T("Game found"), flag, false});
	steps_.push_back({L::T("Cores assigned"), flag2, false});
	steps_.push_back({L::T("Threads placed"), flag && num > 0, false});
	steps_.push_back({L::T("Background lowered"), flag3, false});
	steps_.push_back({L::T("Frame time checked"), flag4, false});
	if (!blocked.empty()) steps_.push_back({blocked, false, true});
	else steps_.push_back({L::T("Nothing blocks the game"), false, false});
	nThreads_ = std::to_string(num);
	nLowered_ = std::to_string(state_["background_count"].Int());
	nChanged_ = std::to_string(state_["changed_count"].Int());
	try
	{
		pages_[(size_t)current_]->Tick();
	}
	catch (const std::exception& ex)
	{
		core::Log(L::F("Page {0}: {1}", {pages_[(size_t)current_]->Title(), ex.what()}), core::Sev::Error);
	}
}

// ================================================================ geometry

void Shell::RestoreGeometry()
{
	// WindowLeft / WindowTop are WPF device-independent units (of the system DPI) written with "0".
	float sys = (float)GetDpiForSystem() / 96.0f;
	std::string ls = core::Setting("WindowLeft"), ts = core::Setting("WindowTop");
	char* e1 = nullptr;
	char* e2 = nullptr;
	double l = ls.empty() ? 0 : strtod(ls.c_str(), &e1);
	double t = ts.empty() ? 0 : strtod(ts.c_str(), &e2);
	bool okL = !ls.empty() && e1 && *e1 == 0, okT = !ts.empty() && e2 && *e2 == 0;
	double vl = GetSystemMetrics(SM_XVIRTUALSCREEN) / sys, vt = GetSystemMetrics(SM_YVIRTUALSCREEN) / sys;
	double vw = GetSystemMetrics(SM_CXVIRTUALSCREEN) / sys, vh = GetSystemMetrics(SM_CYVIRTUALSCREEN) / sys;
	int px, py;
	if (okL && okT && l >= vl && t >= vt && l < vl + vw - 100.0 && t < vt + vh - 100.0)
	{
		px = (int)std::lround(l * sys);
		py = (int)std::lround(t * sys);
		SetWindowPos(hwnd_, nullptr, px, py, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
	}
	else
	{
		// WindowStartupLocation.CenterScreen: the middle of the work area of the monitor with the mouse.
		POINT cur;
		GetCursorPos(&cur);
		HMONITOR mon = MonitorFromPoint(cur, MONITOR_DEFAULTTOPRIMARY);
		MONITORINFO mi{};
		mi.cbSize = sizeof mi;
		GetMonitorInfoW(mon, &mi);
		RECT rc;
		GetWindowRect(hwnd_, &rc);
		int w = rc.right - rc.left, h = rc.bottom - rc.top;
		px = mi.rcWork.left + ((mi.rcWork.right - mi.rcWork.left) - w) / 2;
		py = mi.rcWork.top + ((mi.rcWork.bottom - mi.rcWork.top) - h) / 2;
		SetWindowPos(hwnd_, nullptr, px, py, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
	}
}

void Shell::SaveGeometry()
{
	if (IsZoomed(hwnd_) || IsIconic(hwnd_)) return;
	RECT rc;
	if (!GetWindowRect(hwnd_, &rc)) return;
	float sys = (float)GetDpiForSystem() / 96.0f;
	core::SetSetting("WindowLeft", std::to_string((long long)std::lround(rc.left / sys)));
	core::SetSetting("WindowTop", std::to_string((long long)std::lround(rc.top / sys)));
}

// ================================================================ drawing

void Shell::Draw()
{
	ImGuiViewport* vp = ImGui::GetMainViewport();
	ImGui::SetNextWindowPos(vp->Pos);
	ImGui::SetNextWindowSize(vp->Size);
	ImGui::Begin("##root", nullptr, ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoBringToFrontOnFocus | ImGuiWindowFlags_NoScrollWithMouse | ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoNav);
	ui::PushFont(ui::Face::Ui, ui::Weight::Normal, ui::FsBody);
	chrome::ClearHits();
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ImRect full(vp->Pos, vp->Pos + vp->Size);
	ui::AuroraBackground(dl, full);
	bool max = Maximized();
	ui::Thick pad = Padding();
	float border = (!max && !dwmBorder_) ? 1.0f : 0.0f;
	if (border > 0) ui::DrawEdges(dl, full, ui::C("Line"), ui::Thick(1));
	float bpx = border > 0 ? std::max(1.0f, std::round(ui::S(1))) : 0.0f;
	ImRect inner(full.Min + ImVec2(bpx + ui::S(pad.l), bpx + ui::S(pad.t)), full.Max - ImVec2(bpx + ui::S(pad.r), bpx + ui::S(pad.b)));
	ImRect header(inner.Min, ImVec2(inner.Max.x, inner.Min.y + std::round(ui::S(chrome::CaptionHeight))));
	ImRect body(ImVec2(inner.Min.x, header.Max.y), inner.Max);
	DrawHeader(header);
	if (simple_) DrawSimple(body);
	else DrawAdvanced(body);
	ui::PopFont();
	ImGui::End();
	if (overlay_) DrawOverlay(body);
}

void Shell::DrawHeader(ImRect r)
{
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ui::DrawEdges(dl, r, ui::C("Line"), ui::Thick(0, 0, 0, 1));
	float line = std::max(1.0f, std::round(ui::S(1)));
	float cy = r.Min.y + (r.GetHeight() - line) / 2;   // the items are centred in the row above the line
	// ---- brand: the logo, then "DOMAIN OS TOOLBOX" (spaced, invariant culture), mono 12.5 SemiBold Strong.
	float x = r.Min.x + ui::S(16);
	chrome::Logo(dl, ImVec2(std::round(x), std::round(cy - ui::S(9))), 18);
	x += ui::S(18) + ui::S(12);
	std::string brand = ui::Spaced("Domain OS Toolbox", true);
	ImVec2 bs = ui::Measure(ui::Face::Mono, ui::Weight::Semi, 12.5f, brand);
	ui::TextAt(dl, ImVec2(x, cy - bs.y / 2), ui::Face::Mono, ui::Weight::Semi, 12.5f, ui::C("Strong"), brand);
	x += bs.x;
	// ---- the right side, from the right edge: caption buttons, gear, mode switch, back.
	float capW = std::round(ui::S(44));
	float right = r.Max.x - 3 * capW;
	chrome::CapButton(hwnd_, chrome::Cap::Minimize, ImVec2(right, r.Min.y), L::T("Minimize"));
	chrome::CapButton(hwnd_, chrome::Cap::Maximize, ImVec2(right + capW, r.Min.y), Maximized() ? L::T("Restore") : L::T("Maximize"));
	chrome::CapButton(hwnd_, chrome::Cap::Close, ImVec2(right + 2 * capW, r.Min.y), L::T("Close to the tray"));
	right -= ui::S(10);   // the caption buttons' Margin left 10
	// gear: 32 x 30, no padding
	{
		ui::ButtonStyle gs;
		gs.width = 32;
		gs.height = 30;
		gs.padding = ui::Thick(0);
		gs.margin = ui::Thick(0);
		gs.fg = "Silver";
		gs.tip = nullptr;
		float gx = right - std::round(ui::S(32));
		ImGui::SetCursorScreenPos(ImVec2(gx, std::round(cy - ui::S(15))));
		bool clicked = ui::ButtonCustom("##gear", ImVec2(15, 15), gs, [&](ImRect in, bool) { Gear(dl, in, ui::C("Silver")); });
		ui::Tip(L::T("Settings"));
		chrome::AddHit(ui::LastRect());
		if (clicked)
		{
			if (simple_) ShowSimpleSettings(!simpleSettings_);
			else ShowPage(PageId::Settings);
		}
		right = gx;
	}
	// mode: the switch (26x14, margin right 9) and "Advanced mode"; padding 10,0,12,0, margin right 8, Silver.
	{
		std::string modeText = L::T("Advanced mode");
		ImVec2 ts = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsBody, modeText);
		float content = ui::S(26) + ui::S(9) + ts.x;
		float w = std::round(content + ui::S(10) + ui::S(12));
		right -= ui::S(8);
		float mx = right - w;
		ui::ButtonStyle ms;
		ms.height = 30;
		ms.padding = ui::Thick(10, 0, 12, 0);
		ms.margin = ui::Thick(0);
		ms.fg = "Silver";
		ImGui::SetCursorScreenPos(ImVec2(mx, std::round(cy - ui::S(15))));
		bool simple = simple_;
		bool clicked = ui::ButtonCustom("##mode", ImVec2(content / ui::Dpi(), ts.y / ui::Dpi()), ms, [&](ImRect in, bool) {
			float sy = std::round(in.GetCenter().y - ui::S(7));
			ImRect sw(ImVec2(std::round(in.Min.x), sy), ImVec2(std::round(in.Min.x + ui::S(26)), sy + std::round(ui::S(14))));
			ui::DrawBorder(dl, sw, 0, ui::C(simple ? "Dim" : "Accent"), 1, 2);
			float kb = std::max(1.0f, std::round(ui::S(1)));
			float kx = simple ? sw.Min.x + kb + ui::S(2) : sw.Max.x - kb - ui::S(2) - ui::S(8);
			float ky = std::round(sw.GetCenter().y - ui::S(4));
			dl->AddRectFilled(ImVec2(std::round(kx), ky), ImVec2(std::round(kx + ui::S(8)), ky + std::round(ui::S(8))), ui::C(simple ? "Dim" : "Accent"));
			ui::TextAt(dl, ImVec2(in.Min.x + ui::S(35), in.GetCenter().y - ts.y / 2), ui::Face::Ui, ui::Weight::Normal, ui::FsBody, ui::C("Silver"), modeText);
		});
		ui::Tip(simple_ ? L::T("Advanced mode") : L::T("Simple mode"));
		chrome::AddHit(ui::LastRect());
		if (clicked) SetSimple(!simple_);
		right = mx;
	}
	// back: only in the simple settings
	if (simple_ && simpleSettings_)
	{
		std::string t = "\xE2\x86\x90 " + L::T("Back");
		ImVec2 ts = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsBody, t);
		float w = std::round(ts.x + ui::S(24));
		right -= ui::S(8);
		float bx = right - w;
		ui::ButtonStyle bs2;
		bs2.height = 30;
		bs2.margin = ui::Thick(0);
		ImGui::SetCursorScreenPos(ImVec2(bx, std::round(cy - ui::S(15))));
		bool clicked = ui::Button(t + "##back", bs2);
		chrome::AddHit(ui::LastRect());
		if (clicked) ShowSimpleSettings(false);
		right = bx;
	}
	// ---- crumb (advanced only): "ADVANCED  /  " Dim, then the page title in Accent; margin left 28.
	if (!simple_)
	{
		x += ui::S(28);
		std::vector<ui::Run> runs(2);
		runs[0].text = ui::Spaced(L::T("Advanced")) + "  /  ";
		runs[0].brush = "Dim";
		runs[0].face = ui::Face::Mono;
		runs[0].weight = ui::Weight::Semi;
		runs[1].text = ui::Spaced(pages_[(size_t)current_]->Title());
		runs[1].brush = "Accent";
		runs[1].face = ui::Face::Mono;
		runs[1].weight = ui::Weight::Semi;
		ImVec2 cs = ui::MeasureRuns(runs, ui::FsSmall);
		ImGui::SetCursorScreenPos(ImVec2(x, cy - cs.y / 2));
		ui::Runs(runs, ui::FsSmall, {}, 100000);
		x += cs.x;
	}
	// ---- status pill: "OPTIMIZATION WORKING | 00:42:17" in the star column (margin 22,0,12,0, left).
	{
		float px0 = x + ui::S(22), px1 = right - ui::S(12);
		std::vector<ui::Run> runs(4);
		runs[0].text = ui::Spaced(L::T("Optimization")) + " ";
		runs[0].brush = "Dim";
		runs[1].text = pillWord_;
		runs[1].brush = wordColor_;
		runs[1].weight = ui::Weight::Semi;
		runs[2].text = pillSep_;
		runs[2].brush = "Faint";
		runs[3].text = pillTime_;
		runs[3].brush = "Silver";
		for (auto& rr : runs)
		{
			rr.face = ui::Face::Mono;
			if (rr.weight == ui::Weight::Normal) rr.weight = ui::Weight::Medium;
		}
		ImVec2 ts = ui::MeasureRuns(runs, ui::FsSmall);
		float bw = std::max(1.0f, std::round(ui::S(1)));
		float inH = std::max(ui::S(13), ts.y);
		float h = std::round(inH + ui::S(10) + 2 * bw);
		float w = std::round(bw + ui::S(10) + ui::S(13) + ui::S(8) + ts.x + ui::S(12) + bw);
		if (px1 > px0 + ui::S(40))
		{
			w = std::min(w, px1 - px0);
			ImRect pr(ImVec2(std::round(px0), std::round(cy - h / 2)), ImVec2(std::round(px0 + w), std::round(cy - h / 2) + h));
			ui::DrawBorder(dl, pr, 0, ui::C(pillLine_), 1, 3);
			float hx = pr.Min.x + bw + ui::S(10);
			float hy = std::round(pr.GetCenter().y - ui::S(6.5f));
			ImRect halo(ImVec2(std::round(hx), hy), ImVec2(std::round(hx + ui::S(13)), hy + std::round(ui::S(13))));
			dl->AddRectFilled(halo.Min, halo.Max, ui::C(haloColor_));
			ImVec2 dc = halo.GetCenter();
			float d = std::round(ui::S(7));
			ImVec2 d0(std::round(dc.x - d / 2), std::round(dc.y - d / 2));
			dl->AddRectFilled(d0, d0 + ImVec2(d, d), ui::C(dotColor_));
			float tx = halo.Max.x + ui::S(8);
			ImGui::SetCursorScreenPos(ImVec2(tx, pr.GetCenter().y - ts.y / 2));
			ui::Runs(runs, ui::FsSmall, {}, std::max(0.0f, (pr.Max.x - bw - ui::S(12) - tx) / ui::Dpi()));
			ui::TipRect(pr, pillTip_, ImGui::GetID("##pill"));
		}
	}
}

void Shell::DrawAdvanced(ImRect r)
{
	ImDrawList* dl = ImGui::GetWindowDrawList();
	// Native-only: in a window narrower than the start size (1340 DIP) the page list and the right column give up
	// some width (216 -> 180, 288 -> 240 at 1000 DIP and below), so the page keeps room for names.
	float wide = std::clamp((r.GetWidth() / ui::Dpi() - 1000.0f) / 340.0f, 0.0f, 1.0f);
	float sideW = std::round(ui::S(180 + 36 * wide)), rightW = std::round(ui::S(240 + 48 * wide));
	if (pages_[(size_t)current_]->Wide()) rightW = 0;   // native-only: a wide page (Thread map) takes the right column
	ImRect side(r.Min, ImVec2(r.Min.x + sideW, r.Max.y));
	ImRect rightCol(ImVec2(r.Max.x - rightW, r.Min.y), r.Max);
	ImRect page(ImVec2(side.Max.x, r.Min.y), ImVec2(rightCol.Min.x, r.Max.y));
	float line = std::max(1.0f, std::round(ui::S(1)));
	// ---- the page list with the machine at the bottom
	ui::DrawEdges(dl, side, ui::C("Line"), ui::Thick(0, 0, 1, 0));
	{
		ui::TextStyle fs;
		fs.size = ui::FsSmall * 0.95f;
		fs.brush = "Dim";
		fs.face = ui::Face::Mono;
		fs.wrap = true;
		fs.lineHeight = ui::FsSmall * 1.6f;
		float fx0 = side.Min.x + ui::S(18), fx1 = side.Max.x - line - ui::S(14);
		// Measure: the same wrap the text uses.
		float px = ui::FontPx(ui::Face::Mono, fs.size);
		float lh = std::max(ui::S(fs.lineHeight), px);
		ImFont* f = ui::Font(ui::Face::Mono, ui::Weight::Normal);
		int lines = 0;
		size_t start = 0;
		while (true)
		{
			size_t nl = foot_.find('\n', start);
			std::string part = foot_.substr(start, nl == std::string::npos ? std::string::npos : nl - start);
			const char* b = part.c_str();
			const char* e = b + part.size();
			if (b == e) lines++;
			while (b < e)
			{
				const char* brk = f->CalcWordWrapPosition(px, b, e, fx1 - fx0);
				if (brk == b) brk = b + 1;
				lines++;
				b = brk;
				while (b < e && *b == ' ') b++;
			}
			if (nl == std::string::npos) break;
			start = nl + 1;
		}
		float footH = ui::S(10) + lh * (float)lines + ui::S(14);
		ImRect foot(ImVec2(side.Min.x, side.Max.y - footH), ImVec2(side.Max.x - line, side.Max.y));
		ImGui::SetCursorScreenPos(ImVec2(fx0, foot.Min.y + ui::S(10)));
		{
			ui::Box b(fx0, fx1);
			ui::TextEx(foot_, fs);
		}
		DrawNav(ImRect(side.Min, ImVec2(side.Max.x - line, foot.Min.y)));
	}
	// ---- the page: padding 20,14,20,8
	DrawPage(ImRect(page.Min + ui::S(20, 14), page.Max - ui::S(20, 8)));
	// ---- the right column: Panel, a line at the left, padding 16,14,12,12
	if (rightW <= 0) return;
	dl->AddRectFilled(rightCol.Min, rightCol.Max, ui::C("Panel"));
	ui::DrawEdges(dl, rightCol, ui::C("Line"), ui::Thick(1, 0, 0, 0));
	ImRect in(rightCol.Min + ImVec2(line + ui::S(16), ui::S(14)), rightCol.Max - ui::S(12, 12));
	float used = 0;
	if (std::string(pages_[(size_t)current_]->Section()) == "Scheduler") DrawStatus(in, used);
	ImRect sys(ImVec2(in.Min.x, in.Min.y + used), in.Max);
	if (sys.GetHeight() > 1 && sysAdv_)
	{
		ImGui::SetCursorScreenPos(sys.Min);
		ImGui::BeginChild("##sysadv", sys.GetSize(), ImGuiChildFlags_None, ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoScrollbar | ImGuiWindowFlags_NoScrollWithMouse);
		try
		{
			sysAdv_->Draw(ImRect(sys.Min, sys.Max));
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Error: {0}", {ex.what()}), core::Sev::Error);
		}
		ImGui::EndChild();
	}
}

void Shell::DrawStatus(ImRect r, float& usedH)
{
	ImGui::SetCursorScreenPos(r.Min);
	ImVec2 start = r.Min;
	{
		ui::Box box(r.Min.x, r.Max.x);
		ui::Caps(L::T("Status"));
		ui::TextStyle big;
		big.size = ui::FsBody * 1.6f;
		big.weight = ui::Weight::Semi;
		big.brush = stateBigBrush_;
		big.margin = ui::Thick(0, 3, 0, 1);
		ui::TextEx(stateBig_, big);
		ui::Text(stateSub_, ui::FsNote, "Dim", false, true, ui::Thick(0, 0, 0, 10));
		ui::ButtonStyle tg;
		tg.width = -1;
		tg.height = 34;
		tg.margin = ui::Thick(0);
		tg.weight = toggleSemi_ ? ui::Weight::Semi : ui::Weight::Medium;
		tg.enabled = toggleEnabled_;
		ui::Primary(tg, !toggleOn_);
		if (!toggleSemi_) tg.weight = ui::Weight::Medium;
		if (ui::Button((toggleOn_ ? L::T("Turn off") : L::T("Turn on")) + "##toggle", tg))
		{
			toggleEnabled_ = false;
			dos_auto_click();
		}
		ui::ButtonStyle ud;
		ud.width = -1;
		ud.height = 30;
		ud.margin = ui::Thick(0, 6, 0, 0);
		if (ui::Button(L::T("Undo everything") + "##undo", ud)) UndoEverything();
		ui::Caps(L::T("Steps done"), 0, "Dim", true, ui::Thick(0, 14, 0, 4));
		ImDrawList* dl = ImGui::GetWindowDrawList();
		for (const Step& s : steps_)
		{
			// The box: 14x14, radius 2, line 1, margin right 9, centred; a check mark or "!" inside.
			ImVec2 p = ImGui::GetCursorScreenPos();
			float tx = p.x + ui::S(14) + ui::S(9);
			ui::TextStyle ts;
			ts.size = ui::FsNote;
			ts.brush = s.done ? "Text" : (s.warn ? "Warn" : "Dim");
			ts.wrap = true;
			ts.margin = ui::Thick(0, 3, 0, 3);
			ImGui::SetCursorScreenPos(ImVec2(tx, p.y));
			ui::TextEx(s.text, ts);
			ImRect tr = ui::LastRect();
			float by = std::round(tr.GetCenter().y - ui::S(7));
			ImRect bx(ImVec2(std::round(p.x), by), ImVec2(std::round(p.x + ui::S(14)), by + std::round(ui::S(14))));
			ui::DrawBorder(dl, bx, ui::C(s.done ? "Accent" : "None"), ui::C(s.done ? "Accent" : (s.warn ? "Warn" : "Line")), 1, 2);
			const char* mark = s.done ? "\xE2\x9C\x93" : (s.warn ? "!" : "");
			if (*mark)
			{
				ImVec2 ms = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.85f, mark);
				ui::TextAt(dl, bx.GetCenter() - ms * 0.5f, ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.85f, ui::C(s.done ? "Ink" : "Warn"), mark);
			}
			ImGui::SetCursorScreenPos(ImVec2(p.x, tr.Max.y + ui::S(3)));
			ImGui::Dummy(ImVec2(0, 0));
		}
		ui::Caps(L::T("Changed right now"), 0, "Dim", true, ui::Thick(0, 12, 0, 0));
		ui::Kv(L::T("Threads placed"), nThreads_);
		ui::Kv(L::T("Programs lowered"), nLowered_);
		ui::Kv(L::T("Programs changed"), nChanged_);
	}
	float y = ui::LastRect().Max.y;
	float contentEnd = std::max(y, ImGui::GetCursorScreenPos().y);
	// The box: a line below after a 12 padding, then a 12 margin.
	float lineY = std::round(contentEnd + ui::S(12));
	float line = std::max(1.0f, std::round(ui::S(1)));
	ImGui::GetWindowDrawList()->AddRectFilled(ImVec2(r.Min.x, lineY), ImVec2(r.Max.x, lineY + line), ui::C("Line"));
	usedH = (lineY + line + ui::S(12)) - start.y;
}

void Shell::DrawNav(ImRect r)
{
	ImGui::SetCursorScreenPos(r.Min);
	ImGui::BeginChild("##nav", r.GetSize(), ImGuiChildFlags_None, ImGuiWindowFlags_NoBackground);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float right = ImGui::GetCurrentWindow()->WorkRect.Max.x;
	float x0 = ImGui::GetCursorScreenPos().x;
	ui::Space(6);
	std::string current = pages_[(size_t)current_]->Section();
	int number = 0;
	bool autoOn = AutoOn();
	int clicked = -1;
	for (const char* section : kSections)
	{
		bool any = false;
		for (auto& p : pages_) any |= std::string(p->Section()) == section;
		if (!any) continue;
		ui::Caps(SectionTitle(section), ui::FsSmall * 0.9f, current == section ? "Silver" : "Faint", true, ui::Thick(18, number == 0 ? 10.0f : 16.0f, 12, 6));
		for (int i = 0; i < (int)pages_.size(); i++)
		{
			if (std::string(pages_[(size_t)i]->Section()) != section || (pages_[(size_t)i]->ClosedInAuto() && autoOn)) continue;
			number++;
			bool on = i == current_;
			ImGui::PushID(i);
			ImGuiID id = ImGui::GetID("##nav");
			ImVec2 p = ImGui::GetCursorScreenPos();
			ImRect row(ImVec2(x0, p.y), ImVec2(right, p.y + std::round(ui::S(34))));
			ImGui::ItemSize(row.GetSize());
			ImGui::ItemAdd(row, id);
			bool hov = false, held = false;
			if (ImGui::ButtonBehavior(row, id, &hov, &held, ImGuiButtonFlags_PressedOnRelease)) clicked = i;
			if (hov) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
			if (on) dl->AddRectFilled(row.Min, row.Max, ui::C("Sel"));
			else if (hov) dl->AddRectFilled(row.Min, row.Max, ui::C("Hover"));
			if (on) dl->AddRectFilled(row.Min, ImVec2(row.Min.x + std::round(ui::S(2)), row.Max.y), ui::C("Accent"));
			char num[8];
			snprintf(num, sizeof num, "%02d", number);
			float npx = ui::FontPx(ui::Face::Mono, ui::FsSmall);
			ui::TextAt(dl, ImVec2(row.Min.x + ui::S(18), row.GetCenter().y - npx / 2), ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, ui::C(on ? "Accent" : "Faint"), num);
			float tpx = ui::FontPx(ui::Face::Ui, ui::FsBody);
			float tx = row.Min.x + ui::S(46);
			ui::TextAt(dl, ImVec2(tx, row.GetCenter().y - tpx / 2), ui::Face::Ui, on ? ui::Weight::Medium : ui::Weight::Normal, ui::FsBody, ui::C(on ? "Strong" : "Text"),
			             pages_[(size_t)i]->Title(), std::max(0.0f, row.Max.x - ui::S(10) - tx));
			ImGui::PopID();
		}
	}
	ui::Space(10);
	ImGui::EndChild();
	if (clicked >= 0) Show(clicked);
}

void Shell::DrawPage(ImRect r)
{
	if (r.GetWidth() < 1 || r.GetHeight() < 1) return;
	ImGui::SetCursorScreenPos(r.Min);
	ImGui::PushID(pageBuiltFor_);
	ImGui::BeginChild("##page", r.GetSize(), ImGuiChildFlags_None, ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoScrollbar | ImGuiWindowFlags_NoScrollWithMouse);
	Page* p = simple_ && simpleSettings_ ? pages_[(size_t)PageId::Settings].get() : pages_[(size_t)current_].get();
	if (!pageError_.empty())
	{
		ui::Box b(r.Min.x, r.Max.x);
		ui::Text(pageError_, 0, "Error", false, true);
	}
	else
	{
		try
		{
			p->Draw(r);
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Page {0}: {1}", {p->Title(), ex.what()}), core::Sev::Error);
			pageError_ = ex.what();
		}
	}
	ImGui::EndChild();
	ImGui::PopID();
}

void Shell::DrawSimple(ImRect r)
{
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float barH = std::round(ui::S(36));
	ImRect host(r.Min, ImVec2(r.Max.x, r.Max.y - barH));
	ImRect bar(ImVec2(r.Min.x, r.Max.y - barH), r.Max);
	if (simpleSettings_) DrawPage(ImRect(host.Min + ui::S(24, 16), host.Max - ui::S(24, 6)));
	else if (home_)
	{
		ImGui::SetCursorScreenPos(host.Min);
		ImGui::BeginChild("##home", host.GetSize(), ImGuiChildFlags_None, ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoScrollbar | ImGuiWindowFlags_NoScrollWithMouse);
		try
		{
			home_->Draw(host);
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Error: {0}", {ex.what()}), core::Sev::Error);
		}
		ImGui::EndChild();
	}
	// ---- the footer: a line on top; margin 16,0,12,0; "Undo everything" at the right, the machine in the rest.
	ui::DrawEdges(dl, bar, ui::C("Line"), ui::Thick(0, 1, 0, 0));
	float line = std::max(1.0f, std::round(ui::S(1)));
	float cy = bar.Min.y + line + (bar.GetHeight() - line) / 2;
	std::string ut = L::T("Undo everything");
	ImVec2 uts = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ut);
	float content = ui::S(14) + ui::S(7) + uts.x;
	float w = std::round(content + ui::S(10) + ui::S(12));
	float ux = bar.Max.x - ui::S(12) - w;
	ui::ButtonStyle us;
	us.height = 26;
	us.padding = ui::Thick(10, 0, 12, 0);
	us.margin = ui::Thick(0);
	us.line = "AccentLine";
	us.fg = "Strong";
	ImGui::SetCursorScreenPos(ImVec2(ux, std::round(cy - ui::S(13))));
	bool clicked = ui::ButtonCustom("##simpleundo", ImVec2(content / ui::Dpi(), uts.y / ui::Dpi()), us, [&](ImRect in, bool) {
		ImVec2 io(std::round(in.Min.x), std::round(in.GetCenter().y - ui::S(7)));
		UndoIcon(dl, io, ui::C("Accent"));
		ui::TextAt(dl, ImVec2(in.Min.x + ui::S(21), in.GetCenter().y - uts.y / 2), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C("Strong"), ut);
	});
	if (clicked) UndoEverything();
	float fx = bar.Min.x + ui::S(16);
	float fpx = ui::FontPx(ui::Face::Mono, ui::FsSmall);
	ImRect fr(ImVec2(fx, cy - fpx / 2), ImVec2(ux, cy + fpx / 2));
	ui::TextAt(dl, fr.Min, ui::Face::Mono, ui::Weight::Medium, ui::FsSmall, ui::C("Dim"), simpleFoot_, std::max(0.0f, fr.GetWidth()));
	ui::TipRect(fr, simpleFootTip_, ImGui::GetID("##simplefoot"));
}

void Shell::DrawOverlay(ImRect body)
{
	ImGui::SetNextWindowPos(body.Min);
	ImGui::SetNextWindowSize(body.GetSize());
	ImGui::PushStyleColor(ImGuiCol_WindowBg, ui::C("Overlay"));
	ImGui::Begin("##overlay", nullptr, ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoNav | ImGuiWindowFlags_NoFocusOnAppearing);
	ImGui::BringWindowToDisplayFront(ImGui::GetCurrentWindow());
	ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsBody, overlayText_);
	ui::TextAt(ImGui::GetWindowDrawList(), body.GetCenter() - ts * 0.5f, ui::Face::Mono, ui::Weight::Normal, ui::FsBody, ui::C("Text"), overlayText_);
	ImGui::End();
	ImGui::PopStyleColor();
}
