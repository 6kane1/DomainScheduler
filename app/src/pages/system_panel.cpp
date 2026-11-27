// SystemPanel.cs: "This system": what the PC is and how Windows is set up for games, in the free space of the right
// column (home screen and advanced layout). Read once when the program starts (hardware facts come from the same
// background read the footer uses), so it costs nothing while a game runs. Owner: app pages A.
#include <cmath>
#include <algorithm>
#include <string>
#include <vector>

#include "core.h"
#include "core_bridge.h"
#include "page.h"
#include "pages/common_a.h"
#include "ui.h"

namespace {

class SystemPanel : public SystemPanelBase {
public:
	SystemPanel() { Texts(); }

	void SetFacts(const json::Value& f) override
	{
		facts_ = f["hardware"];
		Texts();
	}

	void Texts() override
	{
		rows_.clear();
		// WindowsText() and GameMode() read the registry (the core does it: get "system").
		json::Value sys = bridge::SystemInfo();
		Row(L::T("Windows"), sys["windows"].Str());
		const json::Value& f = facts_;
		if (f.IsObj())
		{
			std::string cpu = Trim(f["cpu"].Str());
			if (!f["cpu"].Str().empty()) Row(L::T("Processor"), cpu);
			const json::Value* g = nullptr;
			for (const auto& x : f["gpus"].Arr())
				if (!x["basic"].Bool())
				{
					g = &x;
					break;
				}
			if (g == nullptr && f["gpus"].size() > 0) g = &f["gpus"][0];
			if (g != nullptr)
			{
				Row(L::T("Graphics card"), (*g)["name"].Str());
				const std::string& ver = (*g)["version"].Str();
				const std::string& date = (*g)["date"].Str();
				if (!ver.empty()) Row(L::T("Driver"), !date.empty() ? ver + " \xC2\xB7 " + date : ver);
			}
			int now = f["ram_now"].Int(), rated = f["ram_rated"].Int();
			if (now > 0)
				Row(L::T("Memory"), rated > now ? L::F("{0} MT/s (rated {1})", {std::to_string(now), std::to_string(rated)}) : L::F("{0} MT/s", {std::to_string(now)}));
			int hz = f["refresh"].Int();
			if (hz > 0) Row(L::T("Display"), L::F("{0} Hz", {std::to_string(hz)}));
		}
		else
		{
			Row(L::T("Hardware"), L::T("reading…"));
		}
		bool gm = sys["game_mode"].Bool(true);
		Row(L::T("Game Mode"), gm ? L::T("on") : L::T("off"), gm ? "Accent" : "Warn");
	}

	// Ui.Panel(body, padding 14,12,12,8): the caption docked at the top, the rows in a ScrollViewer that fills.
	void Draw(ImRect area) override
	{
		ImRect in = pa::FillPanel(area, ui::Thick(14, 12, 12, 8));
		ImGui::SetCursorScreenPos(in.Min);
		{
			ui::Box b(in.Min.x, in.Max.x);
			ui::Head(L::T("This system"));
		}
		float y = ImGui::GetCursorScreenPos().y;
		float h = in.Max.y - y;
		if (h < 1) return;
		ImGui::SetCursorScreenPos(ImVec2(in.Min.x, y));
		ui::BeginScroll("##sysrows", ImVec2(in.GetWidth(), h));
		{
			float x0 = ImGui::GetCursorScreenPos().x;
			ui::Box b(x0, ui::BoxRight());
			ImDrawList* dl = ImGui::GetWindowDrawList();
			for (const auto& r : rows_)
			{
				// Border: a Line2 line below, padding 0,7,0,7; the key in capitals, the value (FsNote, wrapped) below.
				ImVec2 p = ImGui::GetCursorScreenPos();
				ui::Space(7);
				// Ui.Caps(HudMath.Spaced(key)): the C# code spaces the key twice (Caps spaces it again); kept as it is.
				ui::Caps(ui::Spaced(r.key), ui::FsSmall * 0.9f, "Dim", true);
				ui::TextStyle st;
				st.size = ui::FsNote;
				st.brush = r.brush;
				st.wrap = true;
				st.margin = ui::Thick(0, 2, 0, 0);
				ui::TextEx(r.value, st);
				ui::Space(7);
				float bw = std::max(1.0f, std::round(ui::S(1)));
				ImRect line(ImVec2(p.x, ImGui::GetCursorScreenPos().y), ImVec2(ui::BoxRight(), ImGui::GetCursorScreenPos().y + bw));
				dl->AddRectFilled(line.Min, line.Max, ui::C("Line2"));
				ui::Place(ImVec2(0, bw));
			}
		}
		ui::EndScroll();
	}

private:
	struct R {
		std::string key, value;
		const char* brush;
	};

	static std::string Trim(const std::string& s)
	{
		size_t a = s.find_first_not_of(" \t\r\n");
		if (a == std::string::npos) return "";
		size_t b = s.find_last_not_of(" \t\r\n");
		return s.substr(a, b - a + 1);
	}

	void Row(const std::string& key, const std::string& value, const char* brush = "Silver") { rows_.push_back(R{key, value, brush}); }

	json::Value facts_;
	std::vector<R> rows_;
};

} // namespace

DOS_REGISTER_SYSTEM_PANEL(SystemPanel)
