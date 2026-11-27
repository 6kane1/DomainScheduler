// GameThreadsPage.cs ("Game threads"): the game's key threads (dynamic Top-5 or the role plan), how it works, what
// is protected, the core-mask button with its short guide, and the last moves. Owner: app pages A.
#include <cstdlib>
#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

#include "core.h"
#include "core_bridge.h"
#include "page.h"
#include "pages/common_a.h"
#include "shell.h"
#include "ui.h"
#include "widgets.h"

namespace {


// Cols: 52, *, 230, 112, 130.
constexpr float kCols[5] = {52, 0, 230, 112, 130};

class GameThreadsPage : public Page {
public:
	std::string Title() override { return L::T("Game threads"); }

	void Build() override { Tick(); }

	void Tick() override
	{
		pa::PollQuiet();
		g_ = bridge::GameThreads();
#ifdef DOS_UI_SHOT
		// Screenshot build only (no game under Wine): DOS_SHOT_SAMPLE=1 fills the table with sample rows.
		if (getenv("DOS_SHOT_SAMPLE") && g_["rows"].Arr().empty())
			g_ = json::Parse(R"J({"share_column":true,"slot_header":"No.","sub":"cs2.exe","rows":[
{"n":1,"lead":true,"name":"MainThread (CSGO Main Thread, render submit)","detail":"engine2.dll+0x1a2b30, game logic and input","share":0.31,"core":"CPU 2, 3","prio":"Above normal (+1)","prio_dim":false},
{"n":2,"lead":false,"name":"RenderThread","detail":"rendersystemvulkan.dll+0x4410","share":0.22,"core":"CPU 4","prio":"Above normal","prio_dim":false},
{"n":3,"lead":false,"name":"Job queue worker with a rather long descriptive name","detail":"tier0.dll+0x99f0","share":0.08,"core":"any","prio":"Normal","prio_dim":true}]})J");
#endif
		// 1.16.0 Test 1.9: the role plan is gone; the "Role plan / Dynamic Top-5" switch is built in C# but never
		// added to the page, so it is not drawn here either.
	}

	void Draw(ImRect area) override
	{
		pa::PollQuiet();
		ImGui::SetCursorScreenPos(area.Min);
		ui::BeginScroll("##gt", area.GetSize());
		{
			ui::Box box(ImGui::GetCursorScreenPos().x, ui::BoxRight());
			Header();
			HeadRow();
			Rows();
			ui::Space(16);
			AllThreads();
			// Card(UniformGrid 3): How it works, What is protected, Right now.
			ui::BeginCard();
			{
				float w = ui::AvailW() / 3.0f;
				ui::Row row(ui::Thick(0));
				Column(L::T("How it works"), g_["how"].Str(), w);
				Column(L::T("What is protected"), L::T("Core 0, anti-cheat threads and every other thread are left as Windows set them. A preferred core is a hint, nothing is hard-pinned."), w);
				Column(L::T("Right now"), g_["moves"].Str(), w);
			}
			ui::EndCard();
			MaskGuide();
			ui::BeginCard();
			ui::Head(L::T("Last moves"));
			const std::string& le = g_["log_empty"].Str();
			if (g_["log"].size() == 0 && !le.empty())
				ui::Note(le);
			else
			{
				// Top-5 lines are Ui.Text(FsNote, Dim); the role plan's one line is a Note.
				bool note = !g_["top5_mode"].Bool();
				for (const auto& l : g_["log"].Arr())
				{
					if (note) ui::Note(l.Str());
					else ui::Text(l.Str(), ui::FsNote, "Dim");
				}
			}
			ui::EndCard();
		}
		ui::EndScroll();
	}

private:
	void Header()
	{
		// DockPanel (margin bottom 16): the mask button docked right (top), the title and the sub line in the rest.
		float x0 = ImGui::GetCursorScreenPos().x, right = ui::BoxRight();
		ImVec2 p = ImGui::GetCursorScreenPos();
		bool on = g_["mask_on"].Bool();
		std::string label = g_["mask_button"].Str();
		ui::ButtonStyle bs;
		bs.height = 30;
		bs.padding = ui::Thick(14, 0, 14, 1);
		bs.margin = ui::Thick(0);
		ui::Primary(bs, on);
		std::string tip = g_["mask_tip"].Str();
		bs.tip = tip.c_str();
		float bwid = ui::Measure(ui::Face::Ui, bs.weight, ui::FsBody, label).x + ui::S(28);
		float bx = right - bwid;
		ImGui::SetCursorScreenPos(ImVec2(bx, p.y));
		bool clicked = false;
		{
			ui::Box b(bx, right);
			clicked = ui::Button(label + "##mask", bs);
		}
		if (clicked)
		{
			// 1.16.1 Test 4: the button flips the mask whatever the rule says: back to the rule ("auto") or against it.
			bridge::MaskToggle();
			Tick();
		}
		ImGui::SetCursorScreenPos(p);
		{
			ui::Box b(x0, std::max(x0, bx));
			ui::Group g;
			ui::TextStyle t;
			t.size = ui::FsBody * 1.3f;
			t.weight = ui::Weight::Semi;
			t.brush = "Strong";
			ui::TextEx(L::T("Key threads of the game"), t);
			ui::Text(g_["sub"].Str(), ui::FsNote, "Dim", false, true, ui::Thick(0, 2, 0, 0));
		}
		float h = std::max(ui::LastRect().Max.y, p.y + ui::S(30)) - p.y;
		ImGui::SetCursorScreenPos(p);
		ui::Place(ImVec2(right - x0, h), ui::Thick(0, 0, 0, 16));
	}

	static std::vector<float> Widths(float total)
	{
		// Native-only: the thread column keeps at least 170 DIP; the share, core and priority columns shrink first.
		return ui::FitColumns(std::vector<float>(kCols, kCols + 5), total, 170.0f, 0.45f);
	}

	void HeadRow()
	{
		// Border: a line below, padding 10,0,10,8; the captions FsSmall Dim (Slot / No., Thread, Share, core, prio).
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float x0 = ImGui::GetCursorScreenPos().x, right = ui::BoxRight(), y0 = ImGui::GetCursorScreenPos().y;
		std::vector<float> w = Widths(right - x0 - ui::S(20));
		std::string caps[5] = {g_["slot_header"].Str(), L::T("Thread"), g_["share_column"].Bool() ? L::T("Share of the game's load") : "", L::T("Preferred core"), L::T("Priority")};
		float x = x0 + ui::S(10);
		float px = ui::FontPx(ui::Face::Ui, ui::FsSmall);
		for (int i = 0; i < 5; i++)
		{
			if (!caps[i].empty()) ui::TextFit(dl, ImVec2(x, y0), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Dim"), caps[i], std::max(0.0f, w[i] - ui::S(8)), ImGui::GetID(i + 100));
			x += w[i];
		}
		float h = px + ui::S(8);
		float bw = std::max(1.0f, std::round(ui::S(1)));
		dl->AddRectFilled(ImVec2(x0, y0 + h), ImVec2(right, y0 + h + bw), ui::C("Line"));
		ui::Place(ImVec2(right - x0, h + bw));
	}

	void Rows()
	{
		const auto& rows = g_["rows"].Arr();
		const std::string& empty = g_["empty"].Str();
		if (rows.empty())
		{
			if (empty.empty()) return;
			// Not active: Ui.Text(FsNote, blocked ? Warn : Dim, wrap), margin 10,14,0,0. Active and no rows yet: a Note in a
			// Border with padding 10,14,0,0. The C# brush is Warn whenever Auto.Blocked is set (the text says the
			// blocker only while Armed); the shell's state carries Blocked.
			bool warn = !S->State()["blocked"].Str().empty() && S->State()["state"].Int() != 2;
			ui::Text(empty, ui::FsNote, warn ? "Warn" : "Dim", false, true, ui::Thick(10, 14, 0, 0));
			return;
		}
		for (const auto& r : rows) Row(r);
	}

	void Row(const json::Value& r)
	{
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float x0 = ImGui::GetCursorScreenPos().x, right = ui::BoxRight(), y0 = ImGui::GetCursorScreenPos().y;
		std::vector<float> w = Widths(right - x0 - ui::S(20));
		float bw = std::max(1.0f, std::round(ui::S(1)));
		// Row height: the thread column (name + detail) or the 24 px number box, padding 10,12,10,12, a line below.
		float nameH = ui::FontPx(ui::Face::Ui, ui::FsBody), detH = ui::FontPx(ui::Face::Ui, ui::FsSmall);
		float coreH = ui::FontPx(ui::Face::Mono, ui::FsNote) + ui::S(7) + 2 * bw;
		float ch = std::max({nameH + detH, ui::S(24), coreH});
		float top = y0 + ui::S(12), mid = top + ch * 0.5f;
		float x = x0 + ui::S(10);
		bool lead = r["lead"].Bool();
		// The number: 24x24, radius 3, AccentSoft / None, AccentLine / Line, mono FsSmall centered.
		{
			ImRect b(ImVec2(x, std::round(mid - ui::S(12))), ImVec2(x + ui::S(24), std::round(mid - ui::S(12)) + ui::S(24)));
			ui::DrawBorder(dl, b, ui::C(lead ? "AccentSoft" : "None"), ui::C(lead ? "AccentLine" : "Line"), 1, 3);
			std::string n = std::to_string(r["n"].Int());
			ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, n);
			ui::TextAt(dl, b.GetCenter() - ts * 0.5f, ui::Face::Mono, ui::Weight::Normal, ui::FsSmall, ui::C(lead ? "AccentText" : "Dim"), n);
		}
		x += w[0];
		{
			float ty = mid - (nameH + detH) * 0.5f;
			float tw = std::max(0.0f, w[1] - ui::S(10));
			ImGui::PushID(r["n"].Int());
			ui::TextFit(dl, ImVec2(x, ty), ui::Face::Ui, ui::Weight::Medium, ui::FsBody, ui::C("Strong"), r["name"].Str(), tw, ImGui::GetID("##name"));
			ui::TextFit(dl, ImVec2(x, ty + nameH), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Faint"), r["detail"].Str(), tw, ImGui::GetID("##detail"));
			ImGui::PopID();
		}
		x += w[1];
		if (r["share"].IsNum())
		{
			// Ui.Bar(min(1, share*2.2), accent, 4) of width 150, then "nn%" (mono FsNote Text, margin left 10).
			double share = r["share"].Num();
			float bh = ui::S(4);
			float barW = std::clamp(w[2] - ui::S(60), ui::S(24), ui::S(150));   // narrower when the column shrank
			ImRect b(ImVec2(x, std::round(mid - bh * 0.5f)), ImVec2(x + barW, std::round(mid - bh * 0.5f) + bh));
			dl->AddRectFilled(b.Min, b.Max, ui::C("Line"));
			float s = (float)std::clamp(std::min(1.0, share * 2.2), 0.0, 1.0);
			if (s > 0) dl->AddRectFilled(b.Min, ImVec2(std::min(b.Max.x, b.Min.x + std::max(bh, b.GetWidth() * s)), b.Max.y), ui::C("Accent"));
			std::string pct = bridge::Num(share * 100.0, "0") + "%";
			float py = mid - ui::FontPx(ui::Face::Mono, ui::FsNote) * 0.5f;
			ui::TextAt(dl, ImVec2(b.Max.x + ui::S(10), py), ui::Face::Mono, ui::Weight::Normal, ui::FsNote, ui::C("Text"), pct, std::max(0.0f, x + w[2] - b.Max.x - ui::S(10)));
		}
		x += w[2];
		{
			// The core: Panel2, Line, radius 3, padding 10,3,10,4, mono FsNote Strong.
			std::string core = r["core"].Str();
			ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsNote, core);
			ImRect b(ImVec2(x, std::round(mid - coreH * 0.5f)), ImVec2(x + std::min(w[3] - ui::S(8), ts.x + ui::S(20) + 2 * bw), std::round(mid - coreH * 0.5f) + coreH));
			ui::DrawBorder(dl, b, ui::C("Panel2"), ui::C("Line"), 1, 3);
			ui::TextAt(dl, b.Min + ImVec2(bw + ui::S(10), bw + ui::S(3)), ui::Face::Mono, ui::Weight::Normal, ui::FsNote, ui::C("Strong"), core, b.GetWidth() - ui::S(20) - 2 * bw);
		}
		x += w[3];
		ImGui::PushID(r["n"].Int());
		ui::TextFit(dl, ImVec2(x, mid - nameH * 0.5f), ui::Face::Ui, ui::Weight::Normal, ui::FsBody, ui::C(r["prio_dim"].Bool() ? "Faint" : "Text"), r["prio"].Str(), w[4], ImGui::GetID("##prio"));
		ImGui::PopID();
		float h = ui::S(12) + ch + ui::S(12);
		dl->AddRectFilled(ImVec2(x0, y0 + h), ImVec2(right, y0 + h + bw), ui::C("Line"));
		ui::Place(ImVec2(right - x0, h + bw));
	}

	/// 1.17.1: ideal cores for all game threads (Top-5's other threads): the switch, one line, the last moves.
	void AllThreads()
	{
		ui::BeginCard();
		ui::Head(L::T("Other threads"));
		bool on = g_["all_on"].Bool();
		if (ui::CheckRow("all_threads_ideal", g_["all_label"].Str(), g_["all_hint"].Str(), on))
		{
			bridge::SettingToggle("AllThreadsIdeal", on);
			Tick();
		}
		const std::string& line = g_["all_line"].Str();
		if (!line.empty()) ui::Text(line, ui::FsNote, "Text", false, true, ui::Thick(0, 6, 0, 0));
		for (const auto& l : g_["all_log"].Arr()) ui::Text(l.Str(), ui::FsNote, "Dim");
		ui::EndCard();
	}

	// Column(head, body): head Medium Strong (margin 0,0,12,3); body FsNote Dim wrapped, LineHeight 1.5, margin right 14.
	static void Column(const std::string& head, const std::string& body, float wPx)
	{
		ui::Group g(wPx / ui::Dpi());
		ui::TextStyle h;
		h.brush = "Strong";
		h.weight = ui::Weight::Medium;
		h.margin = ui::Thick(0, 0, 12, 3);
		ui::TextEx(head, h);
		ui::TextStyle b;
		b.size = ui::FsNote;
		b.brush = "Dim";
		b.wrap = true;
		b.lineHeight = ui::FsNote * 1.5f;
		b.margin = ui::Thick(0, 0, 14, 0);
		ui::TextEx(body, b);
	}

	/// 1.16.1 Test 4: a short guide: the mask is a per-PC choice and has to be measured.
	static void MaskGuide()
	{
		ui::BeginCard();
		ui::Head(L::T("Core mask: only for some processors"));
		ui::Note(L::T("The mask keeps a few cores for devices and the background. Turn it on only on processors with more than 8 cores of two kinds: Intel with E-cores (12th to 14th gen, Core Ultra) or Ryzen with two CCDs (7900X, 7950X, 7950X3D, 9950X3D and similar). On 8 cores or fewer, and on processors with one cache and no E-cores (7800X3D, 9800X3D, 7700X), keep it off: the game needs every core. Even where it fits, only a measurement tells:"));
		const char* steps[] = {
			"1. Same scene each time: a benchmark map or the same deathmatch, the same settings.",
			"2. Alternate: mask off, mask on, off, on (the button above). Wait 10 s after each switch.",
			"3. At least 3 runs of each, the same number for both. Compare the 1% low and the stutters, not the average FPS.",
			"4. Keep the mask only if it is better in every pair of runs. On processors with one cache and no E-cores it is off by default: on the owner's 7800X3D it made the game wait for cores longer.",
		};
		for (const char* st : steps) ui::Text(L::T(st), ui::FsNote, "Text", false, true, ui::Thick(0, 3, 0, 0));
		ui::EndCard();
	}

	json::Value g_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::GameThreads, GameThreadsPage)
