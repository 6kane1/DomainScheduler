// HomeView.cs: the simple home screen in the HUD look (1.14.0): the game's frame time of the last minute, the
// processor map, the Auto / device optimization cards, three plain facts and "This system".
// Nothing here runs while the window is hidden or minimized: Tick is called only by a visible window, and the 4 Hz
// chart timer runs only while the home screen is on screen, a game is handled and the live charts are on.
// Owner: app pages A.
#include <algorithm>
#include <cmath>
#include <memory>
#include <string>
#include <vector>

#include "app.h"
#include "core.h"
#include "core_bridge.h"
#include "dos_core.h"
#include "hud_charts.h"
#include "page.h"
#include "pages/common_a.h"
#include "shell.h"
#include "ui.h"
#include "widgets.h"

namespace {


/// A Ui.SetValue reading: the value, the small unit after it (none = ""), the value's brush.
struct Reading {
	std::string v = "\xE2\x80\x94";
	std::string unit;
	const char* brush = "Faint";
};

// HudInk.GameBar / BgBar / IrqBar (the legend squares of the processor map).
ImU32 InkGameBar() { return ui::C("Accent"); }
ImU32 InkBgBar() { return IM_COL32(201, 206, 214, 150); }
ImU32 InkIrqBar() { return IM_COL32(232, 194, 122, 230); }

float Px1() { return std::max(1.0f, std::round(ui::S(1))); }

/// One card button of the right column (CardButton): padding 14,11,14,12; the title (FsBody*1.08 SemiBold) with the
/// tag at its right, the sub text (FsNote, wrapped, LineHeight 1.4, margin top 4) below.
struct Card {
	std::string title;
	const char* titleBrush = "Strong";
	bool wrapTitle = false;       // TextTrimming.None + Wrap (the device card) instead of "…"
	std::string tag;              // "" = collapsed
	const char* tagLook = "plain";
	std::string sub;
	bool showSub = true;
	const char* subBrush = "Dim";
	const char* bg = "Panel";
	const char* line = "Line";
	bool glow = false;            // OnGlow: the pink gradient of an Auto that is on
	bool enabled = true;
	bool hit = true;              // IsHitTestVisible
	std::string tip;
	float marginTop = 0;
};

/// Draws a card; true when it was clicked.
bool DrawCard(const char* id, const Card& c)
{
	float bw = Px1();
	float innerW = std::max(1.0f, ui::AvailW() - 2 * bw - ui::S(28));
	float titleEm = ui::FsBody * 1.08f;
	float tagEm = ui::FsSmall * 0.92f;
	std::string tagText = c.tag.empty() ? "" : ui::Spaced(c.tag);
	ImVec2 ts = c.tag.empty() ? ImVec2(0, 0) : ui::Measure(ui::Face::Mono, ui::Weight::Semi, tagEm, tagText);
	ImVec2 tagSize = c.tag.empty() ? ImVec2(0, 0) : ImVec2(ts.x + ui::S(12) + 2 * bw, ts.y + ui::S(4) + 2 * bw);
	float titleW = std::max(1.0f, innerW - (c.tag.empty() ? 0.0f : tagSize.x + ui::S(8)));
	float titlePx = ui::FontPx(ui::Face::Ui, titleEm);
	float titleH = c.wrapTitle ? pa::WrappedHeight(ui::Face::Ui, ui::Weight::Semi, titleEm, c.title, titleW) : titlePx;
	float headH = std::max(titleH, tagSize.y);
	float subLh = ui::S(ui::FsNote * 1.4f);
	float subH = c.showSub ? ui::S(4) + pa::WrappedHeight(ui::Face::Ui, ui::Weight::Normal, ui::FsNote, c.sub, innerW, subLh) : 0.0f;
	float op = c.enabled ? 1.0f : 0.45f;
	auto content = [&](ImRect in) {
		ImDrawList* dl = ImGui::GetWindowDrawList();
		if (c.glow)
		{
			// LinearGradientBrush(#29F4B6CB -> #05F4B6CB, 0°): left to right.
			ImRect r(in.Min - ImVec2(ui::S(14), ui::S(11)) + ImVec2(bw, bw), in.Max + ImVec2(ui::S(14), ui::S(12)) - ImVec2(bw, bw));
			ImU32 a = ui::Fade(IM_COL32(244, 182, 203, 41), op), b = ui::Fade(IM_COL32(244, 182, 203, 5), op);
			dl->AddRectFilledMultiColor(r.Min, r.Max, a, b, b, a);
		}
		float ty = in.Min.y + (headH - titleH) * 0.5f;
		if (c.wrapTitle)
			pa::DrawWrapped(dl, ImVec2(in.Min.x, ty), ui::Face::Ui, ui::Weight::Semi, titleEm, ui::C(c.titleBrush, op), c.title, titleW);
		else
			ui::TextAt(dl, ImVec2(in.Min.x, ty), ui::Face::Ui, ui::Weight::Semi, titleEm, ui::C(c.titleBrush, op), c.title, titleW);
		if (!c.tag.empty())
		{
			std::string look = c.tagLook;
			const char* tb = look == "on" ? "Accent" : "None";
			const char* tl = look == "on" ? "Accent" : look == "accent" ? "AccentLine" : "Line";
			const char* tf = look == "on" ? "Ink" : look == "accent" ? "AccentText" : "Silver";
			ImVec2 tp(in.Max.x - tagSize.x, in.Min.y + (headH - tagSize.y) * 0.5f);
			ImRect tr(ImVec2(std::round(tp.x), std::round(tp.y)), ImVec2(std::round(tp.x + tagSize.x), std::round(tp.y + tagSize.y)));
			ui::DrawBorder(dl, tr, ui::C(tb, op), ui::C(tl, op), 1, 2);
			ui::TextAt(dl, tr.Min + ImVec2(bw + ui::S(6), bw + ui::S(2)), ui::Face::Mono, ui::Weight::Semi, tagEm, ui::C(tf, op), tagText);
		}
		if (c.showSub)
			pa::DrawWrapped(dl, ImVec2(in.Min.x, in.Min.y + headH + ui::S(4)), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C(c.subBrush, op), c.sub, innerW, subLh);
	};
	ImVec2 contentDip((innerW) / ui::Dpi(), (headH + subH) / ui::Dpi());
	if (!c.hit)
	{
		// Not hit-testable: the same frame without hover or click.
		float w = ui::AvailW();
		float h = headH + subH + ui::S(11) + ui::S(12) + 2 * bw;
		ImRect r = ui::Place(ImVec2(w, std::round(h)), ui::Thick(0, c.marginTop, 0, 0));
		ui::DrawBorder(ImGui::GetWindowDrawList(), r, ui::C(c.bg, op), ui::C(c.line, op), 1, 8);
		content(ImRect(r.Min + ImVec2(bw + ui::S(14), bw + ui::S(11)), r.Max - ImVec2(bw + ui::S(14), bw + ui::S(12))));
		return false;
	}
	ui::ButtonStyle s;
	s.width = -1;
	s.height = 0;
	s.padding = ui::Thick(14 + bw / ui::Dpi(), 11 + bw / ui::Dpi(), 14 + bw / ui::Dpi(), 12 + bw / ui::Dpi());
	s.margin = ui::Thick(0, c.marginTop, 0, 0);
	s.bg = c.glow ? "None" : c.bg;
	s.line = c.line;
	s.enabled = c.enabled;
	s.tip = c.tip.empty() ? nullptr : c.tip.c_str();
	return ui::ButtonCustom(id, contentDip, s, [&](ImRect in, bool) { content(in); });
}

class HomeView : public HomeViewBase {
public:
	HomeView()
	{
		bridge::Do("self_thread_window");   // SelfThreads.Register(L.N("window")): the UI thread
		sys_ = pages::MakeSystemPanel();
		Texts();
	}

	SystemPanelBase* SysPanel() override { return sys_.get(); }

	void Texts() override
	{
		if (sys_) sys_->Texts();
		ms_ = L::T("ms");
		lowText_ = L::T("1% low");
	}

	/// The shell says whether the home screen is on screen; off stops the chart at once.
	void SetVisible(bool on) override
	{
		visible_ = on;
		if (!on) chartRunning_ = false;
	}

	// ---- the 1 s update (only while the window is visible)
	void Tick() override
	{
		visible_ = true;
		ticked_ = true;
		pa::PollQuiet();
		h_ = bridge::Home();
		upd_ = bridge::Update();
		autoClicked_ = false;
		const json::Value& st = h_["stats"];
		bool valid = st["valid"].Bool();
		std::string fps = L::T("FPS");
		if (valid)
		{
			double l01 = st["low01_fps"].Num(), fm = st["frame_ms"].Num(), hm = st["hitches_per_min"].Num();
			reads_[0] = {bridge::Num(st["low1_fps"].Num(), "0"), fps, "Accent"};
			reads_[1] = {bridge::Num(st["avg_fps"].Num(), "0"), fps, "Strong"};
			reads_[2] = l01 > 0 ? Reading{bridge::Num(l01, "0"), fps, "Strong"} : Reading{"\xE2\x80\x94", "", "Dim"};
			reads_[3] = {bridge::Num(fm, fm < 10.0 ? "0.00" : "0.0"), ms_, "Strong"};
			reads_[4] = {bridge::Num(hm, "0"), L::T("per min"), hm >= 1.0 ? "Warn" : "Strong"};
		}
		else
		{
			for (auto& r : reads_) r = Reading{};
		}
		bool active = h_["active"].Bool();
		bool on = h_["on"].Bool();
		gameText_ = active ? h_["game_exe"].Str() : (on ? L::T("waiting") : L::T("not handled"));
		gameBrush_ = active ? "Strong" : "Dim";
		int bg = h_["background_count"].Int();
		bg_ = bg > 0 ? Reading{bridge::Num(bg, "0"), L::T("quieted"), "Strong"} : Reading{L::T("untouched"), "", "Dim"};
		low_ = active && h_["frames_ok"].Bool() && valid ? Reading{bridge::Num(st["low1_fps"].Num(), "0"), fps, "Accent"} : Reading{"\xE2\x80\x94", "", "Dim"};
		// The processor map.
		const json::Value& m = h_["map"];
		if (m.IsObj())
		{
			std::vector<int> lps;
			std::vector<double> load;
			std::vector<hud::CoreKind> kinds;
			for (const auto& x : m["lps"].Arr()) lps.push_back(x.Int());
			for (const auto& x : m["load"].Arr()) load.push_back(x.Num());
			for (const auto& x : m["kinds"].Arr()) kinds.push_back((hud::CoreKind)x.Int());
			map_.Set(lps, load, kinds);
			mapCap_ = ui::Spaced(L::T("CPU map")) + "  \xC2\xB7  " + ui::Spaced(L::F("{0} cores / {1} threads", {std::to_string(m["physical"].Int()), std::to_string(m["logical"].Int())}));
		}
		// The chart: on while measured and the live charts are on; stopped and cleared otherwise.
		if (!h_["chart_wanted"].Bool())
		{
			chartRunning_ = false;
			chart_.Clear();
		}
		else if (!chartRunning_ && visible_)
		{
			chartRunning_ = true;
			ChartTick();
		}
	}

	void Draw(ImRect area) override
	{
		pa::PollQuiet();
		if (!ticked_) Tick();
		if (chartRunning_)
		{
			if (!visible_ || !h_["chart_wanted"].Bool() || !app::OnScreen())
				chartRunning_ = false;
			else
			{
				double now = app::Now();
				if (now - chartAt_ >= 0.25) ChartTick();
				app::WakeIn(std::max(1, (int)std::ceil((chartAt_ + 0.25 - app::Now()) * 1000.0)));
			}
		}
		// Grid, margin 12,10,12,10: left (*), 10, right (270).
		ImRect main(area.Min + ImVec2(ui::S(12), ui::S(10)), area.Max - ImVec2(ui::S(12), ui::S(10)));
		float rightW = ui::S(270);
		ImRect left(main.Min, ImVec2(std::max(main.Min.x, main.Max.x - rightW - ui::S(10)), main.Max.y));
		ImRect right(ImVec2(main.Max.x - rightW, main.Min.y), main.Max);
		Left(left);
		Right(right);
	}

private:
	void ChartTick()
	{
		chartAt_ = app::Now();
		json::Value c = bridge::HomeChart();
		if (!c.IsObj()) return;
		std::vector<double> mx, end;
		for (const auto& x : c["max"].Arr()) mx.push_back(x.Num());
		for (const auto& x : c["end"].Arr()) end.push_back(x.Num());
		chart_.Set(mx, end, c["median_ms"].Num(), c["p99_ms"].Num(), ms_, lowText_);
	}

	// ---- left: frame time (fills, min 200), 10, processor map, 10, tiles
	void Left(ImRect r)
	{
		float bw = Px1();
		float capPx = ui::FontPx(ui::Face::Mono, ui::FsSmall);
		float mapH = bw + ui::S(13) + capPx + ui::S(10) + ui::S(128) + ui::S(10) + bw;
		float tileH = bw + ui::S(10) + ui::FontPx(ui::Face::Mono, ui::FsSmall) + ui::S(6) + ui::FontPx(ui::Face::Mono, 18) + ui::S(11) + bw;
		float frameH = std::max(ui::S(200), r.GetHeight() - mapH - tileH - ui::S(20));
		ImRect fr(r.Min, ImVec2(r.Max.x, r.Min.y + frameH));
		ImRect mr(ImVec2(r.Min.x, fr.Max.y + ui::S(10)), ImVec2(r.Max.x, fr.Max.y + ui::S(10) + mapH));
		ImRect tr(ImVec2(r.Min.x, mr.Max.y + ui::S(10)), ImVec2(r.Max.x, mr.Max.y + ui::S(10) + tileH));
		FramePanel(fr);
		MapPanel(mr);
		Tiles(tr);
	}

	void FramePanel(ImRect r)
	{
		ImRect in = pa::FillPanel(r, ui::Thick(16, 13, 16, 10));
		ImDrawList* dl = ImGui::GetWindowDrawList();
		// The caption: "▍ FRAME TIME  ·  game.exe" at the left, "LAST 60 S" docked right (margin bottom 8).
		float capPx = ui::FontPx(ui::Face::Mono, ui::FsSmall);
		std::string rightText = ui::Spaced(L::T("last 60 s"));
		float rw = ui::Measure(ui::Face::Mono, ui::Weight::Medium, ui::FsSmall, rightText).x;
		ui::TextAt(dl, ImVec2(in.Max.x - rw, in.Min.y), ui::Face::Mono, ui::Weight::Medium, ui::FsSmall, ui::C("Silver"), rightText);
		{
			ImGui::SetCursorScreenPos(in.Min);
			ui::Box b(in.Min.x, std::max(in.Min.x, in.Max.x - rw));
			std::vector<ui::Run> runs(2);
			runs[0].text = "\xE2\x96\x8D ";
			runs[0].brush = "Accent";
			runs[0].face = ui::Face::Mono;
			runs[0].weight = ui::Weight::Semi;
			runs[1].text = ui::Spaced(L::T("Frame time"));
			runs[1].brush = "Dim";
			runs[1].face = ui::Face::Mono;
			runs[1].weight = ui::Weight::Semi;
			std::string exe = h_["active"].Bool() ? h_["game_exe"].Str() : "";
			if (!exe.empty())
			{
				ui::Run e;
				e.text = "  \xC2\xB7  " + exe;
				e.brush = "Silver";
				e.face = ui::Face::Mono;
				e.weight = ui::Weight::Medium;
				runs.push_back(e);
			}
			ui::Runs(runs, ui::FsSmall);
		}
		float y = in.Min.y + capPx + ui::S(8);
		// The readings: 1% low big and pink, then four smaller ones sharing the rest (Auto, 24, *, *, *, *).
		const char* keys[5] = {"1% low", "Average", "0.1% low", "Frame", "Hitches"};
		float keyPx = ui::FontPx(ui::Face::Mono, ui::FsSmall * 0.95f);
		float bigPx = ui::FontPx(ui::Face::Mono, 34), smallPx = ui::FontPx(ui::Face::Mono, 20);
		float rowH = keyPx + ui::S(2) + std::max(bigPx, smallPx);
		ImVec2 bigSize = ui::MeasureRuns(ValueRuns(reads_[0], 34), 34);
		float bigW = std::max(bigSize.x, ui::Measure(ui::Face::Mono, ui::Weight::Semi, ui::FsSmall * 0.95f, ui::Spaced(L::T(keys[0]))).x);
		float rest = std::max(0.0f, in.GetWidth() - bigW - ui::S(24));
		for (int i = 0; i < 5; i++)
		{
			bool big = i == 0;
			float x0 = big ? in.Min.x : in.Min.x + bigW + ui::S(24) + rest / 4.0f * (float)(i - 1);
			float x1 = big ? in.Min.x + bigW : x0 + rest / 4.0f;
			float vpx = big ? bigPx : smallPx;
			float cy = y + rowH - (keyPx + ui::S(2) + vpx);   // VerticalAlignment.Bottom
			ImGui::SetCursorScreenPos(ImVec2(x0, cy));
			ui::Box b(x0, x1);
			ui::Caps(L::T(keys[i]), ui::FsSmall * 0.95f, big ? "Accent" : "Dim");
			ui::Value(reads_[i].v, reads_[i].unit, big ? 34.0f : 20.0f, reads_[i].brush, ui::Thick(0, 2, 0, 0));
		}
		y += rowH + ui::S(6);
		// The chart area (margin top 4, clipped): the chart, and the empty text centered over it (MaxWidth 420).
		ImRect ca(ImVec2(in.Min.x, y + ui::S(4)), ImVec2(in.Max.x, std::max(y + ui::S(4), in.Max.y)));
		if (ca.GetHeight() < 1) return;
		dl->PushClipRect(ca.Min, ca.Max, true);
		if (h_["chart_wanted"].Bool()) chart_.Draw(ca);
		const std::string& empty = h_["empty"].Str();
		if (!empty.empty())
		{
			float w = std::min(ui::S(420), ca.GetWidth());
			float th = pa::WrappedHeight(ui::Face::Ui, ui::Weight::Normal, ui::FsBody, empty, w);
			pa::DrawWrapped(dl, ImVec2(ca.GetCenter().x - w * 0.5f, ca.GetCenter().y - th * 0.5f), ui::Face::Ui, ui::Weight::Normal, ui::FsBody, ui::C("Dim"), empty, w, 0, true);
		}
		dl->PopClipRect();
	}

	static std::vector<ui::Run> ValueRuns(const Reading& r, float size)
	{
		std::vector<ui::Run> runs;
		ui::Run v;
		v.text = r.v;
		v.face = ui::Face::Mono;
		v.weight = ui::Weight::Semi;
		runs.push_back(v);
		if (!r.unit.empty())
		{
			ui::Run u;
			u.text = " " + r.unit;
			u.face = ui::Face::Mono;
			u.weight = ui::Weight::Medium;
			u.size = std::max(ui::FsSmall, size * 0.42f);
			runs.push_back(u);
		}
		return runs;
	}

	void MapPanel(ImRect r)
	{
		ImRect in = pa::FillPanel(r, ui::Thick(16, 13, 16, 10));
		ImDrawList* dl = ImGui::GetWindowDrawList();
		// The caption (margin bottom 10): the legend docked right, "▍ CPU MAP  ·  8 CORES / 16 THREADS" in the rest.
		float capPx = ui::FontPx(ui::Face::Mono, ui::FsSmall);
		float legEm = ui::FsSmall * 0.95f;
		struct L3 {
			ImU32 c;
			std::string t;
		} legs[3] = {{InkGameBar(), L::T("game")}, {InkBgBar(), L::T("background")}, {InkIrqBar(), L::T("interrupts")}};
		float lw = 0;
		for (const auto& l : legs) lw += ui::S(12) + ui::S(8) + ui::S(5) + ui::Measure(ui::Face::Mono, ui::Weight::Normal, legEm, l.t).x;
		float x = in.Max.x - lw;
		float legPx = ui::FontPx(ui::Face::Mono, legEm);
		for (const auto& l : legs)
		{
			x += ui::S(12);
			float sy = std::round(in.Min.y + (capPx - ui::S(8)) * 0.5f + ui::S(0.5f));
			dl->AddRectFilled(ImVec2(std::round(x), sy), ImVec2(std::round(x + ui::S(8)), sy + std::round(ui::S(8))), l.c);
			x += ui::S(8) + ui::S(5);
			ui::TextAt(dl, ImVec2(x, in.Min.y + (capPx - legPx) * 0.5f), ui::Face::Mono, ui::Weight::Normal, legEm, ui::C("Dim"), l.t);
			x += ui::Measure(ui::Face::Mono, ui::Weight::Normal, legEm, l.t).x;
		}
		if (!mapCap_.empty())
		{
			float maxW = std::max(0.0f, in.Max.x - lw - in.Min.x);
			float aw = ui::Measure(ui::Face::Mono, ui::Weight::Semi, ui::FsSmall, "\xE2\x96\x8D ").x;
			ui::TextAt(dl, in.Min, ui::Face::Mono, ui::Weight::Semi, ui::FsSmall, ui::C("Accent"), "\xE2\x96\x8D ", maxW);
			ui::TextAt(dl, ImVec2(in.Min.x + aw, in.Min.y), ui::Face::Mono, ui::Weight::Semi, ui::FsSmall, ui::C("Dim"), mapCap_, std::max(0.0f, maxW - aw));
		}
		ImRect mr(ImVec2(in.Min.x, in.Min.y + capPx + ui::S(10)), ImVec2(in.Max.x, in.Min.y + capPx + ui::S(10) + ui::S(128)));
		map_.Draw(mr);
	}

	void Tiles(ImRect r)
	{
		float w = (r.GetWidth() - ui::S(20)) / 3.0f;
		struct T {
			std::string cap;
			Reading v;
		} tiles[3] = {
			{L::T("Game"), Reading{gameText_, "", gameBrush_}},
			{L::T("Background programs"), bg_},
			{L::T("Smoothness (1% low)"), low_},
		};
		for (int i = 0; i < 3; i++)
		{
			ImRect t(ImVec2(r.Min.x + (w + ui::S(10)) * (float)i, r.Min.y), ImVec2(r.Min.x + (w + ui::S(10)) * (float)i + w, r.Max.y));
			ImRect in = pa::FillPanel(t, ui::Thick(14, 10, 14, 11));
			ImGui::SetCursorScreenPos(in.Min);
			ui::Box b(in.Min.x, in.Max.x);
			ui::Caps(tiles[i].cap);
			if (i == 0)
			{
				// _game.Text: a plain mono 18 SemiBold text (trimmed), Strong while a game is handled, else Dim.
				ui::TextStyle st;
				st.size = 18;
				st.face = ui::Face::Mono;
				st.weight = ui::Weight::Semi;
				st.brush = tiles[i].v.brush;
				st.margin = ui::Thick(0, 6, 0, 0);
				ui::TextEx(tiles[i].v.v, st);
			}
			else
				ui::Value(tiles[i].v.v, tiles[i].v.unit, 18, tiles[i].v.brush, ui::Thick(0, 6, 0, 0));
		}
	}

	// ---- right: the update card, Auto, devices (+ "Put back"), This system (fills)
	void Right(ImRect r)
	{
		ImGui::SetCursorScreenPos(r.Min);
		{
			ui::Box b(r.Min.x, r.Max.x);
			UpdateCard();
			AutoCard();
			DevCard();
		}
		float y = ImGui::GetCursorScreenPos().y + ui::S(8);
		if (sys_ && r.Max.y - y > 1)
		{
			ImRect sr(ImVec2(r.Min.x, y), r.Max);
			ImGui::SetCursorScreenPos(sr.Min);
			ImGui::BeginChild("##homesys", sr.GetSize(), ImGuiChildFlags_None, ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoScrollbar | ImGuiWindowFlags_NoScrollWithMouse);
			sys_->Draw(sr);
			ImGui::EndChild();
		}
	}

	/// UpdateCard(always: false): a new version, shown only when there is one (and not during a game).
	void UpdateCard()
	{
		const json::Value& u = upd_;
		if (!u["show_home"].Bool()) return;
		ui::BeginPanel(ui::Thick(15, 13, 15, 13), true, ui::Thick(0, 0, 0, 8));
		ui::Head(L::T("Updates"));
		ui::Text(u["this_text"].Str(), ui::FsNote, "Dim", false, true);
		const json::Value& t = u["toolbox"];
		if (t.IsObj())
		{
			ui::Text(t["title"].Str(), ui::FsBody, "Strong", true, true);
			if (!t["notes"].Str().empty()) ui::Text(t["notes"].Str(), ui::FsNote, "Text", false, true);
			{
				ui::Row row;
				ui::ButtonStyle s = ui::BtnStyle();
				ui::Primary(s);
				s.enabled = t["can_update"].Bool();
				if (ui::Button(L::T("Update") + "##updinstall", s)) bridge::UpdateInstall();
			}
			ui::Note(t["note"].Str());
		}
		const json::Value& p = u["playbook"];
		if (p.IsObj())
		{
			ui::Text(p["title"].Str(), ui::FsBody, "Strong", true, true);
			if (!p["notes"].Str().empty()) ui::Text(p["notes"].Str(), ui::FsNote, "Text", false, true);
			if (!p["file"].IsNull())
			{
				ui::Row row;
				if (ui::Btn(L::T("Show the file") + "##pbshow")) bridge::PlaybookShow();
			}
			else
			{
				ui::Row row;
				if (ui::Btn(L::T("Download") + "##pbdl")) bridge::PlaybookDownload();
				if (ui::Btn(L::T("Open the website") + "##pbsite")) bridge::PlaybookSite();
			}
			ui::Note(p["note"].Str());
		}
		if (u["busy"].IsStr()) ui::Text(u["busy"].Str(), ui::FsNote, "Accent", false, true);
		if (u["error"].IsStr()) ui::Text(u["error"].Str(), ui::FsNote, "Warn", false, true);
		ui::EndPanel();
	}

	void AutoCard()
	{
		bool on = h_["on"].Bool();
		Card c;
		c.title = L::T("Turn on Auto");
		c.tag = on ? L::T("ON") : L::T("OFF");
		c.tagLook = on ? "on" : "plain";
		c.sub = h_["auto_sub"].Str();
		c.subBrush = h_["auto_faulted"].Bool() ? "Error" : "Dim";
		c.enabled = h_["button_enabled"].Bool(true) && !autoClicked_;
		c.tip = on ? L::T("Turn off Auto") : L::T("Turn on Auto");
		c.glow = on;
		c.line = on ? "Accent" : "Line";
		if (DrawCard("##autocard", c))
		{
			// _auto.IsEnabled = false; H.Paused = false; Opt.Queue(Auto.Click).
			autoClicked_ = true;
			dos_auto_click();
		}
	}

	void DevCard()
	{
		const json::Value& d = h_["dev"];
		bool done = d["done"].Bool(), game = d["game"].Bool();
		Card c;
		// "Devices optimized · restart needed" is longer than the right column: two lines instead of "…".
		c.title = d["title"].Str();
		c.wrapTitle = true;
		c.titleBrush = done ? "AccentText" : (game ? "Dim" : "Strong");
		c.tag = d["tag"].Str();
		c.tagLook = "plain";
		c.sub = d["note"].Str();
		c.showSub = !c.sub.empty();
		c.subBrush = d["note_warn"].Bool() ? "Warn" : "Dim";
		c.line = done ? "AccentLine" : "Line";
		c.enabled = d["enabled"].Bool();
		c.hit = !done;
		c.marginTop = 8;
		if (DrawCard("##devcard", c) && !done)
		{
			// Pressing the button is the consent the Devices page asks for with "Allow device policy changes".
			uint64_t job = bridge::DevicesOptimize();
			if (job != 0) S->Busy(L::T("Writing…"), job, [](const json::Value&) {});
		}
		if (d["back"].Bool())
		{
			// "Put back": FsSmall, underlined, Dim, right-aligned, margin 0,5,2,0, a hand cursor.
			std::string t = L::T("Put back");
			ImVec2 ts = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, t);
			ImVec2 p = ImGui::GetCursorScreenPos();
			float x = ui::BoxRight() - ui::S(2) - ts.x;
			ImGui::SetCursorScreenPos(ImVec2(x - ui::S(0), p.y));
			ImGuiID id = ImGui::GetID("##devback");
			ImRect r = ui::Place(ts, ui::Thick(0, 5, 2, 0), id);
			bool hov = false, held = false;
			bool click = ImGui::ButtonBehavior(r, id, &hov, &held, ImGuiButtonFlags_PressedOnRelease);
			if (hov) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
			ImDrawList* dl = ImGui::GetWindowDrawList();
			ui::TextAt(dl, r.Min, ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Dim"), t);
			float uy = std::round(r.Max.y - ui::S(1.5f));
			dl->AddRectFilled(ImVec2(r.Min.x, uy), ImVec2(r.Max.x, uy + Px1()), ui::C("Dim"));
			ImGui::SetCursorScreenPos(ImVec2(p.x, ImGui::GetCursorScreenPos().y));
			if (click)
			{
				uint64_t job = bridge::DevicesPutBack();
				if (job != 0) S->Busy(L::T("Writing…"), job, [](const json::Value&) {});
			}
		}
	}

	std::unique_ptr<SystemPanelBase> sys_;
	json::Value h_, upd_;
	hud::HudFrameChart chart_;
	hud::CoreMapView map_;
	std::string mapCap_;
	Reading reads_[5];
	std::string gameText_;
	const char* gameBrush_ = "Dim";
	Reading bg_, low_;
	std::string ms_ = "ms", lowText_ = "1% low";
	bool visible_ = false;
	bool chartRunning_ = false;
	double chartAt_ = -1;
	bool autoClicked_ = false;
	bool ticked_ = false;
};

} // namespace

DOS_REGISTER_HOME(HomeView)
