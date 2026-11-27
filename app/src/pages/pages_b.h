// Small drawing and job helpers shared by the "pages B" files (GpuPage, SettingsPage, UpdatePage, CheckPage,
// BlackBoxPage, DriversPage, ChangesPage, PowerPage). Owner: app pages B. Header only; nothing here
// decides anything: it only lays out what the core returned.
#pragma once
#include <windows.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <functional>
#include <optional>
#include <string>
#include <vector>

#include "app.h"
#include "core.h"
#include "core_bridge.h"
#include "json.h"
#include "page.h"
#include "shell.h"
#include "ui.h"
#include "widgets.h"

namespace pagesb {

/// The page caption of the C# pages: TextBlock FontSize FsBody*1.3, SemiBold, Strong.
inline void BigTitle(std::string_view text, ui::Thick margin = {})
{
	ui::TextStyle st;
	st.size = ui::FsBody * 1.3f;
	st.weight = ui::Weight::Semi;
	st.brush = "Strong";
	st.margin = margin;
	ui::TextEx(text, st);
}

/// Ui.Text(s, size, brush, bold:false, wrap:true) with a margin.
inline void Wrap(std::string_view s, float size, const char* brush, ui::Thick margin = {})
{
	ui::Text(s, size, brush, false, true, margin);
}

/// A text with FontWeight.Medium (the names of cards: Ui.Text(name, 0, "Strong") + Medium).
inline void Medium(std::string_view s, const char* brush = "Strong", bool wrap = false, ui::Thick margin = {})
{
	ui::TextStyle st;
	st.brush = brush;
	st.weight = ui::Weight::Medium;
	st.wrap = wrap;
	st.margin = margin;
	ui::TextEx(s, st);
}

/// Ui.Pill with its text in another brush (DriversPage paints the pill's text Warn when the step is bad).
inline void PillBrush(std::string_view text, bool accent, const char* textBrush, ui::Thick margin = {})
{
	std::string t = ui::Spaced(text);
	float em = ui::FsSmall * 0.95f;
	ImVec2 ts = ui::Measure(ui::Face::Mono, ui::Weight::Medium, em, t);
	float bw = std::max(1.0f, std::round(ui::S(1)));
	ImVec2 size(ts.x + ui::S(12) + 2 * bw, ts.y + ui::S(4) + 2 * bw);
	float avail = ui::AvailW() - ui::S(margin.l) - ui::S(margin.r);
	if (size.x > avail && avail > 0) size.x = avail;
	ImRect r = ui::Place(ImVec2(std::round(size.x), std::round(size.y)), margin);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ui::DrawBorder(dl, r, ui::C(accent ? "AccentSoft" : "Panel2"), ui::C(accent ? "AccentLine" : "Line"), 1, 2);
	// A trimmed pill (a long device list in a narrow window) shows the whole text on hover.
	ui::TextFit(dl, r.Min + ImVec2(bw + ui::S(6), bw + ui::S(2)), ui::Face::Mono, ui::Weight::Medium, em, ui::C(textBrush), t, r.GetWidth() - ui::S(12) - 2 * bw, ImGui::GetID("##pillfit"));
}

/// The height of a Ui.Pill in px (to centre a pill and a text in one row).
inline float PillH()
{
	float bw = std::max(1.0f, std::round(ui::S(1)));
	return std::round(ui::Measure(ui::Face::Mono, ui::Weight::Medium, ui::FsSmall * 0.95f, "X").y + ui::S(4) + 2 * bw);
}

/// The width a ui::Button takes for a label and style (ButtonFrame's rule), in px.
inline float ButtonW(std::string_view label, const ui::ButtonStyle& s)
{
	float em = s.fontSize > 0 ? s.fontSize : ui::FsBody;
	float w = ui::Measure(ui::Face::Ui, s.weight, em, label).x + ui::S(s.padding.l) + ui::S(s.padding.r);
	if (s.minWidth > 0) w = std::max(w, ui::S(s.minWidth));
	if (s.width > 0) w = ui::S(s.width);
	return std::round(w);
}

/// A Border around whatever is drawn between Begin and End (background, line, corner radius, padding, margin), the
/// way ui::BeginPanel does it but with any brushes and without the brackets.
class BoxFrame {
public:
	BoxFrame(const char* bg, const char* line, float radiusDip, ui::Thick pad, ui::Thick margin, ui::Thick edges = ui::Thick(-1))
		: bg_(bg), line_(line), radius_(radiusDip), pad_(pad), margin_(margin), edges_(edges)
	{
		ImGui::BeginGroup();
		ImVec2 p = ImGui::GetCursorScreenPos();
		x0_ = p.x + ui::S(margin.l);
		y0_ = p.y + ui::S(margin.t);
		x1_ = std::max(x0_ + ui::S(2 + pad.l + pad.r), ui::BoxRight() - ui::S(margin.r));
		ImDrawList* dl = ImGui::GetWindowDrawList();
		split_.Split(dl, 2);
		split_.SetCurrentChannel(dl, 1);
		ImGui::SetCursorScreenPos(ImVec2(x0_ + ui::S(pad.l), y0_ + ui::S(pad.t)));
		box_.emplace(x0_ + ui::S(pad.l), x1_ - ui::S(pad.r));
	}
	~BoxFrame()
	{
		box_.reset();
		ImRect c = ui::LastRect();
		float y1 = std::max(c.Max.y, y0_ + ui::S(pad_.t)) + ui::S(pad_.b);
		ImRect r(ImVec2(x0_, y0_), ImVec2(x1_, y1));
		ImDrawList* dl = ImGui::GetWindowDrawList();
		split_.SetCurrentChannel(dl, 0);
		if (edges_.l < 0)
			ui::DrawBorder(dl, r, bg_ ? ui::C(bg_) : 0, line_ ? ui::C(line_) : 0, 1, radius_);
		else
		{
			if (bg_) dl->AddRectFilled(r.Min, r.Max, ui::C(bg_));
			if (line_) ui::DrawEdges(dl, r, ui::C(line_), edges_);
		}
		split_.Merge(dl);
		ImGui::SetCursorScreenPos(r.Min);
		ImGui::Dummy(r.GetSize());
		ImGui::SetCursorScreenPos(r.Max);
		ImGui::Dummy(ImVec2(ui::S(margin_.r), ui::S(margin_.b)));
		ImGui::EndGroup();
		rect = r;
	}
	BoxFrame(const BoxFrame&) = delete;
	BoxFrame& operator=(const BoxFrame&) = delete;
	ImRect rect;

private:
	const char* bg_;
	const char* line_;
	float radius_;
	ui::Thick pad_, margin_, edges_;
	float x0_ = 0, y0_ = 0, x1_ = 0;
	ImDrawListSplitter split_;
	std::optional<ui::Box> box_;
};

/// A WrapPanel of buttons: Next(width) starts a new line when the next child does not fit after the last one;
/// After(marginRight) notes where the child just drawn ended.
class WrapRow {
public:
	explicit WrapRow(ui::Thick margin = {}) : margin_(margin)
	{
		group_.emplace(0, margin);
		row_.emplace(ui::Thick(0));
	}
	/// Call before each child with the width it takes (px, its margins included).
	void Next(float widthPx)
	{
		if (count_ > 0 && ui::BoxRight() - end_ < widthPx)
		{
			row_.reset();
			row_.emplace(ui::Thick(0));
		}
		count_++;
	}
	/// Call after each child with its right margin (px).
	void After(float marginRPx) { end_ = ui::LastRect().Max.x + marginRPx; }
	~WrapRow()
	{
		row_.reset();
		group_.reset();
	}

private:
	ui::Thick margin_;
	int count_ = 0;
	float end_ = 0;
	std::optional<ui::Group> group_;
	std::optional<ui::Row> row_;
};

/// A Button inside a WrapRow.
inline bool WrapButton(WrapRow& w, std::string_view label, const ui::ButtonStyle& s)
{
	std::string shown(label);
	size_t h = shown.find("##");
	if (h != std::string::npos) shown.resize(h);
	w.Next(ButtonW(shown, s) + ui::S(s.margin.l) + ui::S(s.margin.r));
	bool clicked = ui::Button(label, s);
	w.After(ui::S(s.margin.r));
	return clicked;
}

/// MessageBox.Show(text, title, YesNo, Warning, default No): true for Yes.
inline bool AskYesNo(const std::string& text, const std::string& title)
{
	int r = MessageBoxW(app::Hwnd(), ui::Wide(text).c_str(), ui::Wide(title).c_str(), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2);
	return r == IDYES;
}

/// A core job polled by the page itself, without the Busy overlay (C# Task.Run / Opt.Queue / Opt.Run whose result
/// the page shows). Poll() from Draw: returns true once, when it finished, with its result in `result`.
class Job {
public:
	void Start(uint64_t id)
	{
		id_ = id;
		if (id_ != 0) app::WakeIn(50);
	}
	bool Running() const { return id_ != 0; }
	bool Poll(json::Value& result)
	{
		if (id_ == 0) return false;
		char* res = nullptr;
		if (dos_job_poll(id_, &res) == 0)
		{
			app::WakeIn(50);
			return false;
		}
		id_ = 0;
		result = core::TakeJson(res);
		return true;
	}

private:
	uint64_t id_ = 0;
};

/// The page's own scroll viewer (Ui.Scroll) over its whole area.
struct Scroll {
	Scroll(const char* id, ImRect area)
	{
		ImGui::SetCursorScreenPos(area.Min);
		ui::BeginScroll(id, area.GetSize());
	}
	~Scroll() { ui::EndScroll(); }
	Scroll(const Scroll&) = delete;
	Scroll& operator=(const Scroll&) = delete;
};

/// The width of the content inside the current box, in DIP.
inline float AvailDip() { return ui::AvailW() / std::max(0.01f, ui::Dpi()); }

/// Draws runs with one baseline at `pos` (no layout), like ui::Runs.
inline ImVec2 RunsAt(ImDrawList* dl, ImVec2 pos, const std::vector<ui::Run>& runs, float size)
{
	float asc = 0, desc = 0;
	for (const ui::Run& r : runs)
	{
		float em = r.size > 0 ? r.size : size;
		ImFontBaked* b = ui::Font(r.face, r.weight)->GetFontBaked(ui::FontPx(r.face, em));
		asc = std::max(asc, b->Ascent);
		desc = std::max(desc, -b->Descent);
	}
	float x = pos.x;
	for (const ui::Run& r : runs)
	{
		if (r.text.empty()) continue;
		float em = r.size > 0 ? r.size : size;
		ImFontBaked* b = ui::Font(r.face, r.weight)->GetFontBaked(ui::FontPx(r.face, em));
		ui::TextAt(dl, ImVec2(x, pos.y + asc - b->Ascent), r.face, r.weight, em, ui::C(r.brush), r.text);
		x += ui::Measure(r.face, r.weight, em, r.text).x;
	}
	return ImVec2(x - pos.x, asc + desc);
}

/// The two runs of Ui.SetValue (value in `brush`, " unit" at max(FsSmall, size*0.42), Medium, Dim) in a face.
inline std::vector<ui::Run> ValueRuns(const std::string& value, const std::string& unit, float size, const char* brush, ui::Face face = ui::Face::Mono)
{
	std::vector<ui::Run> runs;
	ui::Run v;
	v.text = value;
	v.brush = brush;
	v.face = face;
	v.weight = ui::Weight::Semi;
	runs.push_back(v);
	if (!unit.empty())
	{
		ui::Run u;
		u.text = " " + unit;
		u.brush = "Dim";
		u.face = face;
		u.weight = ui::Weight::Medium;
		u.size = std::max(ui::FsSmall, size * 0.42f);
		runs.push_back(u);
	}
	return runs;
}

/// A DockPanel row: a button docked at the right (vertically centred), and `left` (drawn by the caller, of the
/// height leftHpx) centred in the rest. Returns true when the button was clicked.
inline bool DockRightButton(const std::function<bool(const ui::ButtonStyle&)>& button, const std::string& label, ui::ButtonStyle s, float leftHpx, const std::function<void()>& left)
{
	ImVec2 p = ImGui::GetCursorScreenPos();
	float right = ui::BoxRight();
	float bw = ButtonW(label, s);
	float bh = std::round(ui::S(s.height));
	float h = std::max(bh, leftHpx);
	ImGui::BeginGroup();
	ImGui::SetCursorScreenPos(ImVec2(right - bw, p.y + (h - bh) * 0.5f));
	s.margin = ui::Thick(0);
	bool clicked;
	{
		ui::Box b(right - bw, right);
		clicked = button(s);
	}
	ImGui::SetCursorScreenPos(ImVec2(p.x, p.y + (h - leftHpx) * 0.5f));
	{
		ui::Box b(p.x, right - bw);
		left();
	}
	ImGui::SetCursorScreenPos(p);
	ImGui::Dummy(ImVec2(right - p.x, h));
	ImGui::EndGroup();
	return clicked;
}

} // namespace pagesb
