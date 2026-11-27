// ChangesPage.cs: the list of system changes the Domain OS playbook applies, each one switchable here.
// Owner: app pages B. The catalog, states and texts come from the core (get "changes").
#include "pages_b.h"

namespace {

class ChangesPage : public Page {
public:
	std::string Title() override { return L::T("System changes"); }
	const char* Section() override { return "System"; }

	void Build() override { data_ = bridge::Changes(); }

	void Draw(ImRect area) override
	{
		bool redraw = false;
		{
			pagesb::Scroll sc("##changes", area);
			pagesb::BigTitle(L::T("System changes"));
			pagesb::Wrap(L::T("This is the list the Domain OS playbook applies. It does not touch Defender, Secure Boot, memory integrity or anti-cheats. Each change can be switched back here."), ui::FsNote, "Dim", ui::Thick(0, 2, 0, 12));
			{
				ui::Row row;
				if (ui::Btn(L::T("Switch on all recommended"), 200))
				{
					bridge::TweaksRecommended();
					redraw = true;
				}
			}
			int n = 0;
			for (const json::Value& g : data_["groups"].Arr())
			{
				ui::Text(g["title"].Str(), ui::FsSmall, "Dim", false, false, ui::Thick(2, 8, 0, 6));
				for (const json::Value& t : g["items"].Arr())
				{
					ImGui::PushID(n++);
					if (Card(t)) redraw = true;
					ImGui::PopID();
				}
			}
		}
		if (redraw) data_ = bridge::Changes();   // Draw(): the list is built again with the new states
	}

private:
	static void Line(const std::string& label, const std::string& text, const char* brush)
	{
		// DockPanel, margin top 3: the label (FsSmall Faint, width 96, margin top 2) docked left, the text wrapped.
		ui::Row row(ui::Thick(0, 3, 0, 0));
		ui::TextStyle l;
		l.size = ui::FsSmall;
		l.brush = "Faint";
		l.width = 96;
		l.margin = ui::Thick(0, 2, 0, 0);
		ui::TextEx(label, l);
		ui::Group g;
		pagesb::Wrap(text, ui::FsNote, brush);
	}

	bool Card(const json::Value& t)
	{
		int state = t["state"].Int();
		bool on = state == 1;
		bool clicked;
		ui::BeginCard(11);
		ui::ButtonStyle b = ui::BtnStyle(110);
		if (t["primary"].Bool()) ui::Primary(b);
		std::string label = t["button"].Str();
		std::string title = t["title"].Str();
		std::string pill = t["pill"].Str();
		float h = std::max(ui::FontPx(ui::Face::Ui, ui::FsBody), pagesb::PillH());
		clicked = pagesb::DockRightButton([&](const ui::ButtonStyle& s) { return ui::Button(label + "##toggle", s); }, label, b, h, [&] {
			ui::Row row(ui::Thick(0));
			float th = ui::FontPx(ui::Face::Ui, ui::FsBody), ph = pagesb::PillH();
			pagesb::Medium(title, "Strong", false, ui::Thick(0, (h - th) * 0.5f / ui::Dpi(), 0, 0));
			ui::Pill(pill, on, ui::Thick(10, (h - ph) * 0.5f / ui::Dpi(), 0, 0));
		});
		if (clicked) bridge::TweakToggle(t["id"].Str());
		Line(L::T("What"), t["what"].Str(), "Text");
		Line(L::T("Why"), t["why"].Str(), "Dim");
		Line(L::T("You give up"), t["cost"].Str(), "Dim");
		Line(L::T("Changes"), t["changes"].Str(), "Faint");
		ui::EndCard();
		return clicked;
	}

	json::Value data_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Changes, ChangesPage)
