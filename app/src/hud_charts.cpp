#include "hud_charts.h"

#include <algorithm>
#include <cmath>

namespace hud {

using ui::C;
using ui::Face;
using ui::S;
using ui::Weight;

double Nice(double v)
{
	if (!(v > 0) || std::isinf(v)) return 1;
	double p = std::pow(10.0, std::floor(std::log10(v)));
	double f = v / p;
	double nf = f <= 1.0000001 ? 1 : f <= 2.0000001 ? 2 : f <= 2.5000001 ? 2.5 : f <= 5.0000001 ? 5 : 10;
	return nf * p;
}

double ChartTop(double maxMs, double low1Ms, double medianMs)
{
	double want = maxMs * 1.1;
	if (medianMs > 0) want = std::min(want, medianMs * 8);
	want = std::max(want, low1Ms * 1.3);
	want = std::max(want, medianMs * 1.5);
	return Nice(std::max(want, 1.0));
}

namespace {
// HudInk
ImU32 InkLow() { return IM_COL32(244, 182, 203, 128); }
ImU32 InkTrack() { return IM_COL32(22, 23, 28, 255); }
ImU32 InkBgBar() { return IM_COL32(201, 206, 214, 150); }
ImU32 InkIrqBar() { return IM_COL32(232, 194, 122, 230); }
constexpr float InkTextSize = 9.5f;

// "0.#": at most one decimal, no trailing zero.
std::string Num1(double v)
{
	std::string s = ui::Num(v, 1);
	const std::string& sep = ui::DecimalSep();
	if (s.size() > sep.size() + 1 && s.compare(s.size() - 1, 1, "0") == 0 && s.compare(s.size() - 1 - sep.size(), sep.size(), sep) == 0)
		s.resize(s.size() - 1 - sep.size());
	return s;
}

float Snap(float v) { return std::floor(v) + 0.5f; }

void Polyline(ImDrawList* dl, const std::vector<ImVec2>& pts, ImU32 col, float thick)
{
	if (pts.size() >= 2) dl->AddPolyline(pts.data(), (int)pts.size(), col, ImDrawFlags_None, thick);
}
} // namespace

// ================================================================ HudFrameChart

void HudFrameChart::Set(const std::vector<double>& max, const std::vector<double>& endMax, double medianMs, double low1Ms, const std::string& unit, const std::string& lowText)
{
	max_ = max;
	endMax_ = endMax;
	endMax_.resize(max_.size(), 0.0);
	median_ = medianMs;
	low1_ = low1Ms;
	unit_ = unit.empty() ? "ms" : unit;
	lowText_ = lowText;
	double peak = 0;
	for (double x : max_) peak = std::max(peak, x);
	top_ = ChartTop(peak, low1Ms, medianMs);
}

void HudFrameChart::Clear()
{
	std::fill(max_.begin(), max_.end(), 0.0);
	std::fill(endMax_.begin(), endMax_.end(), 0.0);
}

void HudFrameChart::Draw(ImRect r) const
{
	const float Left = S(42), Right = S(6), Top = S(8), Bottom = S(6);
	float w = r.GetWidth(), h = r.GetHeight();
	if (w < Left + S(40) || h < S(40)) return;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float x0 = r.Min.x + Left, x1 = r.Min.x + w - Right, y0 = r.Min.y + Top, y1 = r.Min.y + h - Bottom;
	auto Y = [&](double ms) { return (float)(y1 - std::clamp(ms / top_, 0.0, 1.0) * (y1 - y0)); };
	float lw = std::max(1.0f, std::round(S(1)));
	for (int i = 0; i <= 5; i++)
	{
		float y = Snap(Y(top_ * i / 5.0));
		if (i == 0) dl->AddLine(ImVec2(x0, y), ImVec2(x1, y), C("Line"), lw);
		else ui::DashedLine(dl, ImVec2(x0, y), ImVec2(x1, y), C("Line2"), lw, 2, 3);
		std::string t = Num1(top_ * i / 5.0) + " " + unit_;
		float th = ui::FontPx(Face::Mono, InkTextSize);
		ui::TextAt(dl, ImVec2(r.Min.x, y - th / 2), Face::Mono, Weight::Normal, InkTextSize, C("Faint"), t);
	}
	int n = (int)max_.size();
	if (n < 2) return;
	float step = (x1 - x0) / (float)(n - 1);
	auto X = [&](int i) { return x0 + (float)i * step; };
	// The figures: runs of buckets with a frame.
	struct Fig { int from, to; };
	std::vector<Fig> figs;
	int last = -1;
	for (int i = 0; i < n;)
	{
		if (max_[(size_t)i] <= 0) { i++; continue; }
		int from = i;
		for (i++; i < n && max_[(size_t)i] > 0; i++) {}
		last = i - 1;
		figs.push_back({from, last});
	}
	// The area under the line: a vertical gradient over the area's bounds, (64, pink) at the top -> transparent.
	float top = y1;
	for (const Fig& f : figs)
		for (int i = f.from; i <= f.to; i++) top = std::min(top, Y(max_[(size_t)i]));
	auto GradAt = [&](float y) {
		float t = y1 > top ? std::clamp((y - top) / (y1 - top), 0.0f, 1.0f) : 1.0f;
		return IM_COL32(244, 182, 203, (int)std::lround(64.0f * (1.0f - t)));
	};
	const ImVec2 uv = dl->_Data->TexUvWhitePixel;
	for (const Fig& f : figs)
	{
		if (f.to == f.from) continue;
		int quads = f.to - f.from;
		dl->PrimReserve(quads * 6, quads * 4);
		for (int i = f.from; i < f.to; i++)
		{
			ImVec2 a(X(i), Y(max_[(size_t)i])), b(X(i + 1), Y(max_[(size_t)i + 1]));
			unsigned base = dl->_VtxCurrentIdx;
			dl->PrimWriteVtx(a, uv, GradAt(a.y));
			dl->PrimWriteVtx(b, uv, GradAt(b.y));
			dl->PrimWriteVtx(ImVec2(b.x, y1), uv, GradAt(y1));
			dl->PrimWriteVtx(ImVec2(a.x, y1), uv, GradAt(y1));
			dl->PrimWriteIdx((ImDrawIdx)base); dl->PrimWriteIdx((ImDrawIdx)(base + 1)); dl->PrimWriteIdx((ImDrawIdx)(base + 2));
			dl->PrimWriteIdx((ImDrawIdx)base); dl->PrimWriteIdx((ImDrawIdx)(base + 2)); dl->PrimWriteIdx((ImDrawIdx)(base + 3));
		}
	}
	if (low1_ > 0)
	{
		float ly = Snap(Y(low1_));
		ui::DashedLine(dl, ImVec2(x0, ly), ImVec2(x1, ly), InkLow(), lw, 5, 4);
		ImVec2 ts = ui::Measure(Face::Mono, Weight::Normal, InkTextSize, lowText_);
		ui::TextAt(dl, ImVec2(x1 - ts.x - S(4), std::max(r.Min.y, ly - ts.y - S(1))), Face::Mono, Weight::Normal, InkTextSize, C("Accent"), lowText_);
	}
	for (const Fig& f : figs)
	{
		std::vector<ImVec2> pts;
		for (int i = f.from; i <= f.to; i++) pts.emplace_back(X(i), Y(max_[(size_t)i]));
		if (f.to == f.from) pts.emplace_back(pts[0].x + S(1), pts[0].y);   // a single point: a short flat stroke
		Polyline(dl, pts, C("Accent"), S(1.3f));
	}
	if (median_ > 0)
	{
		// Rings on every hitch (at most 40, the latest kept), the value on the three longest.
		std::vector<int> hitches;
		for (int i = n - 1; i >= 0 && hitches.size() < 40; i--)
			if (IsHitch(endMax_[(size_t)i], median_)) hitches.push_back(i);
		std::stable_sort(hitches.begin(), hitches.end(), [&](int a, int b) { return endMax_[(size_t)b] < endMax_[(size_t)a]; });
		for (size_t k = 0; k < hitches.size(); k++)
		{
			int i = hitches[k];
			ImVec2 c(X(i), Y(endMax_[(size_t)i]));
			dl->AddCircle(c, S(3), C("Warn"), 24, lw);
			if (k < 3)
			{
				double v = endMax_[(size_t)i];
				std::string t = ui::Num(v, v < 100.0 ? 1 : 0);
				ImVec2 ts = ui::Measure(Face::Mono, Weight::Normal, InkTextSize, t);
				float tx = std::min(c.x + S(6), x1 - ts.x);
				ui::TextAt(dl, ImVec2(tx, std::max(r.Min.y, c.y - ts.y / 2)), Face::Mono, Weight::Normal, InkTextSize, C("Warn"), t);
			}
		}
	}
	if (last >= 0) dl->AddCircleFilled(ImVec2(X(last), Y(max_[(size_t)last])), S(3), C("Accent"), 24);
}

// ================================================================ CoreMapView

bool CoreMapView::Set(const std::vector<int>& lps, const std::vector<double>& load, const std::vector<CoreKind>& kinds)
{
	bool changed = lps.size() != lp_.size();
	for (size_t i = 0; !changed && i < lps.size(); i++)
	{
		double l = i < load.size() ? load[i] : 0.0;
		CoreKind k = i < kinds.size() ? kinds[i] : CoreKind::Background;
		changed = lps[i] != lp_[i] || k != kind_[i] || std::fabs(l - load_[i]) > 0.005;
	}
	if (!changed) return false;
	lp_ = lps;
	load_.assign(lps.size(), 0.0);
	kind_.assign(lps.size(), CoreKind::Background);
	for (size_t i = 0; i < lps.size(); i++)
	{
		if (i < load.size()) load_[i] = load[i];
		if (i < kinds.size()) kind_[i] = kinds[i];
	}
	return true;
}

void CoreMapView::Draw(ImRect r) const
{
	const float LabelH = 16, Period = 5, Seg = 3;   // DIP
	float w = r.GetWidth() / ui::Dpi(), h = r.GetHeight() / ui::Dpi();   // the WPF maths in DIP
	int n = (int)lp_.size();
	if (n == 0 || w < 20 || h < LabelH + 20) return;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float gap = n <= 16 ? 5.0f : (n <= 32 ? 3.0f : 2.0f);
	float bw = std::floor((w - gap * (n - 1)) / n);
	if (bw < 2)
	{
		bw = 2;
		gap = 0;
	}
	float total = bw * n + gap * (n - 1);
	float xs = std::floor((w - total) / 2);
	float barH = h - LabelH - 4;
	int segs = std::max(1, (int)std::floor((barH + (Period - Seg)) / Period));
	float yb = std::floor(barH);
	auto P = [&](float x, float y) { return ImVec2(std::round(r.Min.x + S(x)), std::round(r.Min.y + S(y))); };
	// Track first, then background, interrupt and game bars (the draw order of the four geometries).
	for (int pass = 0; pass < 4; pass++)
	{
		for (int i = 0; i < n; i++)
		{
			float x = xs + i * (bw + gap);
			int lit = (int)std::lround(std::clamp(load_[(size_t)i], 0.0, 1.0) * segs);
			if (load_[(size_t)i] > 0.01 && lit == 0) lit = 1;
			int kindPass = kind_[(size_t)i] == CoreKind::Game ? 3 : kind_[(size_t)i] == CoreKind::Interrupt ? 2 : 1;
			for (int s = 0; s < segs; s++)
			{
				bool on = s < lit;
				if ((on ? kindPass : 0) != pass) continue;
				float y = yb - (s + 1) * Period + (Period - Seg);
				ImU32 col = pass == 0 ? InkTrack() : pass == 1 ? InkBgBar() : pass == 2 ? InkIrqBar() : C("Accent");
				dl->AddRectFilled(P(x, y), P(x + bw, y + Seg), col);
			}
		}
	}
	int every = bw >= 16 ? 1 : (bw >= 7 ? 2 : 4);
	for (int i = 0; i < n; i += every)
	{
		char b[16];
		snprintf(b, sizeof b, "%02d", lp_[(size_t)i]);
		bool game = kind_[(size_t)i] == CoreKind::Game;
		ImVec2 ts = ui::Measure(Face::Mono, Weight::Normal, InkTextSize, b);
		float x = r.Min.x + S(xs + i * (bw + gap) + bw / 2) - ts.x / 2;
		ui::TextAt(dl, ImVec2(x, r.Min.y + S(yb + 4)), Face::Mono, Weight::Normal, InkTextSize, C(game ? "Accent" : "Dim"), b);
	}
}

// ================================================================ SegSlider

namespace {
ImGuiID g_sliderFocus = 0;
}

SegSlider::SegSlider(int min, int max, int step, int value, std::function<int(int)> snap)
	: min_(min), max_(std::max(max, min + 1)), step_(std::max(1, step)), snap_(snap ? std::move(snap) : [](int v) { return v; })
{
	value_ = snap_(std::clamp(value, min_, max_));
}

bool SegSlider::SetValue(int v)
{
	if (!snap_) snap_ = [](int x) { return x; };
	int nv = snap_(std::clamp(v, min_, max_));
	if (nv == value_) return false;
	value_ = nv;
	return true;
}

int SegSlider::FromX(float x, float w) const
{
	w = std::max(1.0f, w);
	return (int)std::lround(min_ + std::clamp(x / w, 0.0f, 1.0f) * (max_ - min_));
}

bool SegSlider::Draw(const char* id, float widthDip, ui::Thick margin)
{
	const int Segments = 48;
	const float BarH = 22;
	ImGuiID gid = ImGui::GetID(id);
	float w = widthDip > 0 ? S(widthDip) : ui::AvailW() - S(margin.l) - S(margin.r);
	ImRect r = ui::Place(ImVec2(std::max(0.0f, w), S(BarH + 2)), margin, gid);
	bool hovered = false, held = false, changed = false;
	if (ImGui::ButtonBehavior(r, gid, &hovered, &held, ImGuiButtonFlags_PressedOnClick)) g_sliderFocus = gid;
	if (hovered || held) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
	if (ImGui::IsMouseClicked(ImGuiMouseButton_Left) && !hovered && g_sliderFocus == gid) g_sliderFocus = 0;
	if (held) changed |= SetValue(FromX(ImGui::GetIO().MousePos.x - r.Min.x, r.GetWidth()));
	bool focused = g_sliderFocus == gid;
	if (focused)
	{
		if (ImGui::IsKeyPressed(ImGuiKey_LeftArrow) || ImGui::IsKeyPressed(ImGuiKey_DownArrow)) changed |= SetValue(value_ - step_);
		else if (ImGui::IsKeyPressed(ImGuiKey_RightArrow) || ImGui::IsKeyPressed(ImGuiKey_UpArrow)) changed |= SetValue(value_ + step_);
	}
	float wd = r.GetWidth() / ui::Dpi();
	if (wd < Segments * 2) return changed;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float gap = 2;
	float sw = (wd - gap * (Segments - 1)) / Segments;
	double zero = std::clamp(0, min_, max_);
	double lo = std::min<double>(zero, value_), hi = std::max<double>(zero, value_);
	double span = max_ - min_;
	for (int i = 0; i < Segments; i++)
	{
		double vFrom = min_ + span * i / Segments, vTo = min_ + span * (i + 1) / Segments;
		double mid = (vFrom + vTo) / 2.0;
		bool lit = hi > lo && mid >= lo && mid <= hi;
		float x = std::round(i * (sw + gap)), x2 = std::round(i * (sw + gap) + sw);
		dl->AddRectFilled(ImVec2(std::round(r.Min.x + S(x)), std::round(r.Min.y + S(1))), ImVec2(std::round(r.Min.x + S(x2)), std::round(r.Min.y + S(BarH + 1))), lit ? C("Accent") : InkTrack());
	}
	// The square marker at the value; outlined in pink while the slider has the keyboard.
	float vx = std::round((float)std::clamp((value_ - min_) / span, 0.0, 1.0) * (wd - 6));
	ImVec2 a(std::round(r.Min.x + S(vx)), r.Min.y), b(std::round(r.Min.x + S(vx + 6)), r.Min.y + S(BarH + 2));
	dl->AddRectFilled(a, b, C("Strong"));
	if (focused) dl->AddRect(a, b, C("Accent"), 0, 0, S(1.3f));
	return changed;
}

// ================================================================ FrameChart

void FrameChart::Set(const std::vector<double>& intervalsMs, double beforeP99, double appliedSecondsAgo, const std::string& markText, const std::string& beforeText)
{
	std::vector<double> list;
	double num = 0, num2 = 0;
	for (double m : intervalsMs)
	{
		num2 = std::max(num2, m);
		for (num += m; num >= 250.0; num -= 250.0)
		{
			list.push_back(num2);
			num2 = 0;
		}
	}
	points_ = std::move(list);
	before_ = beforeP99;
	appliedAgo_ = appliedSecondsAgo;
	markText_ = markText;
	beforeText_ = beforeText;
}

void FrameChart::Draw(ImRect r) const
{
	float w = r.GetWidth(), h = r.GetHeight();
	if (w < S(20) || h < S(20)) return;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float lw = std::max(1.0f, std::round(S(1)));
	for (int j = 0; j <= 3; j++)
	{
		float y = r.Min.y + std::round(h * j / 3.0f) + 0.5f;
		dl->AddLine(ImVec2(r.Min.x, y), ImVec2(r.Max.x, y), C("Line"), lw);
	}
	if (points_.size() < 2) return;
	double mx = 0;
	for (double p : points_) mx = std::max(mx, p);
	double top = std::max(mx, before_) * 1.15;
	if (top <= 0) return;
	const int slots = 240;
	int count = (int)points_.size();
	double wd = w;
	double num = (appliedAgo_ >= 0.0 && appliedAgo_ < 60.0) ? (wd - appliedAgo_ / 60.0 * wd) : -1.0;
	auto Y = [&](double ms) { return (float)(r.Min.y + h - std::clamp(ms / top, 0.0, 1.0) * (h - S(4)) - S(2)); };
	auto X = [&](int i) { return (float)(r.Min.x + wd - (double)(count - 1 - i) * (wd / (double)(slots - 1))); };
	if (before_ > 0)
	{
		ui::DashedLine(dl, ImVec2(r.Min.x, Y(before_)), ImVec2(r.Max.x, Y(before_)), C("Faint"), lw, 3, 4);
		ImVec2 ts = ui::Measure(Face::Ui, Weight::Normal, ui::FsSmall, beforeText_);
		ui::TextAt(dl, ImVec2(r.Max.x - ts.x, std::max(r.Min.y, Y(before_) - ts.y - S(1))), Face::Ui, Weight::Normal, ui::FsSmall, C("Dim"), beforeText_);
	}
	if (num >= 0.0)
	{
		float x = (float)(r.Min.x + num);
		dl->AddLine(ImVec2(x, r.Min.y), ImVec2(x, r.Max.y), C("AccentLine"), lw);
		ImVec2 ts = ui::Measure(Face::Ui, Weight::Normal, ui::FsSmall, markText_);
		ui::TextAt(dl, ImVec2(std::min(x + S(6), r.Max.x - ts.x), r.Min.y), Face::Ui, Weight::Normal, ui::FsSmall, C("Accent"), markText_);
	}
	int num2 = !(num < 0.0) ? std::clamp(count - 1 - (int)std::lround(appliedAgo_ * 4.0), 0, count - 1)
	                        : (!(appliedAgo_ >= 60.0) ? count - 1 : 0);
	if (appliedAgo_ < 0.0) num2 = count - 1;
	auto Trace = [&](int from, int to, const char* brush) {
		if (to - from < 1) return;
		std::vector<ImVec2> pts;
		for (int k = from; k <= to; k++) pts.emplace_back(X(k), Y(points_[(size_t)k]));
		Polyline(dl, pts, C(brush), S(1.4f));
	};
	dl->PushClipRect(r.Min, r.Max, true);
	Trace(0, num2, "Steel");
	Trace(num2, count - 1, "Accent");
	dl->PopClipRect();
}

} // namespace hud
