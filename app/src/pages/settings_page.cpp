// SettingsPage.cs: General (built-in presets, start with Windows, live charts, re-apply period, language), Functions
// (the toggles), Feature flags, Watchlist, Factory reset. Owner: app pages B.
// Native only (owner's decision 3, PLAN.md): in the advanced layout the General card ends with the button
// "Reload hardware and presets" that does what QuickPage.Resync did (the core's "resync" job logs the result).
#include "pages_b.h"

namespace {

class SettingsPage : public Page {
public:
	std::string Title() override { return L::T("Settings"); }
	const char* Section() override { return "Settings"; }

	void Build() override
	{
		data_ = bridge::SettingsState();
		sysPresets_ = data_["sys_presets"].Bool();
		charts_ = data_["home_charts"].Bool(true);
		// The autostart row stays disabled until the task was read (Task.Run in C#).
		autoOn_ = false;
		autoEnabled_ = false;
		autoRead_.Start(bridge::AutostartRead());
		if (!autoRead_.Running()) autoEnabled_ = true;

		std::vector<ui::Opt> re;
		for (const json::Value& o : data_["reapply_options"].Arr()) re.emplace_back(o["key"].Str(), o["label"].Str());
		reapply_ = ui::Combo(150, re);
		if (!reapply_.Select(data_["reapply"].Str())) reapply_.Select("10000");

		std::vector<ui::Opt> langs;
		for (const json::Value& o : data_["languages"].Arr()) langs.emplace_back(o["key"].Str(), o["label"].Str());
		lang_ = ui::Combo(150, langs);
		lang_.Select(data_["lang"].Str());

		functions_.clear();
		for (const json::Value& f : data_["functions"].Arr()) functions_.push_back({f["key"].Str(), f["label"].Str(), f["hint"].Str(), f["value"].Bool()});
		flags_.clear();
		for (const json::Value& f : data_["flags"].Arr()) flags_.push_back({f["key"].Str(), f["label"].Str(), "", f["open"].Bool()});

		watch_.clear();
		for (const json::Value& w : data_["watch"].Arr()) watch_.push_back(w.Str());
		std::vector<ui::Opt> known;
		for (const json::Value& o : data_["known"].Arr()) known.emplace_back(o["key"].Str(), o["label"].Str());
		hasKnown_ = !known.empty();
		pick_ = ui::Combo(320, known);
		if (hasKnown_) pick_.SetIndex(0);
		rebuild_ = false;
	}

	void Draw(ImRect area) override
	{
		json::Value r;
		if (autoRead_.Poll(r))
		{
			autoOn_ = r["exists"].Bool();
			autoEnabled_ = true;
		}
		{
			pagesb::Scroll sc("##settings", area);
			DrawPage();
		}
		// Changes that C# answered with S.ShowPage<SettingsPage>() (a new page): built again after this frame.
		if (rebuild_) S->ShowPage(PageId::Settings);
	}

private:
	struct Toggle {
		std::string key, label, hint;
		bool value;
	};

	void DrawPage()
	{
		// 1.16.0 Test 1.9: updates and the tester key have their own page; the simple layout reaches it from here.
		if (S->Simple())
		{
			ui::ButtonStyle b = ui::BtnStyle(220);
			b.margin = ui::Thick(0, 0, 0, 8);
			if (ui::Button(L::T("Updates and testing") + "##toupd", b)) S->ShowPage(PageId::Update);
		}
		General();
		Functions();
		FlagCard();
		Watchlist();
		ui::BeginCard();
		ui::Head(L::T("Factory reset"));
		ui::Note(L::T("Puts everything back, deletes the settings, presets, autostart and data, then closes Toolbox."));
		{
			ui::Row row;
			if (ui::ConfirmBtn("factory", L::T("Factory reset"))) bridge::FactoryReset();
		}
		ui::EndCard();
	}

	void General()
	{
		ui::BeginCard();
		ui::Head(L::T("General"));
		if (ui::CheckRow("syspresets", L::T("Built-in system presets"), L::T("Thread priorities for Windows and common background programs (the bundled list)."), sysPresets_))
			bridge::SysPresets(sysPresets_);
		bool want = autoOn_;
		if (ui::CheckRow("autostart", L::T("Start with Windows"), L::T("A Task Scheduler task at logon; the program starts in the tray."), want, autoEnabled_))
		{
			autoOn_ = want;
			uint64_t id = bridge::AutostartSet(want);
			if (id != 0)
				S->Busy(L::T("Writing…"), id, [this](const json::Value& res) { autoOn_ = res["exists"].Bool(); });
		}
		// 1.14.0: the home screen's frame-time line is drawn 4 times a second while a game runs; weak PCs can switch it off.
		if (ui::CheckRow("charts", L::T("Live charts on the home screen"), L::T("Drawn only while the window is open and a game runs. Off: the numbers stay, without the line."), charts_))
			bridge::HomeCharts(charts_);
		{
			ui::Row row;
			LabelBeside(L::T("Re-apply presets every") + "  ");
			if (reapply_.Draw("##reapply")) bridge::ReapplySet(reapply_.Key());
		}
		{
			ui::Row row;
			LabelBeside(L::T("Language") + "  ");
			if (lang_.Draw("##lang"))
			{
				std::string k = lang_.Key();
				// The shell builds the page again when the core's language changed (Shell.OnLanguage).
				if (k != data_["lang"].Str()) bridge::LanguageSet(k);
			}
		}
		if (!S->Simple())
		{
			ui::Row row;
			if (ui::Btn(L::T("Reload hardware and presets"), 220)) resync_.Start(bridge::Resync());
		}
		json::Value ignored;
		resync_.Poll(ignored);
		ui::EndCard();
	}

	/// The text before a combo box in Ui.Row: vertically centred on the 32 DIP box.
	static void LabelBeside(const std::string& text)
	{
		float h = ui::FontPx(ui::Face::Ui, ui::FsBody);
		float top = std::max(0.0f, (ui::S(32) - h) * 0.5f) / std::max(0.01f, ui::Dpi());
		ui::Text(text, 0, "Text", false, false, ui::Thick(0, top, 0, 0));
	}

	void Functions()
	{
		ui::BeginCard();
		ui::Head(L::T("Functions"));
		for (Toggle& t : functions_)
		{
			if (ui::CheckRow(("fn_" + t.key).c_str(), t.label, t.hint, t.value)) bridge::SettingToggle(t.key.c_str(), t.value);
		}
		ui::EndCard();
	}

	void FlagCard()
	{
		ui::BeginCard();
		ui::Head(L::T("Feature flags"));
		ui::Note(L::T("Extra actions, closed by default. While closed, their buttons are hidden."));
		for (Toggle& f : flags_)
		{
			// The hint of a flag row is its key (CheckRow(label, key, open)).
			if (ui::CheckRow(("flag_" + f.key).c_str(), f.label, f.key, f.value))
			{
				bridge::FlagSet(f.key.c_str(), f.value);
				rebuild_ = true;
			}
		}
		ui::EndCard();
	}

	void Watchlist()
	{
		ui::BeginCard();
		ui::Head(L::T("Watchlist"));
		ui::Note(L::T("Only these programs are changed (plus the built-in presets and Auto's background). Add programs on the Processes page."));
		for (const std::string& e : watch_)
		{
			ui::Row row;
			if (ui::Button("\xE2\x9C\x95##w_" + e, ui::BtnStyle(30)))
			{
				bridge::WatchSet(e, false);
				rebuild_ = true;
			}
			LabelBeside(e);
		}
		if (hasKnown_)
		{
			ui::Row row;
			pick_.Draw("##known");
			if (ui::Btn(L::T("Add the known game")))
			{
				bridge::WatchSet(pick_.Key(), true);
				rebuild_ = true;
			}
		}
		if (watch_.empty()) ui::Note(L::T("The watchlist is empty: Auto will not pick a game."));
		ui::EndCard();
	}

	json::Value data_;
	bool sysPresets_ = false, charts_ = true;
	bool autoOn_ = false, autoEnabled_ = false;
	pagesb::Job autoRead_, resync_;
	ui::Combo reapply_, lang_, pick_;
	std::vector<Toggle> functions_, flags_;
	std::vector<std::string> watch_;
	bool hasKnown_ = false;
	bool rebuild_ = false;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Settings, SettingsPage)
