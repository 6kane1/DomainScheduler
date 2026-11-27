// UpdatePage.cs (1.16.0 Test 1.9): the update card (UpdateCard with always: true), the daily check and the tester
// key with the update channel. Owner: app pages B. The card's texts come from the core (get "update").
#include "pages_b.h"

namespace {

class UpdatePage : public Page {
public:
	std::string Title() override { return L::T("Updates and testing"); }
	const char* Section() override { return "Settings"; }

	void Build() override
	{
		data_ = bridge::Update();
		check_ = data_["enabled"].Bool(true);
		typed_.clear();
		ShowTester();
	}

	// UpdateService.Changed -> UpdateCard.Refresh: the card is read again with every refresh of the shell.
	void Tick() override { data_ = bridge::Update(); }

	void Draw(ImRect area) override
	{
		json::Value r;
		if (save_.Poll(r))
		{
			if (!r["ok"].Bool())
			{
				state_ = r["state"].Str();
				brush_ = "Warn";
			}
			else
			{
				typed_.clear();
				data_ = bridge::Update();
				ShowTester();
			}
		}
		pagesb::Scroll sc("##update", area);
		Card();
		ui::BeginCard();
		if (ui::CheckRow("checkupd", L::T("Check for updates"), L::T("Once a day. Only a small file is read from the Domain OS site; nothing about this PC is sent. Nothing installs until you press Update."), check_))
			bridge::CheckUpdates(check_);
		ui::EndCard();
		ui::BeginCard();
		Tester();
		ui::EndCard();
	}

private:
	/// UpdateCard(always: true).Refresh: Ui.Card with the margin 0,0,0,8.
	void Card()
	{
		ui::BeginPanel(ui::Thick(15, 13, 15, 13), true, ui::Thick(0, 0, 0, 8));
		ui::Head(data_["head"].StrOr(L::T("Updates").c_str()));
		pagesb::Wrap(data_["this_text"].Str(), ui::FsNote, "Dim");
		const json::Value& t = data_["toolbox"];
		if (t.IsObj())
		{
			ui::Text(t["title"].Str(), ui::FsBody, "Strong", true, true);
			if (!t["notes"].Str().empty()) pagesb::Wrap(t["notes"].Str(), ui::FsNote, "Text");
			{
				ui::Row row;
				ui::ButtonStyle b = ui::BtnStyle();
				ui::Primary(b);
				b.enabled = t["can_update"].Bool();
				if (ui::Button(L::T("Update") + "##install", b)) bridge::UpdateInstall();
			}
			ui::Note(t["note"].Str());
		}
		const json::Value& p = data_["playbook"];
		if (p.IsObj())
		{
			ui::Text(p["title"].Str(), ui::FsBody, "Strong", true, true);
			if (!p["notes"].Str().empty()) pagesb::Wrap(p["notes"].Str(), ui::FsNote, "Text");
			if (p["file"].IsStr())
			{
				ui::Row row;
				if (ui::Btn(L::T("Show the file"))) bridge::PlaybookShow();
			}
			else
			{
				ui::Row row;
				if (ui::Btn(L::T("Download"))) bridge::PlaybookDownload();
				if (ui::Btn(L::T("Open the website"))) bridge::PlaybookSite();
			}
			ui::Note(p["note"].Str());
		}
		if (data_["busy"].IsStr()) pagesb::Wrap(data_["busy"].Str(), ui::FsNote, "Accent");
		if (data_["error"].IsStr()) pagesb::Wrap(data_["error"].Str(), ui::FsNote, "Warn");
		pagesb::Wrap(data_["when"].Str(), ui::FsNote, "Dim");
		{
			ui::Row row;
			if (ui::Btn(L::T("Check now"))) bridge::UpdateCheckNow();
			if (data_["prev"].Bool() && ui::ConfirmBtn("rollback", L::T("Return to the previous version"))) bridge::UpdateRollback();
		}
		ui::EndPanel();
	}

	/// TesterCard's Show(): the state line and the channel button from the key and the channel.
	void ShowTester()
	{
		const json::Value& t = data_["tester"];
		state_ = t["state"].Str();
		brush_ = t["on"].Bool() ? "Accent" : "Dim";
	}

	void Tester()
	{
		const json::Value& t = data_["tester"];
		ui::Head(L::T("Tester"));
		ui::Note(L::T("Testers get a key in the closed Discord channel. With the key Toolbox also receives the test builds; without it, only stable releases."));
		{
			ui::Row row;
			ui::Input("##testerkey", typed_, 360);
			ui::ButtonStyle sv = ui::BtnStyle();
			sv.enabled = !save_.Running();
			if (ui::Button(L::T("Save key") + "##savekey", sv))
			{
				state_ = L::T("Checking the key…");
				brush_ = "Dim";
				save_.Start(bridge::TesterKeySave(typed_));
			}
			if (ui::Btn(L::T("Remove key")))
			{
				bridge::TesterKeyRemove();
				data_ = bridge::Update();
				ShowTester();
			}
		}
		pagesb::Wrap(state_, ui::FsNote, brush_);
		// 1.16.0: the tester chooses the channel without removing the key.
		if (t["channel_visible"].Bool())
		{
			ui::ButtonStyle b = ui::BtnStyle(190);
			b.margin = ui::Thick(0, 8, 0, 0);
			if (ui::Button(t["channel_button"].Str() + "##channel", b))
			{
				bridge::TesterChannel();
				data_ = bridge::Update();
				ShowTester();
			}
		}
	}

	json::Value data_;
	bool check_ = true;
	std::string typed_;
	std::string state_;
	const char* brush_ = "Dim";
	pagesb::Job save_;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Update, UpdatePage)
