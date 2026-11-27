// PowerPage.cs: only the choice of the power plan (powercfg /list, /setactive through the core). Owner: app pages B.
#include "pages_b.h"

namespace {

class PowerPage : public Page {
public:
	std::string Title() override { return L::T("Power"); }
	const char* Section() override { return "Hardware"; }

	void Build() override { list_ = json::Value(); }

	void Shown() override { Load(); }

	void Draw(ImRect area) override
	{
		pagesb::Scroll sc("##power", area);
		pagesb::BigTitle(L::T("Power plan"));
		pagesb::Wrap(L::T("Only the choice of the plan. Core parking, boost mode and similar hidden parameters are not edited here: on current processors changing them by hand gives no repeatable gain and often costs boost frequency."), ui::FsNote, "Dim", ui::Thick(0, 2, 0, 12));
		if (list_.IsNull()) return;
		if (list_["error"].IsStr())
		{
			ui::Note(list_["error"].Str());
			return;
		}
		int n = 0;
		for (const json::Value& p : list_["plans"].Arr())
		{
			ImGui::PushID(n++);
			Plan(p);
			ImGui::PopID();
		}
	}

private:
	void Load()
	{
		uint64_t id = bridge::PowerList();
		if (id == 0) return;
		S->Busy(L::T("Reading the power plans…"), id, [this](const json::Value& r) { list_ = r; });
	}

	void Plan(const json::Value& p)
	{
		bool active = p["active"].Bool();
		std::string guid = p["guid"].Str(), name = p["name"].Str();
		ui::BeginCard(11);
		ui::ButtonStyle b = ui::BtnStyle(130);
		b.enabled = !active;
		std::string label = L::T("Use this plan");
		float h = std::max(ui::FontPx(ui::Face::Ui, ui::FsBody), pagesb::PillH());
		bool clicked = pagesb::DockRightButton([&](const ui::ButtonStyle& s) { return ui::Button(label + "##use", s); }, label, b, h, [&] {
			ui::Row row(ui::Thick(0));
			float th = ui::FontPx(ui::Face::Ui, ui::FsBody), ph = pagesb::PillH();
			pagesb::Medium(name, "Strong", false, ui::Thick(0, (h - th) * 0.5f / ui::Dpi(), 0, 0));
			ui::Pill(active ? L::T("Active") : L::T("Not active"), active, ui::Thick(10, (h - ph) * 0.5f / ui::Dpi(), 0, 0));
		});
		if (clicked)
		{
			uint64_t id = bridge::PowerSet(guid, name);
			if (id != 0) S->Busy(L::T("Writing…"), id, [this](const json::Value&) { Load(); });
		}
		pagesb::Wrap(p["explain"].Str(), ui::FsNote, "Dim", ui::Thick(0, 6, 0, 0));
		ui::EndCard();
	}

	json::Value list_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Power, PowerPage)
