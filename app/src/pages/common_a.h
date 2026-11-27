// Small helpers shared by the pages of area "app: pages A" (HomeView, QuickPage, GameThreadsPage, DevicesPage,
// MasksPage, ProcessesPage, SystemPanel). Owner: app pages A. Nothing here decides anything: it only draws, waits
// for jobs of the core and asks questions.
#pragma once
#include <atomic>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

#include "core.h"
#include "core_bridge.h"
#include "json.h"
#include "ui.h"
#include "widgets.h"

namespace pa {

// ---------------------------------------------------------------- jobs without the overlay
/// Opt.Queue / Opt.Run of the C# pages: a core job that shows no "Busy" overlay. done (optional) runs on the UI
/// thread with the job's JSON result once it finished. A job id 0 (nothing started) is ignored.
void Quiet(uint64_t job, std::function<void(const json::Value&)> done = {});
/// Polls the quiet jobs (every page of this area calls it from Draw and Tick); asks for a redraw while some run.
void PollQuiet();

// ---------------------------------------------------------------- MessageBox.Show(owner, text, title, OKCancel, Question, Cancel)
/// The question is shown by a thread of its own with the window as the owner (the owner is disabled like a modal
/// box), so the frame that asked is not stuck inside a nested message loop.
class Ask {
public:
	/// Starts the question (nothing when one is already open).
	void Start(const std::string& text, const std::string& title);
	/// -1 = no answer yet (or no question), 1 = OK, 0 = Cancel. The answer is returned once.
	int Poll();
	bool Open() const { return st_ && st_->open.load(); }

private:
	struct State {
		std::atomic<bool> open{false};
		std::atomic<int> answer{-1};
	};
	std::shared_ptr<State> st_;
};

// ---------------------------------------------------------------- text
/// Word-wrapped lines of a text at wrapW px (0 = no wrap), like TextWrapping.Wrap (and "\n" breaks).
std::vector<std::string> Wrap(ui::Face f, ui::Weight w, float em, std::string_view s, float wrapW);
/// Draws wrapped lines with a line height in px (0 = the font's); returns the height used.
float DrawWrapped(ImDrawList* dl, ImVec2 pos, ui::Face f, ui::Weight w, float em, ImU32 col, std::string_view s, float wrapW, float lineH = 0, bool center = false);
/// The height DrawWrapped would use.
float WrappedHeight(ui::Face f, ui::Weight w, float em, std::string_view s, float wrapW, float lineH = 0);

// ---------------------------------------------------------------- topology
/// The core's get "topo" (dos_core.h) as the shell's processor-map view (Ui.LpMap): l3_by_llc comes as a list,
/// Hybrid = more than one efficiency class (Topo.Hybrid), null = no topology (valid false).
ui::TopoView Topo(const json::Value& topo);
/// 1UL << lp for lp < 64, else 0 (the C# shifts only reach the first 64 processors).
inline uint64_t Bit(int lp) { return lp >= 0 && lp < 64 ? (1ull << lp) : 0ull; }

// ---------------------------------------------------------------- panels
/// Ui.Panel stretched over a fixed rect (a Grid / DockPanel child that fills): the Panel background, the 1 px
/// line, the corner radius and the two brackets. Returns the content rect (border and padding taken off).
ImRect FillPanel(ImRect r, ui::Thick pad, bool brackets = true);

// ---------------------------------------------------------------- legends
/// Leg.Build items (and an optional plain FsNote Dim text after them) in one WrapPanel with the given margin.
void LegendRow(const std::vector<ui::Leg>& legs, const std::string& tail, ui::Thick margin);

// ---------------------------------------------------------------- remembered heights
/// The height a block had in the last frame (the WPF Auto rows are measured before they are placed; here the
/// first frame uses the guess and the next one the measured size). Changed heights ask for one more frame.
class Heights {
public:
	float Get(const char* key, float guessPx) const;
	void Set(const char* key, float px);

private:
	std::vector<std::pair<std::string, float>> h_;
};

} // namespace pa
