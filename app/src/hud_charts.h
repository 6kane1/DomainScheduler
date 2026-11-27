// HudCharts.cs (HudInk, HudFrameChart, CoreMapView, SegSlider) and FrameChart.cs. Owner: app shell.
// Each chart keeps the data its Set() got and draws it into the rect it is given (Draw), like OnRender.
#pragma once
#include <functional>
#include <string>
#include <vector>

#include "ui.h"

namespace hud {

/// The parts of HudMath (Lib/Hud.cs) the drawings need. The core owns the same functions (area `auto`); these
/// copies only serve the drawing and must stay identical to Hud.cs.
constexpr double HitchFactor = 2.5;
inline bool IsHitch(double ms, double medianMs) { return medianMs > 0 && ms > HitchFactor * medianMs; }
double Nice(double v);
double ChartTop(double maxMs, double low1Ms, double medianMs);

/// CoreKind (Hud.cs): Background, Game, Interrupt.
enum class CoreKind { Background = 0, Game = 1, Interrupt = 2 };

/// The live frame-time line of the home screen: a minute in fixed buckets, the area under it, a dashed 1% low line
/// and amber rings on hitches. Not hit-testable.
class HudFrameChart {
public:
	/// max/endMax: HudMath.Buckets arrays (copied); median and 1% low in ms (0 = unknown).
	void Set(const std::vector<double>& max, const std::vector<double>& endMax, double medianMs, double low1Ms, const std::string& unit, const std::string& lowText);
	void Clear();
	/// Draws into r (px). Call every frame the chart is on screen.
	void Draw(ImRect r) const;

private:
	std::vector<double> max_, endMax_;
	double median_ = 0, low1_ = 0, top_ = 5.0;
	std::string unit_ = "ms", lowText_ = "1% low";
};

/// One segmented bar per logical processor with its load: pink = the game's, grey = background, amber = the
/// interrupt processors the game is kept off.
class CoreMapView {
public:
	/// lps: the processor numbers shown; load 0..1 per entry; kinds per entry. Returns true when it changed.
	bool Set(const std::vector<int>& lps, const std::vector<double>& load, const std::vector<CoreKind>& kinds);
	void Draw(ImRect r) const;

private:
	std::vector<double> load_;
	std::vector<CoreKind> kind_;
	std::vector<int> lp_;
};

/// The segmented offset slider of the graphics card page: pink from zero to the value, a square marker at the
/// value; click or drag sets it, the arrow keys move one step while it has the keyboard. Snap does the rounding.
class SegSlider {
public:
	SegSlider() = default;
	SegSlider(int min, int max, int step, int value, std::function<int(int)> snap);
	int Value() const { return value_; }
	/// Sets the value (clamped, snapped); true when it changed (ValueChanged).
	bool SetValue(int v);
	/// Draws it at the cursor (height 24 DIP, width = widthDip or the box); true when the user changed the value.
	bool Draw(const char* id, float widthDip = 0, ui::Thick margin = {});

private:
	int min_ = 0, max_ = 1, step_ = 1, value_ = 0;
	std::function<int(int)> snap_;
	int FromX(float x, float w) const;
};

/// FrameChart.cs (the check page): the last 60 s of frame times in 250 ms buckets, the p99 before the change as a
/// dashed line, the moment of the change as a pink line; grey before it, pink after.
class FrameChart {
public:
	static constexpr int Seconds = 60;
	void Set(const std::vector<double>& intervalsMs, double beforeP99, double appliedSecondsAgo, const std::string& markText, const std::string& beforeText);
	void Draw(ImRect r) const;

private:
	std::vector<double> points_;
	double before_ = 0, appliedAgo_ = -1.0;
	std::string markText_, beforeText_;
};

} // namespace hud
