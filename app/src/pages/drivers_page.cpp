// DriversPage.cs: the drivers to install by hand, top to bottom, with the makers' official pages. Owner: app pages B.
// The steps come from the core ("drivers" job = Hardware.Detect and the C# Step calls).
#include "pages_b.h"

namespace {

class DriversPage : public Page {
public:
	std::string Title() override { return L::T("Drivers"); }
	const char* Section() override { return "Hardware"; }

	void Build() override
	{
		steps_ = json::Value();
		guide_ = bridge::DriversGuideExists();
	}

	void Shown() override { Load(); }

	void Draw(ImRect area) override
	{
		pagesb::Scroll sc("##drivers", area);
		pagesb::BigTitle(L::T("Drivers"));
		pagesb::Wrap(L::T("Domain OS does not let Windows Update install drivers, so these are yours to install, top to bottom. Links lead to the makers' official pages only."), ui::FsNote, "Dim", ui::Thick(0, 2, 0, 12));
		{
			ui::Row row;
			if (ui::Btn(L::T("Refresh"))) Load();
			if (guide_ && ui::Btn(L::T("Open the step-by-step guide"), 220)) bridge::DriversGuideOpen();
		}
		int n = 0;
		for (const json::Value& s : steps_.Arr())
		{
			ImGui::PushID(n++);
			Step(s);
			ImGui::PopID();
		}
	}

private:
	void Load()
	{
		uint64_t id = bridge::Drivers();
		if (id == 0) return;
		S->Busy(L::T("Reading the hardware…"), id, [this](const json::Value& r) {
			steps_ = json::Value();
			if (r["facts"].Bool()) steps_ = r["steps"];
		});
	}

	void Step(const json::Value& s)
	{
		ui::BeginCard(11);
		pagesb::Medium(s["name"].Str());
		pagesb::PillBrush(s["state"].Str(), false, s["bad"].Bool() ? "Warn" : "Text", ui::Thick(0, 6, 0, 0));
		for (const json::Value& h : s["how"].Arr()) pagesb::Wrap(h.Str(), ui::FsNote, "Text", ui::Thick(0, 5, 0, 0));
		pagesb::Wrap(s["why"].Str(), ui::FsNote, "Dim", ui::Thick(0, 7, 0, 0));
		const auto& links = s["links"].Arr();
		if (!links.empty())
		{
			ui::Row row(ui::Thick(0, 10, 0, 0));
			bool first = true;
			int k = 0;
			for (const json::Value& l : links)
			{
				ui::ButtonStyle b = ui::BtnStyle(190);
				if (first) ui::Primary(b);
				first = false;
				if (ui::Button(l["text"].Str() + "##link" + std::to_string(k++), b)) bridge::OpenLink(l["url"].Str());
			}
		}
		ui::EndCard();
	}

	json::Value steps_;
	bool guide_ = false;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Drivers, DriversPage)
