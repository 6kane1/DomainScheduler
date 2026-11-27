// BlackBoxPage.cs (1.16.1 Test 1): every stutter of the game Auto handles, explained in plain words with what to do.
// Recording runs only while Auto handles a game; nothing is changed by it. Owner: app pages B.
// The state line, the summary and the reports come from the core (get "blackbox"); the page only lays them out.
#include "pages_b.h"

namespace {

class BlackBoxPage : public Page {
public:
	std::string Title() override { return L::T("Black box"); }

	void Build() override
	{
		data_ = bridge::BlackBox();
	}

	void Tick() override { data_ = bridge::BlackBox(); }

	void Draw(ImRect area) override
	{
		pagesb::Scroll sc("##blackbox", area);
		pagesb::BigTitle(L::T("Black box"));
		pagesb::Wrap(L::T("Like a dashcam for the game: while Auto handles a game, the last 30 seconds are kept in memory. When a stutter happens, Toolbox shows what slowed the frame and what to do. Nothing is changed, nothing is written until a stutter."), ui::FsNote, "Dim");
		ui::Space(10);
		pagesb::Wrap(data_["state"].Str(), ui::FsNote, "Accent");
		ui::Space(8);
		const auto& reports = data_["reports"].Arr();
		ui::BeginCard();
		ui::Head(L::T("Last game"));
		pagesb::Wrap(reports.empty() ? L::T("No stutters recorded yet.") : data_["summary"].Str(), ui::FsBody, "Text");
		ui::EndCard();
		ui::BeginCard();
		ui::Head(L::T("Stutters, newest first"));
		if (reports.empty())
		{
			std::string note = data_["empty_note"].Str();
			ui::Note(note.empty() ? L::T("When a frame takes more than three times the usual and longer than two refreshes of the monitor, it appears here.") : note);
		}
		size_t n = 0;
		for (const json::Value& r : reports)
		{
			if (n++ >= 30) break;   // reports.Take(30)
			pagesb::BoxFrame b(nullptr, "Line", 0, ui::Thick(0, 10, 0, 10), {}, ui::Thick(0, 0, 0, 1));
			// The head: the badge docked right (FsNote, bold, its brush), the time and frame time in the rest.
			std::string badge = r["badge"].Str();
			std::string brush = r["brush"].StrOr("Dim");
			ImVec2 p = ImGui::GetCursorScreenPos();
			float right = ui::BoxRight();
			ImVec2 bs = ui::Measure(ui::Face::Ui, ui::Weight::Semi, ui::FsNote, badge);
			float h = std::max(ui::FontPx(ui::Face::Ui, ui::FsBody), ui::FontPx(ui::Face::Ui, ui::FsNote));
			ImDrawList* dl = ImGui::GetWindowDrawList();
			ui::TextAt(dl, ImVec2(right - bs.x, p.y + (h - ui::FontPx(ui::Face::Ui, ui::FsNote)) * 0.5f), ui::Face::Ui, ui::Weight::Semi, ui::FsNote, ui::C(brush), badge);
			{
				ui::Box bx(p.x, right - bs.x);
				ui::Text(r["when"].Str(), ui::FsBody, "Strong", true);
			}
			ImGui::SetCursorScreenPos(p);
			ui::Place(ImVec2(right - p.x, h));
			pagesb::Wrap(r["text"].Str(), ui::FsNote, "Text", ui::Thick(0, 4, 0, 2));
			pagesb::Wrap(r["advice"].Str(), ui::FsNote, "Dim");
		}
		ui::EndCard();
		if (ui::Btn(L::T("Open the folder"))) bridge::BlackBoxOpen();
	}

private:
	json::Value data_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::BlackBox, BlackBoxPage)
