// MasksPage.cs ("Masks"): the mask of watched processes (spec, IRQ refinement, nosmt / nocore0, manual edits on
// the processor map), the result, and the core reservation. Owner: app pages A.
// The page keeps only its draft (spec, irq, the two switches, add / drop); every result is the core's
// (get "masks_preview" = GameMask.Build + Masks.Resolve on the draft).
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

class MasksPage : public Page {
public:
	std::string Title() override { return L::T("Masks"); }

	void Build() override
	{
		topoJ_ = bridge::Topo();
		topo_ = pa::Topo(topoJ_);
		if (!topo_.valid) return;
		LoadStored();
		Recompute();
	}

	void Tick() override { pa::PollQuiet(); }

	void Draw(ImRect area) override
	{
		pa::PollQuiet();
		ImGui::SetCursorScreenPos(area.Min);
		if (!topo_.valid)
		{
			ui::Box b(area.Min.x, area.Max.x);
			ui::Text(L::T("CPU: the topology could not be read"), 0, "Warn");
			return;
		}
		bool changed = false;
		ui::BeginScroll("##masks", area.GetSize());
		{
			ui::Box box(ImGui::GetCursorScreenPos().x, ui::BoxRight());
			// ---- Mask of watched processes
			ui::BeginCard();
			ui::Head(L::T("Mask of watched processes"));
			ui::Note(L::T("Used by presets of watched processes that have no rule of their own. Auto picks its mask itself."));
			{
				ui::Row row;
				if (spec_.Draw("##spec"))
				{
					spec = spec_.Key();
					add_ = drop_ = 0;
					changed = true;
				}
				if (irqC_.Draw("##irq"))
				{
					irq = irqC_.Key();
					changed = true;
				}
			}
			if (ui::CheckRow("##nosmt", L::T("One processor per core (nosmt)"), L::T("Leaves the second SMT thread of every core out of the mask."), noSmt_)) changed = true;
			if (ui::CheckRow("##nocore0", L::T("Without the first core (nocore0)"), L::T("Windows and drivers load the first core most."), noCore0_)) changed = true;
			bool apply = false, reset = false;
			{
				ui::Row row;
				ui::ButtonStyle a = ui::BtnStyle();
				a.enabled = dirty_;
				apply = ui::Button(L::T("Apply") + "##apply", a);
				ui::ButtonStyle r = ui::BtnStyle();
				r.enabled = dirty_;
				reset = ui::Button(L::T("Reset") + "##reset", r);
			}
			ui::EndCard();
			// ---- Result
			ui::BeginCard();
			ui::Head(L::T("Result"));
			ui::FactGrid fg(1);
			fg.Add(L::T("Split"), p_["split_text"].Str()).Add(L::T("Mask"), p_["mask_text"].Str()).Add(L::T("Background"), p_["background_text"].Str());
			fg.Draw();
			for (const auto& w : p_["warn"].Arr()) ui::Text(w.Str(), ui::FsNote, "Warn", false, true);
			for (const auto& n : p_["notes"].Arr()) ui::Note(n.Str());
			bool canSplit = p_["can_split"].Bool();
			uint64_t game = p_["game"].U64(), irqDrop = p_["irq_drop"].U64(), fin = p_["final"].U64(), base = p_["base"].U64();
			int clicked = ui::LpMap("##maskmap", topo_, [&](const ui::LpSet& lp) {
				uint64_t bit = pa::Bit(lp.lp);
				ui::LpLook l;
				l.fill = !canSplit ? ui::C("Panel2") : ((game & bit) != 0 ? ui::C("Game") : ui::C("NonGame"));
				l.ring = (irqDrop & bit) != 0 ? ui::C("Irq") : ((fin & bit) != 0 ? ui::C("Text") : 0);
				return l;
			}, canSplit);
			if (clicked >= 0 && canSplit)
			{
				uint64_t bit = pa::Bit(clicked);
				if ((base & bit) != 0) drop_ ^= bit;
				else add_ ^= bit;
				changed = true;
			}
			std::vector<ui::Leg> legs;
			if (canSplit)
			{
				ui::Leg g;
				g.C = ui::C("Game");
				g.Text = p_["legend_game"].Str();
				legs.push_back(g);
				ui::Leg ng;
				ng.C = ui::C("NonGame");
				ng.Text = L::T("non-game cores");
				legs.push_back(ng);
			}
			ui::Leg in;
			in.C = ui::C("Text");
			in.Look = "ring";
			in.Text = L::T("in the mask");
			legs.push_back(in);
			ui::Leg ex;
			ex.C = ui::C("Irq");
			ex.Look = "ring";
			ex.Text = L::T("excluded for IRQ");
			legs.push_back(ex);
			ui::LegLine(legs);
			ui::Note(p_["click_note"].Str());
			ui::EndCard();
			// ---- Core reservation
			ui::BeginCard();
			Reservation();
			ui::EndCard();
			if (apply) Apply();
			if (reset)
			{
				LoadStored();
				Recompute();
			}
		}
		ui::EndScroll();
		if (changed)
		{
			dirty_ = true;
			Recompute();
		}
	}

private:
	void LoadStored()
	{
		json::Value s = bridge::MasksStored();
		spec = s["spec"].Str();
		irq = s["irq"].Str();
		noSmt_ = s["nosmt"].Bool();
		noCore0_ = s["nocore0"].Bool();
		add_ = s["add"].U64();
		drop_ = s["drop"].U64();
		dirty_ = false;
	}

	bridge::Args Draft() const { return bridge::MaskDraft(spec, irq, noSmt_, noCore0_, add_, drop_); }

	/// Draw(): the options, the result and the reservation for the draft as it is now.
	void Recompute()
	{
		p_ = bridge::MasksPreview(Draft());
		std::vector<ui::Opt> o;
		for (const auto& x : p_["options"].Arr()) o.emplace_back(x["key"].Str(), x["label"].Str());
		spec_ = ui::Combo(260, o);
		if (!spec_.Select(spec)) spec_.Select("auto");
		std::vector<ui::Opt> io;
		for (const auto& x : p_["irq_options"].Arr()) io.emplace_back(x["key"].Str(), x["label"].Str());
		irqC_ = ui::Combo(260, io);
		irqC_.Select(irq);
		r_ = bridge::Reservation();
	}

	void Apply()
	{
		bridge::MasksApply(Draft());   // Settings, Configure, Did("Mask saved: ...")
		dirty_ = false;
		Recompute();
	}

	void Reservation()
	{
		ui::Head(L::T("Core reservation"));
		ui::Note(L::T("Windows keeps reserved cores free of ordinary programs. Works after a reboot; only the non-game cores can be reserved."));
		ui::Pair(L::T("Now"), r_["now"].Str());
		const std::string& ref = r_["refusals"].Str();
		if (!ref.empty()) ui::Note(ref);
		ui::Row row;
		ui::ButtonStyle a = ui::BtnStyle();
		a.enabled = r_["can_apply"].Bool();
		if (ui::ConfirmBtn("##resapply", L::T("Reserve the non-game cores"), a))
			S->Busy(L::T("Writing…"), bridge::ReservationApply(), [this](const json::Value&) { Recompute(); });
		ui::ButtonStyle r = ui::BtnStyle();
		r.enabled = r_["can_revert"].Bool();
		if (ui::ConfirmBtn("##resrevert", L::T("Remove the reservation"), r))
			S->Busy(L::T("Writing…"), bridge::ReservationRevert(), [this](const json::Value&) { Recompute(); });
	}

	json::Value topoJ_;
	ui::TopoView topo_;
	std::string spec = "auto";
	std::string irq = "Core";
	bool noSmt_ = false, noCore0_ = false, dirty_ = false;
	uint64_t add_ = 0, drop_ = 0;
	ui::Combo spec_, irqC_;
	json::Value p_, r_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Masks, MasksPage)
