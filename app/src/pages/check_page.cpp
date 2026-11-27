// CheckPage.cs (1.16.0 Test 1): the measurement of Domain SchedTest inside Toolbox. One run = 30 s of the game:
// frames and how long the game's threads waited for a processor. The run itself (countdown, trace, beeps, the
// report on the desktop) is done by the core ("check_start" / "check_stop"); the page shows its state.
// Owner: app pages B.
#include "pages_b.h"

namespace {

// CheckPage.RunSeconds, Warmup, Countdown.
constexpr int RunSeconds = 30, Warmup = 6, Countdown = 10;

class CheckPage : public Page {
public:
	std::string Title() override { return L::T("Check"); }

	void Build() override { data_ = bridge::Check(); }

	// Shown(): "Ready." unless a run is going. Build made a new, empty results list: the results of the last run
	// are shown only right after it (DrawResults at its end), not when the page is opened again.
	void Shown() override
	{
		data_ = bridge::Check();
		fresh_ = !data_["running"].Bool();
	}

	void Tick() override { data_ = bridge::Check(); }

	void Draw(ImRect area) override
	{
		bool running = data_["running"].Bool();
		if (running)
		{
			// The countdown and the seconds of the run: read again twice a second while it goes.
			if (app::Now() - read_ >= 0.5)
			{
				read_ = app::Now();
				data_ = bridge::Check();
				running = data_["running"].Bool();
			}
			app::WakeIn(500);
		}
		pagesb::Scroll sc("##check", area);
		pagesb::BigTitle(L::T("Check"));
		pagesb::Wrap(L::T("Measures the game while you play: frames and how long the game's threads wait for a processor. Play the same scene the whole time (for example a deathmatch with bots). Nothing in the game is changed."), ui::FsNote, "Dim", ui::Thick(0, 2, 0, 12));
		ui::BeginCard(12);
		// DrawState: the game line, then the state text.
		pagesb::Wrap(data_["game"].Str(), 0, data_["game_found"].Bool() ? "Strong" : "Warn");
		if (running) fresh_ = false;
		std::string st = data_["state"].Str();
		const char* brush = data_["state_brush"].Str() == "Accent" ? "Accent" : data_["state_brush"].Str() == "Warn" ? "Warn" : "Text";
		if (fresh_ || (st.empty() && !running))
		{
			st = L::T("Ready.");
			brush = "Text";
		}
		pagesb::Wrap(st, ui::FsNote, brush, ui::Thick(0, 4, 0, 0));
		{
			ui::Row row(ui::Thick(0, 10, 0, 0));
			ui::ButtonStyle one = ui::BtnStyle(150);
			ui::Primary(one);
			one.enabled = !running;
			if (ui::Button(L::T("One run (30 s)") + "##one", one))
			{
				fresh_ = false;
				bridge::CheckStart();
				data_ = bridge::Check();
			}
			ui::ButtonStyle stop = ui::BtnStyle(90);
			stop.enabled = running;
			if (ui::Button(L::T("Stop") + "##stop", stop))
			{
				bridge::CheckStop();
				data_ = bridge::Check();
			}
		}
		ui::Note(L::F("Press the button, switch to the game within {0} s and play. The run lasts {1} s (plus {2} s while the trace starts); a short beep marks its start and end. To compare two variants, make the same number of runs of each.",
		              {std::to_string(Countdown), std::to_string(RunSeconds), std::to_string(Warmup)}),
		         ui::Thick(0, 10, 0, 0));
		ui::EndCard();
		const auto& results = data_["results"].Arr();
		if (!results.empty() && !fresh_)
		{
			ui::Group g(0, ui::Thick(0, 14, 0, 0));
			for (const json::Value& r : results)
			{
				ui::BeginCard(10);
				ui::Text(r["head"].Str(), 0, "Strong", true);
				pagesb::Wrap(r["frames"].Str(), ui::FsNote, "Text");
				pagesb::Wrap(r["waits"].Str(), ui::FsNote, "Text");
				ui::EndCard();
			}
		}
	}

private:
	json::Value data_;
	bool fresh_ = true;
	double read_ = 0;
};

} // namespace

DOS_REGISTER_PAGE(PageId::Check, CheckPage)
