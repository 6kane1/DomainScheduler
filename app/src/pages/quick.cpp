// QuickPage.cs ("Overview"): the processor map with the game's threads and the background, three plain columns
// (game, background, undo), the frame time of the last 60 s and the presets line. Owner: app pages A.
// Owner's decision (PLAN.md, 27.11.2026): the row "Boost / Reapply / Resync / Revert" is NOT ported; Resync moved to
// the Settings page as "Reload hardware and presets".
#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

#include "core.h"
#include "core_bridge.h"
#include "hud_charts.h"
#include "page.h"
#include "pages/common_a.h"
#include "shell.h"
#include "ui.h"
#include "widgets.h"

namespace {


class QuickPage : public Page {
public:
	std::string Title() override { return L::T("Overview"); }

	void Build() override
	{
		mapKey_.clear();
		shown_.clear();
		Tick();
	}

	void Tick() override
	{
		pa::PollQuiet();
		q_ = bridge::Quick();
		const json::Value& fr = q_["frames"];
		std::vector<double> iv;
		for (const auto& x : fr["intervals"].Arr()) iv.push_back(x.Num());
		if (fr["shown"].Bool())
			chart_.Set(iv, fr["baseline"].Num(), fr["applied_ago"].Num(-1), fr["mark_label"].Str(), fr["before_label"].Str());
		else
			chart_.Set({}, 0.0, -1.0, "", "");
		Bars();
	}

	void Draw(ImRect area) override
	{
		pa::PollQuiet();
		ImGui::SetCursorScreenPos(area.Min);
		ui::BeginScroll("##quick", area.GetSize());
		{
			ui::Box box(ImGui::GetCursorScreenPos().x, ui::BoxRight());
			ui::TextStyle title;
			title.size = ui::FsBody * 1.3f;
			title.weight = ui::Weight::Semi;
			title.brush = "Strong";
			ui::TextEx(L::T("Processor map"), title);
			ui::Text(L::T("Where the game's threads and the background programs are placed now"), ui::FsNote, "Dim", false, false, ui::Thick(0, 2, 0, 14));
			DrawMap();
			// WrapPanel, margin 0,10,0,12: the two legend items and the plain note.
			std::vector<ui::Leg> legs(2);
			legs[0].C = ui::C("Accent");
			legs[0].Text = L::T("Key thread of the game");
			legs[1].C = ui::C("Steel");
			legs[1].Text = L::T("Worker thread");
			pa::LegendRow(legs, L::T("Bar under a core = its load"), ui::Thick(0, 10, 0, 12));
			// Card(UniformGrid 3 columns): Game, Background, Undo.
			ui::BeginCard();
			{
				float w = ui::AvailW() / 3.0f;
				ui::Row row(ui::Thick(0));
				Column(L::T("Game"), q_["game_text"].Str(), w);
				Column(L::T("Background"), q_["bg_text"].Str(), w);
				Column(L::T("Undo"), q_["undo_text"].Str(), w);
			}
			ui::EndCard();
			// Card: the frame-time caption with the four readings at the right, then the chart (height 96).
			ui::BeginCard();
			Frames();
			ui::EndCard();
			ui::Note(q_["layer_text"].Str());
		}
		ui::EndScroll();
	}

private:
	// Column(head, body): the head Medium Strong (margin bottom 3), the body FsNote Dim wrapped, LineHeight 1.5.
	static void Column(const std::string& head, const std::string& body, float wPx)
	{
		ui::Group g(wPx / ui::Dpi());
		ui::TextStyle h;
		h.brush = "Strong";
		h.weight = ui::Weight::Medium;
		h.margin = ui::Thick(0, 0, 0, 3);
		ui::TextEx(head, h);
		ui::TextStyle b;
		b.size = ui::FsNote;
		b.brush = "Dim";
		b.wrap = true;
		b.lineHeight = ui::FsNote * 1.5f;
		ui::TextEx(body, b);
	}

	/// DrawMap's bar update: the map is built again when its key changes, else only the bars whose load moved by
	/// 0.02 or more are redrawn.
	void Bars()
	{
		std::string key;
		std::vector<double> load;
		for (const auto& d : q_["domains"].Arr())
		{
			key += d["caption"].Str() + "|" + (d["accent"].Bool() ? "1" : "0") + "|";
			for (const auto& c : d["cores"].Arr())
			{
				key += c["caption"].Str() + "," + c["pill"].Str() + "," + c["text"].Str() + ";";
				load.push_back(c["load"].Num());
			}
		}
		if (key != mapKey_ || shown_.size() != load.size())
		{
			mapKey_ = key;
			shown_ = load;
			return;
		}
		for (size_t i = 0; i < load.size(); i++)
			if (!(std::fabs(load[i] - shown_[i]) < 0.02)) shown_[i] = load[i];
	}

	void DrawMap()
	{
		if (!q_["topo_ok"].Bool())
		{
			ui::Text(L::T("CPU: the topology could not be read"), 0, "Warn");
			return;
		}
		const auto& doms = q_["domains"].Arr();
		int cols = std::max(1, q_["domains_columns"].Int(1));
		float x0 = ImGui::GetCursorScreenPos().x, right = ui::BoxRight();
		float cellW = (right - x0) / (float)cols;
		float bw = std::max(1.0f, std::round(ui::S(1)));
		// UniformGrid: every domain cell has the size of the largest one.
		float capH = std::max(ui::FontPx(ui::Face::Ui, ui::FsBody), ui::FontPx(ui::Face::Ui, ui::FsSmall));
		float cellH = 0;
		for (const auto& d : doms)
		{
			int cc = std::max(1, d["columns"].Int(1));
			int n = (int)d["cores"].size();
			int rows = (n + cc - 1) / cc;
			float h = bw + ui::S(13) + capH + ui::S(10) + rows * (ui::S(78) + ui::S(7)) + ui::S(7) + bw;
			cellH = std::max(cellH, h);
		}
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float y0 = ImGui::GetCursorScreenPos().y;
		int rowsOut = ((int)doms.size() + cols - 1) / cols;
		size_t bar = 0;
		for (size_t i = 0; i < doms.size(); i++)
		{
			const json::Value& d = doms[i];
			int c = (int)i % cols, r = (int)i / cols;
			float ml = (i % 2 != 0) ? ui::S(7) : 0, mr = (i % 2 == 0 && doms.size() > 1) ? ui::S(7) : 0;
			ImRect cell(ImVec2(x0 + c * cellW + ml, y0 + r * cellH), ImVec2(x0 + (c + 1) * cellW - mr, y0 + (r + 1) * cellH));
			bool acc = d["accent"].Bool();
			ui::DrawBorder(dl, cell, ui::C(acc ? "AccentSoft" : "Panel"), ui::C(acc ? "AccentLine" : "Line"), 1, 3);
			ImRect in(cell.Min + ImVec2(bw + ui::S(14), bw + ui::S(13)), cell.Max - ImVec2(bw + ui::S(7), bw + ui::S(7)));
			// DockPanel (margin bottom 10): the L3 size docked right (FsSmall Dim), the caption (Medium Strong).
			std::string l3 = d["l3_text"].Str();
			ImVec2 l3s = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, l3);
			float l3x = in.Max.x - l3s.x;
			ui::TextAt(dl, ImVec2(l3x, in.Min.y + (capH - ui::FontPx(ui::Face::Ui, ui::FsSmall)) * 0.5f), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Dim"), l3, in.GetWidth());
			ui::TextAt(dl, ImVec2(in.Min.x, in.Min.y + (capH - ui::FontPx(ui::Face::Ui, ui::FsBody)) * 0.5f), ui::Face::Ui, ui::Weight::Medium, ui::FsBody, ui::C("Strong"), d["caption"].Str(), std::max(0.0f, l3x - in.Min.x));
			int cc = std::max(1, d["columns"].Int(1));
			float gx0 = in.Min.x, gy0 = in.Min.y + capH + ui::S(10);
			float coreW = in.GetWidth() / (float)cc;
			const auto& cores = d["cores"].Arr();
			for (size_t k = 0; k < cores.size(); k++, bar++)
			{
				const json::Value& co = cores[k];
				int ccol = (int)k % cc, crow = (int)k / cc;
				ImRect b(ImVec2(gx0 + ccol * coreW, gy0 + crow * (ui::S(78) + ui::S(7))), ImVec2(gx0 + (ccol + 1) * coreW - ui::S(7), gy0 + crow * (ui::S(78) + ui::S(7)) + ui::S(78)));
				bool line = co["accent_line"].Bool();
				ui::DrawBorder(dl, b, ui::C("Well"), ui::C(line ? "AccentLine" : "Line"), 1, 3);
				ImRect ci(b.Min + ImVec2(bw + ui::S(9), bw + ui::S(8)), b.Max - ImVec2(bw + ui::S(9), bw + ui::S(9)));
				float cw = std::max(0.0f, ci.GetWidth());
				ui::TextAt(dl, ci.Min, ui::Face::Ui, ui::Weight::Normal, ui::FsSmall * 0.9f, ui::C("Faint"), co["caption"].Str(), cw);
				float ty = ci.Min.y + ui::FontPx(ui::Face::Ui, ui::FsSmall * 0.9f) + ui::S(6);
				const std::string& pill = co["pill"].Str();
				if (!pill.empty())
				{
					// Ui.Pill: AccentSoft / Panel2, a line, mono capitals at FsSmall*0.95.
					bool pa_ = co["pill_accent"].Bool();
					std::string t = ui::Spaced(pill);
					float em = ui::FsSmall * 0.95f;
					ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Medium, em, t);
					ImVec2 sz(std::min(cw, ts.x + ui::S(12) + 2 * bw), ts.y + ui::S(4) + 2 * bw);
					ImRect pr(ImVec2(ci.Min.x, ty), ImVec2(ci.Min.x + std::round(sz.x), ty + std::round(sz.y)));
					ui::DrawBorder(dl, pr, ui::C(pa_ ? "AccentSoft" : "Panel2"), ui::C(pa_ ? "AccentLine" : "Line"), 1, 2);
					ui::TextAt(dl, pr.Min + ImVec2(bw + ui::S(6), bw + ui::S(2)), ui::Face::Mono, ui::Weight::Medium, em, ui::C(pa_ ? "AccentText" : "Text"), t, pr.GetWidth() - ui::S(12) - 2 * bw);
				}
				else
				{
					float barTop = ci.Max.y - ui::S(3);
					std::vector<std::string> lines = pa::Wrap(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, co["text"].Str(), cw);
					float lp = ui::FontPx(ui::Face::Ui, ui::FsSmall);
					for (const auto& l : lines)
					{
						if (ty + lp > barTop) break;   // the DockPanel clips what does not fit above the bar
						ui::TextAt(dl, ImVec2(ci.Min.x, ty), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Faint"), l);
						ty += lp;
					}
				}
				// Ui.Bar(load, accent: a thread of the game is there), 3 DIP, docked at the bottom.
				double share = std::clamp(bar < shown_.size() ? shown_[bar] : co["load"].Num(), 0.0, 1.0);
				ImRect br(ImVec2(ci.Min.x, ci.Max.y - ui::S(3)), ImVec2(ci.Max.x, ci.Max.y));
				dl->AddRectFilled(br.Min, br.Max, ui::C("Line"));
				if (share > 0)
				{
					float fw = std::max(br.GetHeight(), br.GetWidth() * (float)share);
					dl->AddRectFilled(br.Min, ImVec2(std::min(br.Max.x, br.Min.x + fw), br.Max.y), ui::C(line ? "Accent" : "Steel"));
				}
			}
		}
		ui::Place(ImVec2(right - x0, rowsOut * cellH));
	}

	void Frames()
	{
		const json::Value& fr = q_["frames"];
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float x0 = ImGui::GetCursorScreenPos().x, right = ui::BoxRight(), y0 = ImGui::GetCursorScreenPos().y;
		// The four readings, docked right: caption FsSmall Dim over a mono value at FsBody*1.6, both right-aligned,
		// each with a margin of 26 at the left.
		struct Read {
			std::string cap, val;
			const char* brush;
		};
		Read reads[4] = {
			{L::T("Worst frames now (p99), ms"), fr["p99"].StrOr("\xE2\x80\x94"), "Accent"},
			{L::T("Before, ms"), fr["before"].StrOr("\xE2\x80\x94"), "Strong"},
			{L::T("1% low, FPS"), fr["low1"].StrOr("\xE2\x80\x94"), "Strong"},
			{L::T("0.1% low, FPS"), fr["low01"].StrOr("\xE2\x80\x94"), "Strong"},
		};
		float capPx = ui::FontPx(ui::Face::Ui, ui::FsSmall), valPx = ui::FontPx(ui::Face::Mono, ui::FsBody * 1.6f);
		float rh = capPx + valPx;
		float t1 = ui::FontPx(ui::Face::Ui, ui::FsBody);
		float lh = t1 + capPx;
		// Native-only: when the readings leave the title less than 150 DIP (a narrow window), the title goes on a row
		// of its own above them instead of being drawn under them.
		float readW = 0;
		for (const Read& r : reads)
			readW += std::max(ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, r.cap).x, ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsBody * 1.6f, r.val).x) + ui::S(14);
		bool stacked = right - x0 - readW < ui::S(150);
		float yTitle = y0;
		if (stacked) y0 += lh + ui::S(8);
		float x = right;
		for (int i = 3; i >= 0; i--)
		{
			float cw = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, reads[i].cap).x;
			float vw = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsBody * 1.6f, reads[i].val).x;
			float w = std::max(cw, vw);
			ui::TextAt(dl, ImVec2(x - cw, y0), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Dim"), reads[i].cap);
			ui::TextAt(dl, ImVec2(x - vw, y0 + capPx), ui::Face::Mono, ui::Weight::Normal, ui::FsBody * 1.6f, ui::C(reads[i].brush), reads[i].val);
			x -= w + (stacked ? ui::S(14) : ui::S(26));
		}
		float leftW = stacked ? right - x0 : std::max(1.0f, x - x0);
		float h = stacked ? (y0 - yTitle) + rh : std::max(rh, lh);
		float ly = stacked ? yTitle : y0 + (h - lh) * 0.5f;   // the left column is VerticalAlignment.Center
		y0 = yTitle;
		ui::TextAt(dl, ImVec2(x0, ly), ui::Face::Ui, ui::Weight::Medium, ui::FsBody, ui::C("Strong"), L::T("Frame time, ms"), leftW);
		ui::TextAt(dl, ImVec2(x0, ly + t1), ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Dim"), fr["note"].Str(), leftW);
		ui::Place(ImVec2(right - x0, h), ui::Thick(0, 0, 0, 10));
		ImRect cr = ui::Place(ImVec2(right - ImGui::GetCursorScreenPos().x, ui::S(96)));
		chart_.Draw(cr);
	}

	json::Value q_;
	hud::FrameChart chart_;
	std::string mapKey_;
	std::vector<double> shown_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Quick, QuickPage)
