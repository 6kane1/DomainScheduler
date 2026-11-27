// DevicesPage.cs ("Devices"): the interrupt layout card (apply / restore with a backup and the boot guard), the
// device list with MSI, pins and priorities, and the facts of one device with the advanced pin editor.
// Owner: app pages A.
#include <cmath>
#include <algorithm>
#include <cstdlib>
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


class DevicesPage : public Page {
public:
	DevicesPage()
	{
		table_.SelectionChanged = [this]() {
			sel_ = atoi(table_.Selected().c_str());
			if (sel_ < 0 || sel_ >= rows_) sel_ = -1;
			LoadFacts();
		};
	}

	std::string Title() override { return L::T("Devices"); }
	const char* Section() override { return "Hardware"; }

	void Build() override
	{
		tv_ = pa::Topo(bridge::Topo());
		sel_ = -1;
		table_.SetColumns({{L::T("Device"), 0}, {L::T("MSI"), 46}, {L::T("Pinned to"), 90}, {L::T("Priority"), 70}});
		table_.SetRows({});
		rows_ = 0;
		facts_ = json::Value();
		autoState_ = Reading;
		plan_ = json::Value();
	}

	void Shown() override
	{
		DrawAuto();
		Refresh();
	}

	void Tick() override { pa::PollQuiet(); }

	void Draw(ImRect area) override
	{
		pa::PollQuiet();
		int answer = ask_.Poll();
		if (answer == 1)
			S->Busy(L::T("Writing…"), bridge::DevicesApplyPlan(), [this](const json::Value&) {
				DrawAuto();
				Refresh();
			});
		ImGui::SetCursorScreenPos(area.Min);
		ui::Box box(area.Min.x, area.Max.x);
		bool refresh = false;
		{
			ui::Row row;
			refresh = ui::Btn(L::T("Refresh") + "##refresh");
		}
		// The interrupt layout card (margin bottom 12), docked at the top.
		ui::BeginPanel(ui::Thick(15, 13, 15, 13), true, ui::Thick(0, 0, 0, 12));
		Auto();
		ui::EndPanel();
		// Grid: the table (*) and the facts card (380, margin 12,0,0,12).
		float y = ImGui::GetCursorScreenPos().y;
		// Native-only: the card gives way in a narrow window (at most 40 % of the page, at least 240).
		float cardW = std::max(std::min(ui::S(380), area.GetWidth() * 0.40f), ui::S(240));
		ImRect tableR(ImVec2(area.Min.x, y), ImVec2(std::max(area.Min.x, area.Max.x - cardW - ui::S(12)), area.Max.y));
		ImRect cardR(ImVec2(area.Max.x - cardW, y), ImVec2(area.Max.x, area.Max.y - ui::S(12)));
		if (area.GetWidth() < ui::S(640))
		{
			// Very narrow: the card goes below the table instead of beside it.
			float th = std::round((area.Max.y - y) * 0.5f);
			tableR = ImRect(ImVec2(area.Min.x, y), ImVec2(area.Max.x, y + th));
			cardR = ImRect(ImVec2(area.Min.x, y + th + ui::S(12)), ImVec2(area.Max.x, area.Max.y - ui::S(12)));
		}
		if (tableR.GetWidth() > 1 && tableR.GetHeight() > 1)
		{
			ImGui::SetCursorScreenPos(tableR.Min);
			ui::Box tb(tableR.Min.x, tableR.Max.x);
			table_.Draw("##devs", tableR.GetSize());
		}
		if (cardR.GetHeight() > ui::S(30))
		{
			ImRect in = pa::FillPanel(cardR, ui::Thick(15, 13, 15, 13));
			ImGui::SetCursorScreenPos(in.Min);
			ui::BeginScroll("##devinfo", in.GetSize());
			{
				ui::Box b(ImGui::GetCursorScreenPos().x, ui::BoxRight());
				Facts();
			}
			ui::EndScroll();
		}
		if (refresh) Refresh();
	}

private:
	enum AutoState { Reading, Failed, Ready };

	/// DrawAuto: the plan is read off the UI thread (IrqAuto.PlanNow, Task.Run: no overlay).
	void DrawAuto()
	{
		autoState_ = Reading;
		pa::Quiet(bridge::DevicesPlan(), [this](const json::Value& v) {
			if (!v.IsObj())
			{
				autoState_ = Failed;
				autoError_ = "";
				return;
			}
			if (!v["error"].IsNull())
			{
				autoState_ = Failed;
				autoError_ = v["error"].Str();
				return;
			}
			plan_ = v;
			autoState_ = Ready;
		});
	}

	void Auto()
	{
		ui::Head(L::T("Interrupt layout"));
		// 1.16.0 Test 1.9: no big plan table on the page any more. One line on what it does; the plan itself is shown in
		// the confirmation of "Apply the layout", and nothing is offered when the layout is already in place.
		ui::Note(L::T("The mouse, the graphics card and the network card get cores of their own. Works after a restart; every value is backed up and comes back by itself if Windows does not start."));
		if (autoState_ == Reading)
		{
			ui::Note(L::T("Reading the devices…"));
			return;
		}
		if (autoState_ == Failed)
		{
			ui::Note(L::F("Could not read the devices: {0}", {autoError_}));
			return;
		}
		const json::Value& p = plan_;
		if (p["empty"].Bool())
		{
			ui::Note(L::T("No devices with interrupt settings were found."));
			return;
		}
		bool inPlace = p["in_place"].Bool();
		if (inPlace) ui::Text(p["in_place_text"].Str(), ui::FsBody, "On", false, true, ui::Thick(0, 6, 0, 6));
		if (!p["flag_open"].Bool())
		{
			ui::Row row;
			if (ui::ConfirmBtn("##allowauto", L::T("Allow device policy changes")))
			{
				bridge::DevicesAllow();
				DrawAuto();
			}
			return;
		}
		bool backup = p["has_backup"].Bool();
		if (inPlace && !backup) return;   // an empty row is not added
		ui::Row row;
		if (!inPlace)
		{
			ui::ButtonStyle s = ui::BtnStyle(190);
			ui::Primary(s);
			if (ui::Button(L::T("Apply the layout") + "##applylayout", s)) ask_.Start(p["confirm_text"].Str(), p["confirm_title"].Str());
		}
		if (backup && ui::ConfirmBtn("##restorelayout", L::T("Restore all devices")))
			S->Busy(L::T("Writing…"), bridge::DevicesRestore("layout"), [this](const json::Value&) {
				DrawAuto();
				Refresh();
			});
	}

	void Refresh()
	{
		S->Busy(L::T("Reading devices…"), bridge::DevicesList(), [this](const json::Value& v) {
			std::vector<ui::TableRow> trs;
			int i = 0;
			for (const auto& r : v["rows"].Arr())
			{
				ui::TableRow tr;
				tr.tag = std::to_string(i++);
				tr.cells.push_back(ui::Cell::T(r["name"].Str()));
				tr.cells.push_back(ui::Cell::T(r["msi"].Bool() ? "\xE2\x9C\x93" : "", "On"));
				ui::Cell pin = ui::Cell::T(r["pinned"].Str(), "Dim");
				pin.size = ui::FsNote;
				tr.cells.push_back(pin);
				ui::Cell pr = ui::Cell::T(r["prio"].Str(), "Dim");
				pr.size = ui::FsNote;
				tr.cells.push_back(pr);
				trs.push_back(std::move(tr));
			}
			rows_ = i;
			sel_ = -1;
			table_.SetRows(std::move(trs));
		});
	}

	void LoadFacts()
	{
		if (sel_ < 0) return;
		int row = sel_;
		S->Busy(L::T("Reading the device…"), bridge::DeviceFacts(row), [this, row](const json::Value& f) {
			if (f.IsObj() && row == sel_) SetFacts(f);
		});
	}

	void SetFacts(const json::Value& f)
	{
		facts_ = f;
		std::vector<ui::Opt> lo, po;
		for (const auto& o : f["lp_options"].Arr()) lo.emplace_back(o["key"].Str(), o["label"].Str());
		for (const auto& o : f["prio_options"].Arr()) po.emplace_back(o["key"].Str(), o["label"].Str());
		lp_ = ui::Combo(150, lo);
		prio_ = ui::Combo(150, po);
	}

	void Facts()
	{
		const json::Value& f = facts_;
		if (!f.IsObj())
		{
			ui::Note(L::T("Select a device. Interrupt pins are only read here; the game mask steps around dedicated ones."));
			return;
		}
		ui::Head(f["name"].Str());
		ui::FactGrid fg(1);
		fg.Add(L::T("Driver"), f["driver"].Str()).Add(L::T("Location"), f["location"].Str()).Add(L::T("Windows started"), f["boot"].Str()).Add(L::T("Restarted by the program"), f["restarted"].Str());
		fg.Draw();
		ui::Head(L::T("Interrupts"));
		if (f["resources"].size() == 0) ui::Note(L::T("No interrupt resources."));
		for (const auto& r : f["resources"].Arr()) ui::Note(r.Str());
		if (!f["topo_ok"].Bool() || !tv_.valid) return;
		uint64_t mine = f["mine"].U64(), others = f["others"].U64();
		ui::LpMap("##devmap", tv_, [&](const ui::LpSet& lp) {
			uint64_t bit = pa::Bit(lp.lp);
			ui::LpLook l;
			l.fill = (mine & bit) != 0 ? ui::C("Irq") : ui::C("Panel2");
			l.ring = (others & bit) != 0 ? ui::C("Dim") : 0;
			return l;
		}, false, 22.0f);
		std::vector<ui::Leg> legs(2);
		legs[0].C = ui::C("Irq");
		legs[0].Text = L::T("this device");
		legs[1].C = ui::C("Dim");
		legs[1].Look = "ring";
		legs[1].Text = L::T("other devices");
		ui::LegLine(legs);
		ui::Head(L::T("Change the pin (advanced)"));
		int write = f["write"].Int();
		if (write == 1)
		{
			ui::Note(f["write_note"].Str());
			ui::Row row;
			if (ui::ConfirmBtn("##allowdev", L::T("Allow device policy changes")))
			{
				bridge::DevicesAllow();
				// Draw(row, f) again: the core answers the other checks (game, storage, MSI) for the same row.
				int idx = f["index"].Int();
				pa::Quiet(bridge::DeviceFacts(idx), [this, idx](const json::Value& nf) {
					if (nf.IsObj() && facts_["index"].Int() == idx) SetFacts(nf);
				});
			}
			return;
		}
		if (write != 0)
		{
			ui::Note(f["write_note"].Str());
			return;
		}
		{
			ui::Row row;
			lp_.Draw("##lp");
			prio_.Draw("##prio");
		}
		{
			ui::Row row;
			if (ui::ConfirmBtn("##write", L::T("Write and restart the device")))
				S->Busy(L::T("Writing…"), bridge::DeviceWrite(f["index"].Int(), lp_.Key(), prio_.Key()), [this](const json::Value&) { Refresh(); });
			if (ui::ConfirmBtn("##restoredev", L::T("Restore all devices")))
				S->Busy(L::T("Writing…"), bridge::DevicesRestore("device"), [this](const json::Value&) { Refresh(); });
		}
		ui::Note(L::T("The device restarts for a moment (a network card drops the connection). The original values are backed up first and put back if the read-back differs."));
	}

	ui::Table table_;
	int rows_ = 0;
	int sel_ = -1;
	ui::TopoView tv_;
	json::Value facts_;
	json::Value plan_;
	AutoState autoState_ = Reading;
	std::string autoError_;
	ui::Combo lp_, prio_;
	pa::Ask ask_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Devices, DevicesPage)
