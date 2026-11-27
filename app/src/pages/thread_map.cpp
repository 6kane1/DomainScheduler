// Thread map (native-only page of the advanced layout, "Карта потоков"): the game's processes and threads as a live
// graph with who wakes whom, who waits on whose lock, who takes whose core and who calls whom, over a 30 s history.
// The logic is the threadmap crate (threadmap/threadmap.h, linked into the core); the core runs its kernel trace only
// while this page is on screen (dos_tm_open / dos_tm_close, core/src/ffi/thread_map.rs). This file only draws: it
// polls tm_snapshot_json about 10 times a second, steps the layout every frame and animates the links.
// Visual reference: the owner-approved ThreadMap-reference.html (left filters and legends, the canvas, the selected
// node on the right, the time strip at the bottom).
#include <windows.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <set>
#include <string>
#include <unordered_map>
#include <vector>

#include "app.h"
#include "core.h"
#include "dos_core.h"
#include "json.h"
#include "page.h"
#include "threadmap.h"
#include "ui.h"
#include "widgets.h"

namespace {

// Link and state colours of the reference (wake = the theme's accent, lock = amber, preempt = red, ipc = blue,
// running = green, sleeping = faint).
ImU32 ColWake() { return ui::C("Accent"); }
const ImU32 kLock = IM_COL32(227, 179, 65, 255);
const ImU32 kPreempt = IM_COL32(255, 107, 129, 255);
const ImU32 kIpc = IM_COL32(127, 178, 255, 255);
const ImU32 kRun = IM_COL32(139, 214, 160, 255);

enum Kind { KWake = 0, KPreempt = 1, KLock = 2, KIpc = 3 };

ImU32 Alpha(ImU32 c, float a)
{
	int al = (int)((c >> IM_COL32_A_SHIFT) & 0xFF);
	return (c & ~IM_COL32_A_MASK) | ((ImU32)std::clamp((int)std::lround(al * a), 0, 255) << IM_COL32_A_SHIFT);
}

ImU32 KindCol(int k) { return k == KWake ? ColWake() : k == KLock ? kLock : k == KPreempt ? kPreempt : kIpc; }

int KindOf(const std::string& s) { return s == "wake" ? KWake : s == "preempt" ? KPreempt : s == "lock" ? KLock : KIpc; }

struct NodeV {
	uint64_t id = 0;
	std::string kind, cls, label, image, state, reason;
	uint64_t parent = 0;
	uint32_t pid = 0, tid = 0;
	int count = 0, alive = -1, prio = -1000, core = -1;
	bool expanded = false;
	double cpu = 0, p99 = -1;
	float r = 14;
	bool hidden = false;
};

struct LinkV {
	uint64_t from = 0, to = 0;
	int kind = 0;
	double rate = 0;
};

ImVec2 Bez(ImVec2 a, ImVec2 c, ImVec2 b, float t)
{
	float u = 1 - t;
	return a * (u * u) + c * (2 * u * t) + b * (t * t);
}

/// A quadratic curve sampled into a polyline with arc lengths (for dashes and dots that move along it).
struct Curve {
	static constexpr int N = 28;
	ImVec2 p[N + 1];
	float len[N + 1];
	void Make(ImVec2 a, ImVec2 c, ImVec2 b)
	{
		for (int i = 0; i <= N; i++) p[i] = Bez(a, c, b, (float)i / N);
		len[0] = 0;
		for (int i = 1; i <= N; i++) len[i] = len[i - 1] + std::sqrt(ImLengthSqr(p[i] - p[i - 1]));
	}
	float Total() const { return len[N]; }
	ImVec2 At(float s) const
	{
		s = std::clamp(s, 0.0f, len[N]);
		int i = 1;
		while (i < N && len[i] < s) i++;
		float seg = len[i] - len[i - 1];
		float f = seg > 0 ? (s - len[i - 1]) / seg : 0;
		return p[i - 1] + (p[i] - p[i - 1]) * f;
	}
	/// Dashes of `dash` px every `period` px, shifted by `offset` px along the curve (moving dashes).
	void Dashes(ImDrawList* dl, ImU32 col, float thick, float dash, float period, float offset) const
	{
		float L = Total();
		float start = std::fmod(offset, period);
		if (start > 0) start -= period;
		for (float s = start; s < L; s += period)
		{
			float s0 = std::max(0.0f, s), s1 = std::min(L, s + dash);
			if (s1 <= s0) continue;
			ImVec2 pts[8];
			int n = 0;
			pts[n++] = At(s0);
			for (int i = 1; i < N && n < 7; i++)
				if (len[i] > s0 && len[i] < s1) pts[n++] = p[i];
			pts[n++] = At(s1);
			dl->AddPolyline(pts, n, col, ImDrawFlags_None, thick);
		}
	}
	void Dots(ImDrawList* dl, ImU32 col, float radius, float period, float offset) const
	{
		float L = Total();
		float start = std::fmod(offset, period);
		if (start > 0) start -= period;
		for (float s = start; s < L; s += period)
			if (s >= 0) dl->AddCircleFilled(At(s), radius, col, 8);
	}
};

std::string Fixed(double v, int decimals) { return ui::Num(v, decimals); }

class ThreadMapPage : public Page {
public:
	std::string Title() override { return L::T("Thread map"); }
	bool Wide() override { return true; }

	void Build() override
	{
		lastPoll_ = -1;
		fitted_ = false;
	}

	void Hidden() override
	{
		if (open_) dos_tm_close();
		open_ = false;
	}

	~ThreadMapPage() override { Hidden(); }

	void Draw(ImRect area) override
	{
		ImGuiIO& io = ImGui::GetIO();
		float dt = std::clamp(io.DeltaTime, 0.0f, 0.25f);
		animT_ += dt;
		if (!open_) Open();
		if (lastPoll_ < 0 || animT_ - lastPoll_ >= 0.1)
		{
			lastPoll_ = animT_;
			Poll();
		}
		Physics(dt);
		// The side columns give way in a narrow window so the canvas keeps at least about half of the width.
		float leftW = std::round(std::clamp(area.GetWidth() * 0.21f, ui::S(172), ui::S(206)));
		float rightW = std::round(std::clamp(area.GetWidth() * 0.24f, ui::S(196), ui::S(244)));
		float stripH = std::round(ui::S(84));
		float gap = std::round(ui::S(10));
		ImRect body(area.Min, ImVec2(area.Max.x, area.Max.y - stripH - gap));
		ImRect left(body.Min, ImVec2(body.Min.x + leftW, body.Max.y));
		ImRect right(ImVec2(body.Max.x - rightW, body.Min.y), body.Max);
		ImRect canvas(ImVec2(left.Max.x + gap, body.Min.y), ImVec2(right.Min.x - gap, body.Max.y));
		ImRect strip(ImVec2(area.Min.x, area.Max.y - stripH), area.Max);
		DrawLeft(left);
		DrawCanvas(canvas);
		DrawRight(right);
		DrawStrip(strip, ImRect(ImVec2(canvas.Min.x, strip.Min.y), ImVec2(canvas.Max.x, strip.Max.y)), leftW, rightW);
		// The animation runs at frame rate while the page is on screen (and only then: the trace too).
		app::WakeIn(16);
	}

private:
	// ---------------------------------------------------------------- data
	void Open()
	{
		// The synthetic feed only with DOS_TM_DEMO=1, or in the screenshot build unless DOS_TM_DEMO=0; never otherwise.
		char v[8] = {};
		bool set = GetEnvironmentVariableA("DOS_TM_DEMO", v, sizeof v) > 0;
#ifdef DOS_UI_SHOT
		bool demo = !set || v[0] != '0';
#else
		bool demo = set && v[0] == '1';
#endif
		map_ = (tm_map*)dos_tm_open(demo ? 1 : 0);
		open_ = true;
		lastPoll_ = -1;
	}

	std::string Query() const
	{
		bool focus = fGame_ && gamePid_ != 0;
		std::string q = "{\"end_age\":" + std::to_string(endAge_) + ",\"span\":10";
		if (focus) q += ",\"focus_pid\":" + std::to_string(gamePid_) + ",\"neighbours\":" + std::string(fBg_ || fSys_ ? "true" : "false");
		q += ",\"hide_system\":" + std::string(fSys_ ? "false" : "true");
		q += ",\"merge\":" + std::string(fMerge_ ? "true" : "false");
		q += ",\"max_nodes\":" + std::string(focus ? "120" : "60");
		q += ",\"expand\":[";
		bool first = true;
		for (uint32_t p : expand_)
		{
			if (!first) q += ",";
			q += std::to_string(p);
			first = false;
		}
		q += "]}";
		return q;
	}

	void Poll()
	{
		status_ = core::TakeJson(dos_tm_status());
		uint32_t g = (uint32_t)status_["game_pid"].Num();
		if (g != gamePid_)
		{
			gamePid_ = g;
			if (g != 0) expand_.insert(g);   // the game opens into its threads, once (the user may collapse it)
		}
		if (!map_ || paused_) return;
		Snapshot();
		if (++seriesTick_ % 2 == 0 || series_.IsNull()) TakeSeries();
	}

	void TakeSeries()
	{
		char* s = tm_series_json(map_);
		if (s)
		{
			series_ = json::Parse(s);
			tm_free_str(s);
		}
	}

	void Snapshot()
	{
		if (!map_) return;
		std::string q = Query();
		char* s = tm_snapshot_json(map_, q.c_str());
		if (!s) return;
		json::Value v = json::Parse(s);
		tm_free_str(s);
		window_ = v["window"];
		nodes_.clear();
		links_.clear();
		for (const json::Value& n : v["nodes"].Arr())
		{
			NodeV x;
			x.id = (uint64_t)n["id"].Num();
			x.kind = n["kind"].Str();
			x.cls = n["class"].Str();
			x.label = n["label"].Str();
			x.image = n["image"].Str();
			x.state = n["state"].Str();
			x.reason = n["wait_reason_name"].Str();
			x.parent = (uint64_t)n["parent"].Num();
			x.pid = (uint32_t)n["pid"].Num();
			x.tid = (uint32_t)n["tid"].Num();
			x.count = n["count"].Int();
			x.alive = n["threads_alive"].IsNull() ? -1 : n["threads_alive"].Int();
			x.prio = n["prio"].IsNull() ? -1000 : n["prio"].Int();
			x.core = n["core"].IsNull() ? -1 : n["core"].Int();
			x.expanded = n["expanded"].Bool();
			x.cpu = n["cpu"].Num();
			x.p99 = n["wait_p99_us"].IsNull() ? -1 : n["wait_p99_us"].Num();
			x.r = (float)n["r"].Num(14);
			// Filters the query has no switch for: the game itself, background programs.
			x.hidden = (!fGame_ && x.cls == "game") || (!fBg_ && x.cls == "background" && fGame_ && gamePid_ != 0);
			nodes_.push_back(std::move(x));
		}
		index_.clear();
		for (size_t i = 0; i < nodes_.size(); i++) index_[nodes_[i].id] = i;
		for (const json::Value& l : v["links"].Arr())
		{
			LinkV x;
			x.from = (uint64_t)l["from"].Num();
			x.to = (uint64_t)l["to"].Num();
			x.kind = KindOf(l["kind"].Str());
			x.rate = l["rate"].Num();
			links_.push_back(x);
		}
		if (!fitted_ && !nodes_.empty())
		{
			tm_layout_warm(map_, 260);
			needFit_ = true;
			fitted_ = true;
		}
		if (sel_ == 0 || !index_.count(sel_)) PickDefault();
	}

	void PickDefault()
	{
		sel_ = 0;
		double best = -1;
		for (const NodeV& n : nodes_)
			if (!n.hidden && n.kind == "thread" && n.cls == "game" && n.cpu > best)
			{
				best = n.cpu;
				sel_ = n.id;
			}
		if (sel_ == 0)
			for (const NodeV& n : nodes_)
				if (!n.hidden && !n.expanded)
				{
					sel_ = n.id;
					break;
				}
	}

	const NodeV* Node(uint64_t id) const
	{
		auto it = index_.find(id);
		return it == index_.end() ? nullptr : &nodes_[it->second];
	}

	// ---------------------------------------------------------------- layout and view
	void Physics(float dt)
	{
		if (!map_) return;
		if (!paused_) tm_layout_step(map_, dt > 0 ? dt : 1.0f / 60);
		size_t n = tm_layout_count(map_);
		pos_.resize(n);
		n = tm_layout_positions(map_, pos_.data(), n);
		pos_.resize(n);
		float a = 1 - std::exp(-dt * 10.0f);   // displayed positions follow the layout smoothly, never jump
		for (const tm_pos& p : pos_)
		{
			auto it = disp_.find(p.id);
			if (it == disp_.end()) disp_[p.id] = ImVec2(p.x, p.y);
			else if (p.id == drag_) it->second = ImVec2(p.x, p.y);
			else it->second += (ImVec2(p.x, p.y) - it->second) * a;
		}
		if (disp_.size() > 4 * pos_.size() + 256)
		{
			std::unordered_map<uint64_t, ImVec2> keep;
			for (const tm_pos& p : pos_) keep[p.id] = disp_[p.id];
			disp_.swap(keep);
		}
	}

	float Scale() const { return k_ * ui::Dpi(); }
	ImVec2 ToScreen(ImVec2 p) const { return center_ + (p + pan_) * Scale(); }
	ImVec2 ToLayout(ImVec2 s) const { return (s - center_) / Scale() - pan_; }

	ImVec2 Disp(uint64_t id) const
	{
		auto it = disp_.find(id);
		return it == disp_.end() ? ImVec2(0, 0) : it->second;
	}

	void Fit(ImRect canvas)
	{
		if (pos_.empty()) return;
		ImVec2 mn(1e9f, 1e9f), mx(-1e9f, -1e9f);
		for (const tm_pos& p : pos_)
		{
			const NodeV* n = Node(p.id);
			if (n && n->hidden) continue;
			mn = ImMin(mn, ImVec2(p.x - p.r, p.y - p.r));
			mx = ImMax(mx, ImVec2(p.x + p.r, p.y + p.r + 16));
		}
		if (mn.x > mx.x) return;
		ImVec2 size = mx - mn;
		float sx = canvas.GetWidth() * 0.86f / std::max(1.0f, size.x * ui::Dpi());
		float sy = canvas.GetHeight() * 0.80f / std::max(1.0f, size.y * ui::Dpi());
		k_ = std::clamp(std::min(sx, sy), 0.45f, 1.5f);
		pan_ = -(mn + mx) * 0.5f;
	}

	/// The node under a screen point (not halos, not hidden ones), topmost first.
	uint64_t HitTest(ImVec2 m) const
	{
		for (size_t i = nodes_.size(); i-- > 0;)
		{
			const NodeV& n = nodes_[i];
			if (n.hidden || n.expanded) continue;
			ImVec2 s = ToScreen(Disp(n.id));
			float r = n.r * Scale() + ui::S(4);
			if (ImLengthSqr(m - s) <= r * r) return n.id;
		}
		return 0;
	}

	void ToggleExpand(uint64_t id)
	{
		const NodeV* n = Node(id);
		if (!n) return;
		if (n->kind == "process" && !n->expanded)
		{
			if (n->alive == 0 || n->pid == 0xFFFFFFFEu || n->pid == 0xFFFFFFFFu) return;
			expand_.insert(n->pid);
		}
		else expand_.erase(n->pid);
		Snapshot();
	}

	// ---------------------------------------------------------------- left column
	void DrawLeft(ImRect r)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		ui::DrawBorder(dl, r, ui::C("Panel"), ui::C("Line"), 1, 8);
		// Scrolls when the window is too low for the filters, legends and buttons (never over the time strip).
		ImGui::SetCursorScreenPos(r.Min + ui::S(14, 12));
		ui::BeginScroll("##tmleft", ImVec2(r.GetWidth() - ui::S(20), r.GetHeight() - ui::S(18)));
		DrawLeftBody();
		ui::EndScroll();
	}

	void DrawLeftBody()
	{
		ui::Box box(ImGui::GetCursorScreenPos().x, ui::BoxRight() - ui::S(6));
		ui::Caps(L::T("Show"), 0, "Faint", true, ui::Thick(0, 0, 0, 6));
		std::string game = status_["game_exe"].Str();
		bool changed = false;
		changed |= ui::CheckRow("##tmgame", L::T("The game and its links"),
		                        gamePid_ != 0 ? L::F("{0}, who wakes it and who gets in its way", {game.empty() ? std::string("PID ") + std::to_string(gamePid_) : game})
		                                      : L::T("No game right now: the busiest programs are shown"),
		                        fGame_);
		changed |= ui::CheckRow("##tmbg", L::T("Background programs"), L::T("only those linked to the game"), fBg_);
		changed |= ui::CheckRow("##tmsys", L::T("System"), L::T("Windows services, drivers"), fSys_);
		changed |= ui::CheckRow("##tmmerge", L::T("Merge worker threads"), L::T("Worker x12 instead of 12 nodes"), fMerge_);
		if (changed) Snapshot();
		ui::Caps(L::T("Links"), 0, "Faint", true, ui::Thick(0, 12, 0, 6));
		LegendLine(KWake, L::T("wakes (A hands work to B)"));
		LegendLine(KLock, L::T("waits for a lock"));
		LegendLine(KPreempt, L::T("takes the core"));
		LegendLine(KIpc, L::T("calls (ALPC, pipe)"));
		ui::Caps(L::T("Node state"), 0, "Faint", true, ui::Thick(0, 12, 0, 6));
		LegendRing(kRun, L::T("running (ring = load)"));
		LegendRing(ui::C("Faint"), L::T("sleeping"));
		LegendRing(kLock, L::T("waits for another thread"));
		LegendRing(kPreempt, L::T("ready, but the core is busy"));
		ui::Space(12);
		ui::ButtonStyle bs = ui::BtnStyle(0);
		bs.width = -1;
		bs.height = 28;
		bs.margin = ui::Thick(0, 0, 0, 6);
		if (ui::Button(L::T("Lay out again") + "##tmrelayout", bs) && map_)
		{
			tm_layout_relayout(map_);
			tm_layout_warm(map_, 260);
			needFit_ = true;
		}
		if (ui::Button(L::T("Collapse all") + "##tmcollapse", bs))
		{
			expand_.clear();
			Snapshot();
		}
	}

	void LegendLine(int kind, const std::string& text)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float px = ui::FontPx(ui::Face::Ui, ui::FsNote);
		float tw = ui::AvailW() - ui::S(39);
		ImVec2 ts = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsNote, text, tw);
		ImRect r = ui::Place(ImVec2(ui::AvailW(), std::max(px, ts.y) + ui::S(6)));
		float y = r.Min.y + ui::S(3) + px / 2, x0 = r.Min.x, x1 = r.Min.x + ui::S(30);
		Curve c;
		c.Make(ImVec2(x0, y), ImVec2((x0 + x1) / 2, y), ImVec2(x1, y));
		ImU32 col = KindCol(kind);
		float t = ui::S(kind == KWake ? 2.5f : 2.0f);
		if (kind == KLock) c.Dashes(dl, col, t, ui::S(4), ui::S(8), animT_ * ui::S(12));
		else if (kind == KIpc) c.Dots(dl, col, ui::S(1.2f), ui::S(5), animT_ * ui::S(12));
		else dl->AddLine(ImVec2(x0, y), ImVec2(x1, y), col, t);
		ui::TextAt(dl, ImVec2(x1 + ui::S(9), r.Min.y + ui::S(3)), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C("Dim"), text, 0, tw);
	}

	void LegendRing(ImU32 col, const std::string& text)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float h = ui::FontPx(ui::Face::Ui, ui::FsNote) + ui::S(6);
		ImRect r = ui::Place(ImVec2(ui::AvailW(), h));
		ImVec2 c(r.Min.x + ui::S(7), r.GetCenter().y);
		dl->AddCircle(c, ui::S(5), col, 20, ui::S(2));
		ui::TextAt(dl, ImVec2(r.Min.x + ui::S(23), c.y - ui::FontPx(ui::Face::Ui, ui::FsNote) / 2), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C("Dim"), text, r.Max.x - r.Min.x - ui::S(23));
	}

	// ---------------------------------------------------------------- canvas
	ImU32 StateCol(const NodeV& n) const
	{
		if (n.state == "running") return kRun;
		if (n.state == "ready") return kPreempt;
		if (n.state == "wait_lock" || n.state == "wait_ipc") return kLock;
		return ui::C("Faint");
	}

	void DrawCanvas(ImRect cv)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		ui::DrawBorder(dl, cv, ui::C("Well", 0.55f), ui::C("Line"), 1, 8);
		center_ = cv.GetCenter();
		if (needFit_ && !pos_.empty())
		{
			Fit(cv);
			needFit_ = false;
		}
		ImGui::SetCursorScreenPos(cv.Min);
		ImGui::InvisibleButton("##tmcanvas", cv.GetSize(), ImGuiButtonFlags_MouseButtonLeft);
		bool hovered = ImGui::IsItemHovered();
		Interact(cv, hovered);
		dl->PushClipRect(cv.Min + ui::S(1, 1), cv.Max - ui::S(1, 1), true);
		// A faint grid that moves with the view (the reference's 40 px grid).
		float gs = 40 * Scale();
		if (gs > 8)
		{
			ImU32 gc = IM_COL32(255, 255, 255, 9);
			ImVec2 o = ToScreen(ImVec2(0, 0));
			for (float x = cv.Min.x + std::fmod(o.x - cv.Min.x, gs) - gs; x < cv.Max.x; x += gs) dl->AddLine(ImVec2(x, cv.Min.y), ImVec2(x, cv.Max.y), gc);
			for (float y = cv.Min.y + std::fmod(o.y - cv.Min.y, gs) - gs; y < cv.Max.y; y += gs) dl->AddLine(ImVec2(cv.Min.x, y), ImVec2(cv.Max.x, y), gc);
		}
		std::string err = status_["error"].Str();
		bool running = status_["running"].Bool();
		bool any = false;
		for (const NodeV& n : nodes_) any |= !n.hidden;
		if (!running && !err.empty() && nodes_.empty())
		{
			Message(cv, err, L::T("The thread map needs the Windows kernel trace: Toolbox runs as administrator on Windows 10 or newer, and no other tool holds all kernel trace sessions."));
		}
		else if (!any)
		{
			Message(cv, nodes_.empty() ? L::T("Waiting for events…") : L::T("Nothing to show with these filters"), "");
		}
		else
		{
			DrawHalos(dl);
			DrawLinks(dl);
			DrawNodes(dl);
		}
		// Hint (top left), state pill (top right), the demo note (bottom right).
		float pillW = Pill(dl, cv, running);
		float hintW = cv.GetWidth() - pillW - ui::S(36);
		const std::string& hint = L::T("Double-click a process: show its threads · drag nodes with the mouse · wheel: zoom");
		// Two lines at most; in a narrow window one trimmed line with the whole hint on hover.
		if (ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, hint, hintW).y <= ui::FontPx(ui::Face::Ui, ui::FsSmall) * 2.2f)
			ui::TextAt(dl, cv.Min + ui::S(12, 10), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Faint"), hint, 0, hintW);
		else if (hintW > ui::S(40))
			ui::TextFit(dl, cv.Min + ui::S(12, 10), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Faint"), hint, hintW, ImGui::GetID("##tmhint"));
		if (status_["demo"].Bool())
		{
			std::string d = L::T("sample data");
			ImVec2 ds = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, d);
			ui::TextAt(dl, ImVec2(cv.Max.x - ui::S(12) - ds.x, cv.Max.y - ui::S(10) - ds.y), ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, ui::C("Faint"), d);
		}
		if (!err.empty() && (running || !nodes_.empty()))
			ui::TextAt(dl, ImVec2(cv.Min.x + ui::S(12), cv.Max.y - ui::S(24)), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Warn"), err, cv.GetWidth() - ui::S(24));
		dl->PopClipRect();
	}

	void Message(ImRect cv, const std::string& head, const std::string& sub)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float w = std::min(cv.GetWidth() - ui::S(60), ui::S(460));
		ImVec2 hs = ui::Measure(ui::Face::Ui, ui::Weight::Semi, ui::FsBody * 1.15f, head, w);
		ImVec2 ss = sub.empty() ? ImVec2(0, 0) : ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsNote, sub, w);
		float y = cv.GetCenter().y - (hs.y + ss.y + ui::S(8)) / 2;
		ui::TextAt(dl, ImVec2(cv.GetCenter().x - hs.x / 2, y), ui::Face::Ui, ui::Weight::Semi, ui::FsBody * 1.15f, ui::C(sub.empty() ? "Dim" : "Warn"), head, 0, w);
		if (!sub.empty()) ui::TextAt(dl, ImVec2(cv.GetCenter().x - ss.x / 2, y + hs.y + ui::S(8)), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C("Dim"), sub, 0, w);
	}

	/// The live / paused pill at the top right of the canvas; returns its width.
	float Pill(ImDrawList* dl, ImRect cv, bool running)
	{
		bool live = running && !paused_ && endAge_ == 0;
		std::string t = ui::Spaced(!running ? L::T("trace off") : live ? L::T("live · trace on") : L::T("paused · frame frozen"));
		float em = ui::FsSmall * 0.9f;
		ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Medium, em, t);
		ImVec2 size(ts.x + ui::S(30), ts.y + ui::S(10));
		ImRect r(ImVec2(cv.Max.x - ui::S(12) - size.x, cv.Min.y + ui::S(8)), ImVec2(cv.Max.x - ui::S(12), cv.Min.y + ui::S(8) + size.y));
		ui::DrawBorder(dl, r, ui::C("Panel"), ui::C("Line"), 1, 4);
		ImU32 dot = !running ? ui::C("Faint") : live ? kRun : kLock;
		ImVec2 dc(r.Min.x + ui::S(11), r.GetCenter().y);
		dl->AddCircleFilled(dc, ui::S(5.5f), Alpha(dot, 0.25f), 16);
		dl->AddCircleFilled(dc, ui::S(3.5f), dot, 16);
		ui::TextAt(dl, ImVec2(r.Min.x + ui::S(21), r.GetCenter().y - ts.y / 2), ui::Face::Mono, ui::Weight::Medium, em, ui::C("Dim"), t);
		return size.x;
	}

	void Interact(ImRect cv, bool hovered)
	{
		ImGuiIO& io = ImGui::GetIO();
		ImVec2 m = io.MousePos;
		hover_ = hovered ? HitTest(m) : 0;
		if (hover_ || drag_) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
		if (ImGui::IsItemActivated())
		{
			uint64_t id = HitTest(m);
			if (id)
			{
				drag_ = id;
				sel_ = id;
				dragFrom_ = m;
				moved_ = false;
			}
			else
			{
				panning_ = true;
				panFrom_ = m;
				panStart_ = pan_;
			}
		}
		if (ImGui::IsItemActive())
		{
			if (drag_ && map_)
			{
				if (ImLengthSqr(m - dragFrom_) > ui::S(3) * ui::S(3)) moved_ = true;
				if (moved_)
				{
					ImVec2 p = ToLayout(m);
					tm_layout_drag(map_, drag_, p.x, p.y);   // moves and pins it
				}
			}
			else if (panning_) pan_ = panStart_ + (m - panFrom_) / Scale();
		}
		if (ImGui::IsItemDeactivated())
		{
			drag_ = 0;
			panning_ = false;
		}
		if (hovered && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left))
		{
			uint64_t id = HitTest(m);
			if (id) ToggleExpand(id);
		}
		if (hovered && io.MouseWheel != 0)
		{
			ImVec2 before = ToLayout(m);
			k_ = std::clamp(k_ * std::pow(1.1f, io.MouseWheel), 0.45f, 2.4f);
			ImVec2 after = ToLayout(m);
			pan_ += after - before;   // the point under the mouse stays
		}
	}

	bool Focused(uint64_t id) const
	{
		if (!sel_) return true;
		if (id == sel_) return true;
		for (const LinkV& l : links_)
			if ((l.from == sel_ && l.to == id) || (l.to == sel_ && l.from == id)) return true;
		return false;
	}

	void DrawHalos(ImDrawList* dl)
	{
		for (const NodeV& g : nodes_)
		{
			if (!g.expanded || g.hidden) continue;
			ImVec2 c(0, 0);
			int k = 0;
			for (const NodeV& n : nodes_)
				if (n.parent == g.id && !n.hidden)
				{
					c += Disp(n.id);
					k++;
				}
			if (k == 0) continue;
			c /= (float)k;
			float rad = 0;
			for (const NodeV& n : nodes_)
				if (n.parent == g.id && !n.hidden) rad = std::max(rad, std::sqrt(ImLengthSqr(Disp(n.id) - c)) + n.r);
			rad += 26;
			ImVec2 s = ToScreen(c);
			float R = rad * Scale();
			dl->AddCircleFilled(s, R, IM_COL32(244, 182, 203, 12), 64);
			// Dashed outline (3 on, 5 off).
			int segs = std::max(24, (int)(R * 2 * 3.14159f / ui::S(8)));
			for (int i = 0; i < segs; i++)
			{
				float a0 = (float)i / segs * 6.28318f, a1 = a0 + 6.28318f / segs * 0.4f;
				dl->PathArcTo(s, R, a0, a1, 4);
				dl->PathStroke(IM_COL32(244, 182, 203, 72), ImDrawFlags_None, std::max(1.0f, ui::S(1)));
			}
			std::string t = g.label;
			float em = std::clamp(ui::FsSmall * k_, ui::FsSmall * 0.8f, ui::FsSmall * 1.3f);
			ImVec2 ts = ui::Measure(ui::Face::Ui, ui::Weight::Medium, em, t);
			ui::TextAt(dl, ImVec2(s.x - ts.x / 2, s.y - R + ui::S(6)), ui::Face::Ui, ui::Weight::Medium, em, Alpha(ui::C(g.cls == "game" ? "Accent" : "Dim"), Focused(g.id) ? 0.85f : 0.35f), t);
		}
	}

	void DrawLinks(ImDrawList* dl)
	{
		float sc = Scale();
		// Several links from A to B (wake and lock, say) bend apart instead of covering each other.
		std::unordered_map<uint64_t, int> same;
		for (size_t i = 0; i < links_.size(); i++)
		{
			const LinkV& l = links_[i];
			int nth = same[l.from * 1000003ull ^ l.to]++;
			const NodeV* a = Node(l.from);
			const NodeV* b = Node(l.to);
			if (!a || !b || a->hidden || b->hidden) continue;
			ImVec2 p1 = ToScreen(Disp(l.from)), p2 = ToScreen(Disp(l.to));
			if (ImLengthSqr(p2 - p1) < 4) continue;
			bool on = !sel_ || l.from == sel_ || l.to == sel_;
			float al = on ? 1.0f : 0.16f;
			ImU32 col = Alpha(KindCol(l.kind), al);
			float w = std::min(5.0f, 0.8f + (float)std::log10(l.rate + 1) * 1.3f) * sc;
			w = std::max(w, 1.0f);
			float bend = 0.12f + 0.10f * (float)nth;
			ImVec2 mid = (p1 + p2) * 0.5f + ImVec2((p2.y - p1.y) * bend, -(p2.x - p1.x) * bend);
			Curve c;
			c.Make(p1, mid, p2);
			float speed = 0.24f * std::min(6.0f, (float)std::log10(l.rate + 1) * 2.0f);   // reference: tick*0.004*f at 60 fps
			float moveT = paused_ ? 0.0f : animT_;
			if (l.kind == KLock) c.Dashes(dl, col, w, 6 * sc, 12 * sc, moveT * 36 * sc);   // dashes creep toward the owner
			else if (l.kind == KIpc) c.Dots(dl, col, std::max(1.0f, w * 0.6f), 7 * sc, moveT * 30 * sc);
			else dl->AddBezierQuadratic(p1, mid, p2, col, w, 24);
			// Arrow head at 82 % of the way.
			ImVec2 ah = Bez(p1, mid, p2, 0.82f);
			ImVec2 d = p2 - ah;
			float dl2 = std::sqrt(ImLengthSqr(d));
			if (dl2 > 0.01f)
			{
				d /= dl2;
				ImVec2 n(-d.y, d.x);
				float s = 7 * sc;
				dl->AddTriangleFilled(ah + d * s, ah - d * (s * 0.8f) + n * (s * 0.6f), ah - d * (s * 0.8f) - n * (s * 0.6f), col);
			}
			// Pulses moving in the link's direction (more of them for busy links).
			if (on && (l.kind == KWake || l.kind == KPreempt))
			{
				int count = l.rate > 1000 ? 3 : l.rate > 100 ? 2 : 1;
				float seed = (float)((l.from * 7 + l.to * 13 + (uint64_t)l.kind) % 97) / 97.0f;
				for (int j = 0; j < count; j++)
				{
					float t = std::fmod(moveT * speed + seed + (float)j / count, 1.0f);
					ImVec2 pp = Bez(p1, mid, p2, t);
					dl->AddCircleFilled(pp, std::max(2.0f, 2.6f * sc), IM_COL32(255, 255, 255, 235), 12);
					dl->AddCircleFilled(pp, std::max(3.5f, 5.0f * sc), Alpha(KindCol(l.kind), 0.25f), 16);
				}
			}
		}
	}

	void DrawNodes(ImDrawList* dl)
	{
		float sc = Scale();
		for (const NodeV& n : nodes_)
		{
			if (n.hidden || n.expanded) continue;
			ImVec2 s = ToScreen(Disp(n.id));
			float r = n.r * sc;
			float al = Focused(n.id) ? 1.0f : 0.3f;
			dl->AddCircleFilled(s, r, Alpha(ui::C("Pop"), std::max(al, 0.85f)), 40);
			ImU32 line = n.id == sel_ ? ui::C("Strong") : n.id == hover_ ? IM_COL32(244, 182, 203, 180) : ui::C("Line");
			dl->AddCircle(s, r, Alpha(line, al), 40, std::max(1.0f, 1.2f * sc));
			double load = n.kind == "process" ? std::min(1.0, n.cpu / 3) : std::min(1.0, n.cpu / std::max(1, n.kind == "group" ? n.count : 1));
			float ringR = r - 3 * sc;
			ImU32 sc0 = Alpha(StateCol(n), al);
			if (load > 0.005 && ringR > 1)
			{
				float a0 = -1.5708f, a1 = a0 + 6.28318f * (float)load;
				dl->PathArcTo(s, ringR, a0, a1, std::max(8, (int)(48 * load)));
				dl->PathStroke(sc0, ImDrawFlags_None, std::max(1.5f, 3 * sc));
			}
			else if (ringR > 1)
				dl->AddCircle(s, ringR, Alpha(sc0, 0.35f), 32, std::max(1.0f, 1 * sc));
			if (n.kind == "process" && !n.expanded && n.alive != 0 && n.pid != 0xFFFFFFFEu && n.pid != 0xFFFFFFFFu)
			{
				float em = ui::FsSmall * std::clamp(k_, 0.8f, 1.4f);
				ImVec2 ps = ui::Measure(ui::Face::Ui, ui::Weight::Normal, em, "+");
				ui::TextAt(dl, s - ps * 0.5f, ui::Face::Ui, ui::Weight::Normal, em, Alpha(ui::C("Faint"), al), "+");
			}
			bool thread = n.kind != "process";
			ui::Face f = thread ? ui::Face::Mono : ui::Face::Ui;
			float em = std::clamp((thread ? ui::FsSmall : ui::FsNote) * k_, ui::FsSmall * 0.75f, ui::FsBody * 1.3f);
			ImVec2 ts = ui::Measure(f, ui::Weight::Normal, em, n.label);
			float maxW = ui::S(170) * std::max(1.0f, k_);
			float tw = std::min(ts.x, maxW);
			ImU32 tc = n.cls == "game" && !thread ? ui::C("Accent") : ui::C("Text");
			ui::TextAt(dl, ImVec2(s.x - tw / 2, s.y + r + ui::S(4)), f, ui::Weight::Normal, em, Alpha(tc, al), n.label, maxW);
			if (n.id == hover_ && (ts.x > maxW || n.kind == "group")) hoverTip_ = n.kind == "process" ? n.label : n.image + " · " + n.label;
		}
		// The hovered node's whole name (labels are cut at 170 DIP on the canvas).
		if (!hoverTip_.empty() && hover_)
		{
			ImGui::BeginTooltip();
			ImGui::TextUnformatted(hoverTip_.c_str());
			ImGui::EndTooltip();
		}
		hoverTip_.clear();
	}

	// ---------------------------------------------------------------- right column
	std::string StateText(const NodeV& n) const
	{
		if (n.state == "running") return L::T("running");
		if (n.state == "ready") return L::T("ready, core busy");
		if (n.state == "wait_lock") return L::T("waits for a lock");
		if (n.state == "wait_ipc") return L::T("waits for an answer");
		if (n.state == "waiting") return L::T("waits for another thread");
		if (n.state == "sleeping") return L::T("sleeping");
		if (n.state == "gone") return L::T("ended");
		return L::T("unknown");
	}

	void DrawRight(ImRect r)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		ui::DrawBorder(dl, r, ui::C("Panel"), ui::C("Line"), 1, 8);
		ImGui::SetCursorScreenPos(r.Min + ui::S(16, 14));
		ui::Box box(r.Min.x + ui::S(16), r.Max.x - ui::S(14));
		const NodeV* n = Node(sel_);
		if (!n)
		{
			ui::Note(L::T("Select a node"));
			return;
		}
		ui::TextStyle big;
		big.size = ui::FsBody * 1.25f;
		big.weight = ui::Weight::Semi;
		big.brush = "Strong";
		big.wrap = true;   // a long process or thread name wraps instead of being cut
		ui::TextEx(n->label, big);
		std::string sub;
		if (n->kind == "thread") sub = n->image + " · " + L::T("thread") + (n->tid ? "  " + std::to_string(n->tid) : std::string());
		else if (n->kind == "group") sub = n->image + " · " + L::F("threads x{0}", {std::to_string(n->count)});
		else sub = "PID " + std::to_string(n->pid) + (n->cls == "game" ? " · " + L::T("the game") : std::string());
		ui::Text(sub, ui::FsNote, "Dim", false, true, ui::Thick(0, 2, 0, 0));
		// The state pill.
		{
			std::string t = ui::Spaced(n->kind == "process" && !n->expanded && n->state.empty() ? L::T("process") : StateText(*n));
			ImU32 col = StateCol(*n);
			float em = ui::FsSmall * 0.85f;
			ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Medium, em, t);
			ImRect pr = ui::Place(ImVec2(std::min(ts.x + ui::S(16), ui::AvailW()), ts.y + ui::S(6)), ui::Thick(0, 8, 0, 10));
			dl->AddRect(pr.Min, pr.Max, Alpha(col, 0.4f), pr.GetHeight() / 2, 0, std::max(1.0f, ui::S(1)));
			ui::TextFit(dl, ImVec2(pr.Min.x + ui::S(8), pr.GetCenter().y - ts.y / 2), ui::Face::Mono, ui::Weight::Medium, em, col, t, pr.GetWidth() - ui::S(16), ImGui::GetID("##tmstate"));
		}
		std::string p99 = n->p99 >= 0 ? L::F("{0} ms", {Fixed(n->p99 / 1000.0, 2)}) : std::string("—");
		if (n->kind == "process")
		{
			ui::Kv(L::T("Processor"), L::F("{0} cores", {Fixed(n->cpu, 2)}));
			ui::Kv(L::T("Threads"), n->alive >= 0 ? std::to_string(n->alive) : std::string("—"));
		}
		else
		{
			double per = n->kind == "group" ? n->cpu / std::max(1, n->count) : n->cpu;
			ui::Kv(L::T("Load"), L::F("{0}% of a core", {Fixed(per * 100, 0)}));
			ui::Kv(L::T("Priority"), n->prio > -1000 ? std::to_string(n->prio) : std::string("—"));
			ui::Kv(L::T("Core"), n->core >= 0 ? std::to_string(n->core) : std::string("—"));
		}
		ui::Kv(L::T("Waited for a core, p99"), p99);
		if (!n->reason.empty()) ui::Kv(L::T("Wait reason"), n->reason);
		ui::Caps(L::T("Node links"), 0, "Faint", true, ui::Thick(0, 14, 0, 6));
		std::vector<const LinkV*> mine;
		for (const LinkV& l : links_)
			if ((l.from == sel_ || l.to == sel_) && Node(l.from) && Node(l.to) && !Node(l.from)->hidden && !Node(l.to)->hidden) mine.push_back(&l);
		std::sort(mine.begin(), mine.end(), [](const LinkV* a, const LinkV* b) { return a->rate > b->rate; });
		if (mine.empty())
		{
			ui::Note(L::T("no links in this window"));
			return;
		}
		static const char* verbs[] = {"wakes", "takes the core", "waits for a lock", "calls"};
		float lineH = ui::FontPx(ui::Face::Ui, ui::FsNote);
		for (const LinkV* l : mine)
		{
			ImVec2 cur = ImGui::GetCursorScreenPos();
			float h = lineH * 2 + ui::S(12);
			if (cur.y + h > r.Max.y - ui::S(10)) break;
			ImRect br = ui::Place(ImVec2(ui::AvailW(), h), ui::Thick(0, 0, 0, 6));
			ui::DrawBorder(dl, br, ui::C("Panel2"), ui::C("None"), 0, 3);
			bool out = l->from == sel_;
			const NodeV* o = Node(out ? l->to : l->from);
			std::string who = std::string(out ? "→ " : "← ") + (o ? o->label : "?");
			std::string rate = L::F("{0}/s", {Fixed(l->rate, l->rate < 10 ? 1 : 0)});
			ImVec2 rs = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, rate);
			ui::TextAt(dl, ImVec2(br.Max.x - ui::S(8) - rs.x, br.Min.y + ui::S(6)), ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, ui::C("Text"), rate);
			ui::TextFit(dl, br.Min + ui::S(8, 6), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C("Text"), who, br.GetWidth() - ui::S(24) - rs.x, ImGui::GetID((const void*)l));
			ui::TextAt(dl, ImVec2(br.Min.x + ui::S(8), br.Min.y + ui::S(6) + lineH), ui::Face::Ui, ui::Weight::Medium, ui::FsNote, KindCol(l->kind), L::T(verbs[l->kind]));
		}
	}

	// ---------------------------------------------------------------- time strip
	void DrawStrip(ImRect r, ImRect spark, float leftW, float rightW)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		ui::DrawBorder(dl, r, ui::C("Panel"), ui::C("Line"), 1, 8);
		// Pause / Now at the left.
		ImGui::SetCursorScreenPos(ImVec2(r.Min.x + ui::S(14), r.GetCenter().y - ui::S(14)));
		{
			ui::Box box(r.Min.x + ui::S(14), r.Min.x + leftW);
			ui::Row row(ui::Thick(0));
			ui::ButtonStyle bs = ui::BtnStyle(0);
			bs.height = 28;
			if (ui::Button((paused_ ? L::T("Resume") : L::T("Pause")) + "##tmpause", bs))
			{
				paused_ = !paused_;
				if (!paused_) endAge_ = 0;
				lastPoll_ = -1;
			}
			ui::ButtonStyle ns = ui::BtnStyle(0);
			ns.height = 28;
			ui::Primary(ns, endAge_ != 0 || paused_);
			if (ui::Button(L::T("Now") + "##tmnow", ns))
			{
				endAge_ = 0;
				paused_ = false;
				lastPoll_ = -1;
			}
		}
		// The sparkline under the canvas.
		ImRect sp(ImVec2(spark.Min.x, r.Min.y + ui::S(8)), ImVec2(spark.Max.x, r.Max.y - ui::S(8)));
		ImGui::SetCursorScreenPos(sp.Min);
		ImGui::InvisibleButton("##tmstrip", sp.GetSize());
		const auto& load = series_["load"].Arr();
		const auto& wait = series_["wait_ms"].Arr();
		const auto& valid = series_["valid"].Arr();
		int n = (int)load.size();
		if (ImGui::IsItemHovered()) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
		if (ImGui::IsItemClicked() && n > 1)
		{
			int i = (int)std::lround((ImGui::GetIO().MousePos.x - sp.Min.x) / sp.GetWidth() * (n - 1));
			i = std::clamp(i, 0, n - 1);
			endAge_ = n - 1 - i;
			paused_ = endAge_ > 0;
			Snapshot();
		}
		float x0 = sp.Min.x, w = sp.GetWidth(), y1 = sp.Max.y - ui::S(4), hh = sp.GetHeight() - ui::S(18);
		bool anyValid = false;
		for (const json::Value& v : valid) anyValid |= v.Int() != 0;
		if (n > 1 && anyValid)
		{
			double maxL = 1, maxW = 0.5;
			std::vector<double> ws;
			for (int i = 0; i < n; i++)
			{
				maxL = std::max(maxL, load[(size_t)i].Num());
				maxW = std::max(maxW, wait[(size_t)i].Num());
				if (valid[(size_t)i].Int()) ws.push_back(wait[(size_t)i].Num());
			}
			std::vector<ImVec2> pl, pw;
			for (int i = 0; i < n; i++)
			{
				float x = x0 + (float)i / (n - 1) * w;
				pl.emplace_back(x, y1 - (float)(load[(size_t)i].Num() / maxL) * hh);
				pw.emplace_back(x, y1 - (float)(wait[(size_t)i].Num() / maxW) * hh);
			}
			dl->AddPolyline(pl.data(), (int)pl.size(), kRun, ImDrawFlags_None, std::max(1.0f, ui::S(1.4f)));
			dl->AddPolyline(pw.data(), (int)pw.size(), kPreempt, ImDrawFlags_None, std::max(1.0f, ui::S(1.4f)));
			// The biggest wait spike (3x the median and at least 0.5 ms) gets a marker.
			if (!ws.empty())
			{
				std::vector<double> sorted = ws;
				std::nth_element(sorted.begin(), sorted.begin() + (long)sorted.size() / 2, sorted.end());
				double med = sorted[sorted.size() / 2];
				int at = -1;
				double best = std::max(0.5, med * 3);
				for (int i = 0; i < n; i++)
					if (wait[(size_t)i].Num() > best)
					{
						best = wait[(size_t)i].Num();
						at = i;
					}
				if (at >= 0)
				{
					float x = x0 + (float)at / (n - 1) * w;
					std::string t = L::T("wait spike");
					ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.95f, t);
					float tx = std::clamp(x - ts.x / 2, x0 + ui::S(50), x0 + w - ts.x - ui::S(50));
					ui::TextAt(dl, ImVec2(tx, sp.Min.y), ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.95f, kPreempt, t);
					dl->AddLine(ImVec2(x, sp.Min.y + ts.y + ui::S(2)), ImVec2(x, y1), Alpha(kPreempt, 0.35f), 1);
				}
			}
			float cx = x0 + (float)(n - 1 - endAge_) / (n - 1) * w;
			dl->AddRectFilled(ImVec2(cx - ui::S(1), sp.Min.y + ui::S(2)), ImVec2(cx + ui::S(1), sp.Max.y), IM_COL32(244, 182, 203, 230));
		}
		ui::TextAt(dl, ImVec2(x0 + ui::S(2), sp.Min.y), ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.95f, ui::C("Faint"), L::T("−30 s"));
		std::string nowT = L::T("now");
		ImVec2 ns2 = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.95f, nowT);
		ui::TextAt(dl, ImVec2(x0 + w - ns2.x - ui::S(2), sp.Min.y), ui::Face::Mono, ui::Weight::Normal, ui::FsSmall * 0.95f, ui::C("Faint"), nowT);
		// The label at the right.
		std::string label = endAge_ == 0 ? L::T("30 s history · green: game load, red: waits for a core")
		                                 : L::F("Frame −{0} s · the map shows the links of that moment", {Fixed(endAge_ / 10.0, 1)});
		float lx = r.Max.x - rightW + ui::S(4);
		ImVec2 ls = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, label, rightW - ui::S(20));
		ui::TextAt(dl, ImVec2(lx, r.GetCenter().y - ls.y / 2), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Faint"), label, 0, rightW - ui::S(20));
	}

	// ---------------------------------------------------------------- state
	tm_map* map_ = nullptr;
	bool open_ = false;
	double animT_ = 0, lastPoll_ = -1;
	int seriesTick_ = 0;
	json::Value status_, series_, window_;
	std::vector<NodeV> nodes_;
	std::vector<LinkV> links_;
	std::unordered_map<uint64_t, size_t> index_;
	std::vector<tm_pos> pos_;
	std::unordered_map<uint64_t, ImVec2> disp_;
	std::set<uint32_t> expand_;
	uint32_t gamePid_ = 0;
	bool fGame_ = true, fBg_ = true, fSys_ = false, fMerge_ = true;
	bool paused_ = false;
	int endAge_ = 0;
	bool fitted_ = false, needFit_ = false;
	float k_ = 1;
	ImVec2 pan_ = ImVec2(0, 0), center_ = ImVec2(0, 0);
	uint64_t sel_ = 0, hover_ = 0, drag_ = 0;
	std::string hoverTip_;
	bool panning_ = false, moved_ = false;
	ImVec2 panFrom_, panStart_, dragFrom_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::ThreadMap, ThreadMapPage)
