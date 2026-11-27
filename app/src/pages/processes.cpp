// ProcessesPage.cs ("Processes"): every process with its class and affinity, the watchlist switch, and the mask
// editor of a watched process (saved as its rule). Owner: app pages A.
#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

#include "core.h"
#include "core_bridge.h"
#include "icon_cache.h"
#include "page.h"
#include "pages/common_a.h"
#include "shell.h"
#include "ui.h"
#include "widgets.h"

namespace {


bool EqualsIgnoreCase(const std::string& a, const std::string& b)
{
	if (a.size() != b.size()) return false;
	for (size_t i = 0; i < a.size(); i++)
	{
		char x = a[i], y = b[i];
		if (x >= 'A' && x <= 'Z') x = (char)(x + 32);
		if (y >= 'A' && y <= 'Z') y = (char)(y + 32);
		if (x != y) return false;
	}
	return true;
}

class ProcessesPage : public Page {
public:
	ProcessesPage()
	{
		table_.SelectionChanged = [this]() {
			sel_ = atoi(table_.Selected().c_str());
			if (sel_ < 0 || sel_ >= (int)rows_.size()) sel_ = -1;
			LoadEdit();
			Info();
		};
	}

	std::string Title() override { return L::T("Processes"); }

	void Build() override
	{
		topo_ = bridge::Topo();
		tv_ = pa::Topo(topo_);
		table_.SetColumns({{"", 24}, {L::T("Process"), 0}, {L::T("PID"), 62}, {L::T("Class"), 96}, {L::T("Affinity"), 110}, {L::T("Processors"), 96}, {L::T("Watch"), 58}});
		table_.SetRows({});   // a new table each time the page is built
		rows_.clear();
		sel_ = -1;
		dirty_ = false;
		Info();
	}

	void Shown() override { Refresh(); }

	void Tick() override { pa::PollQuiet(); }

	void Draw(ImRect area) override
	{
		pa::PollQuiet();
		bool autoOn = S->AutoOn();
		const Row* r = Sel();
		bool canApply = r != nullptr && dirty_ && elig_.IsNull() && !autoOn && r->ok && !r->denied && tv_.valid;
		bool canReset = r != nullptr && r->watched && !autoOn && r->ok && !r->denied && tv_.valid;
		ImGui::SetCursorScreenPos(area.Min);
		ui::Box box(area.Min.x, area.Max.x);
		bool refresh = false, apply = false, reset = false;
		{
			ui::Row row;
			refresh = ui::Btn(L::T("Refresh") + "##refresh");
			ui::ButtonStyle a = ui::BtnStyle();
			a.enabled = canApply;
			apply = ui::Button(L::T("Apply") + "##apply", a);
			ui::ButtonStyle rs = ui::BtnStyle();
			rs.enabled = canReset;
			reset = ui::Button(L::T("Reset") + "##reset", rs);
		}
		// Grid: the table (*) and the info card (318, margin 12,0,0,12).
		float y = ImGui::GetCursorScreenPos().y;
		// Native-only: in a narrow window the card gives way (at most 38 % of the page, at least 220), the table keeps
		// room for the process names.
		float cardW = std::max(std::min(ui::S(318), area.GetWidth() * 0.38f), std::min(ui::S(220), ui::S(318)));
		float infoW = cardW + ui::S(12);
		ImRect tableR(ImVec2(area.Min.x, y), ImVec2(std::max(area.Min.x, area.Max.x - infoW), area.Max.y));
		ImRect cardR(ImVec2(area.Max.x - cardW, y), ImVec2(area.Max.x, area.Max.y - ui::S(12)));
		if (area.GetWidth() < ui::S(640))
		{
			// Very narrow: the card goes below the table instead of beside it.
			float th = std::round((area.Max.y - y) * 0.58f);
			tableR = ImRect(ImVec2(area.Min.x, y), ImVec2(area.Max.x, y + th));
			cardR = ImRect(ImVec2(area.Min.x, y + th + ui::S(12)), ImVec2(area.Max.x, area.Max.y - ui::S(12)));
		}
		if (tableR.GetWidth() > 1 && tableR.GetHeight() > 1)
		{
			ImGui::SetCursorScreenPos(tableR.Min);
			ui::Box tb(tableR.Min.x, tableR.Max.x);
			table_.Draw("##procs", tableR.GetSize());
		}
		if (cardR.GetHeight() > 1)
		{
			ImRect in = pa::FillPanel(cardR, ui::Thick(15, 13, 15, 13));
			ImGui::SetCursorScreenPos(in.Min);
			ui::BeginScroll("##procinfo", in.GetSize());
			{
				ui::Box b(ImGui::GetCursorScreenPos().x, ui::BoxRight());
				DrawInfo(autoOn);
			}
			ui::EndScroll();
		}
		if (refresh) Refresh();
		if (apply) Apply();
		if (reset) Reset();
	}

private:
	struct Row {
		uint32_t pid = 0;
		std::string exe, path, classText, hex, ranges;
		bool watched = false, denied = false, ok = false, changed = false;
		uint64_t affinity = 0;
	};

	const Row* Sel() const { return sel_ >= 0 && sel_ < (int)rows_.size() ? &rows_[(size_t)sel_] : nullptr; }

	void Refresh()
	{
		S->Busy(L::T("Reading processes…"), bridge::ProcessesList(), [this](const json::Value& v) {
			rows_.clear();
			for (const auto& x : v["rows"].Arr())
			{
				Row r;
				r.pid = (uint32_t)x["pid"].I64();
				r.exe = x["exe"].Str();
				r.path = x["path"].Str();
				r.watched = x["watched"].Bool();
				r.denied = x["denied"].Bool();
				r.ok = x["ok"].Bool();
				r.changed = x["changed"].Bool();
				r.classText = x["class_text"].Str();
				r.affinity = x["affinity"].U64();
				if (r.ok)
				{
					r.hex = ui::Hex(r.affinity);
					r.ranges = bridge::Ranges(r.affinity);
				}
				rows_.push_back(std::move(r));
			}
			SetRows();
			sel_ = -1;
			dirty_ = false;
			Info();
		});
	}

	void SetRows()
	{
		int px = (int)std::lround(14.72 * S->DpiScaleX());
		std::vector<ui::TableRow> trs;
		for (size_t i = 0; i < rows_.size(); i++)
		{
			const Row& r = rows_[i];
			ui::TableRow tr;
			tr.tag = std::to_string(i);
			ui::Cell icon;
			icon.heightDip = 14.72f;
			std::string path = r.path;
			int ipx = std::max(16, px);
			icon.draw = [path, ipx](ImRect c) { icons::Draw(path, ipx, c.Min, ui::S(14.72f)); };
			tr.cells.push_back(icon);
			const char* brush = r.denied ? "Faint" : (r.watched ? "Text" : "Dim");
			tr.cells.push_back(ui::Cell::T(r.exe + (r.changed ? "  \xE2\x80\xA2" : ""), brush));
			ui::Cell pid = ui::Cell::T(std::to_string(r.pid), "Dim");
			pid.size = ui::FsNote;
			tr.cells.push_back(pid);
			ui::Cell cls = ui::Cell::T(r.classText, "Dim");
			cls.size = ui::FsNote;
			tr.cells.push_back(cls);
			ui::Cell aff = ui::Cell::T(r.ok ? r.hex : "", "Dim");
			aff.size = ui::FsNote;
			tr.cells.push_back(aff);
			ui::Cell rg = ui::Cell::T(r.ok ? r.ranges : "", "Dim");
			rg.size = ui::FsNote;
			tr.cells.push_back(rg);
			tr.cells.push_back(ui::Cell::T(r.watched ? "\xE2\x9C\x93" : "", "On"));
			trs.push_back(std::move(tr));
		}
		table_.SetRows(std::move(trs));
	}

	void LoadEdit()
	{
		const Row* r = Sel();
		uint64_t full = topo_["full_mask"].U64();
		loaded_ = (r != nullptr && r->ok && tv_.valid) ? (r->affinity & full) : 0;
		edit_ = loaded_;
		dirty_ = false;
	}

	/// BuildInfo's reads that the core answers (the eligibility, the ranges text of the edited mask).
	void Info()
	{
		const Row* r = Sel();
		elig_ = r != nullptr ? bridge::Eligibility(r->exe) : json::Value();
		EditText();
	}

	void EditText()
	{
		editText_ = ui::Hex(edit_) + "  " + bridge::Ranges(edit_);
	}

	void DrawInfo(bool autoOn)
	{
		const Row* rp = Sel();
		if (rp == nullptr)
		{
			ui::Note(L::T("Select a process. Only processes on the watchlist are changed; denylisted ones are never opened."));
			return;
		}
		Row& r = rows_[(size_t)sel_];
		ui::Head(r.exe);
		ui::Pair(L::T("PID"), std::to_string(r.pid));
		if (r.denied)
		{
			ui::Note(L::T("On the denylist (system-critical or integrity-sensitive): never opened, never changed."));
			return;
		}
		bool w = r.watched;
		if (ui::CheckRow("##watch", L::T("On the watchlist"), L::T("The program changes scheduling only for watched processes."), w))
		{
			std::string exe = r.exe;
			bridge::ProcessWatch(exe, w);
			for (Row& x : rows_)
				if (EqualsIgnoreCase(x.exe, exe)) x.watched = w;   // the table's rows stay as they were drawn (C#)
			Info();
			S->Refresh();
			return;
		}
		if (!r.ok || !tv_.valid)
		{
			ui::Note(L::T("The process cannot be read (no access)."));
			return;
		}
		const json::Value& split = topo_["split"];
		bool canSplit = split["can_split"].Bool();
		uint64_t game = split["game"].U64();
		ui::Pair(L::T("Affinity"), editText_);
		if (canSplit) ui::Pair(L::T("Check"), (edit_ & ~game) == 0 ? L::T("only game cores") : L::T("also non-game cores"));
		uint64_t edit = edit_;
		int clicked = ui::LpMap("##procmap", tv_, [&](const ui::LpSet& lp) {
			ui::LpLook l;
			l.fill = (lp.lp >= 64 || ((edit >> lp.lp) & 1) == 0) ? ui::C("Panel2") : ((canSplit && ((game >> lp.lp) & 1) == 0) ? ui::C("NonGame") : ui::C("Game"));
			return l;
		}, r.watched, 24.0f);
		if (clicked >= 0 && r.watched)
		{
			uint64_t num = edit_ ^ pa::Bit(clicked);
			if (num != 0)
			{
				edit_ = num;
				dirty_ = edit_ != loaded_;
				EditText();
			}
		}
		if (canSplit)
		{
			std::vector<ui::Leg> legs(2);
			legs[0].C = ui::C("Game");
			legs[0].Text = L::T("game cores");
			legs[1].C = ui::C("NonGame");
			legs[1].Text = L::T("non-game cores");
			ui::LegLine(legs);
		}
		ui::Note(r.watched ? L::T("Click processors to edit the mask, then Apply. The mask is saved as this program's rule and kept while it runs.") : L::T("Put the process on the watchlist to edit its mask."));
		if (autoOn) ui::Note(L::T("Auto is on: masks are set by Auto."));
	}

	void Apply()
	{
		const Row* r = Sel();
		if (r == nullptr || !dirty_) return;
		// SaveAffRule, Configure, Paused = false, then the worker: Sys.Pass; Did("{0}: mask {1} ({2}) saved and applied").
		pa::Quiet(bridge::ProcessApply(r->exe, edit_), [this](const json::Value&) { Refresh(); });
	}

	void Reset()
	{
		const Row* r = Sel();
		if (r == nullptr) return;
		// SaveAffRule(""), Configure, the worker: Engine.Revert(pid); Did("{0}: rule removed, {1} values put back").
		pa::Quiet(bridge::ProcessReset(r->pid, r->exe), [this](const json::Value&) { Refresh(); });
	}

	ui::Table table_;
	std::vector<Row> rows_;
	int sel_ = -1;
	uint64_t edit_ = 0, loaded_ = 0;
	bool dirty_ = false;
	json::Value topo_;
	ui::TopoView tv_;
	json::Value elig_;
	std::string editText_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Processes, ProcessesPage)
