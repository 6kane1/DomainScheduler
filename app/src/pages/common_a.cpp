// Helpers of the pages of area "app: pages A" (see common_a.h). Owner: app pages A.
#include "pages/common_a.h"

#include <windows.h>

#include <algorithm>
#include <cfloat>
#include <cmath>
#include <thread>

#include "app.h"
#include "dos_core.h"

namespace pa {

// ================================================================ quiet jobs

namespace {
struct QuietJob {
	uint64_t id;
	std::function<void(const json::Value&)> done;
};
std::vector<QuietJob> g_quiet;
} // namespace

void Quiet(uint64_t job, std::function<void(const json::Value&)> done)
{
	if (job == 0) return;
	g_quiet.push_back(QuietJob{job, std::move(done)});
	app::WakeIn(50);
}

void PollQuiet()
{
	if (g_quiet.empty()) return;
	for (size_t i = 0; i < g_quiet.size();)
	{
		char* res = nullptr;
		if (dos_job_poll(g_quiet[i].id, &res) == 0)
		{
			i++;
			continue;
		}
		json::Value v = core::TakeJson(res);
		auto done = std::move(g_quiet[i].done);
		g_quiet.erase(g_quiet.begin() + (long)i);
		try
		{
			if (done) done(v);
		}
		catch (const std::exception& ex)
		{
			core::Log(L::F("Error: {0}", {ex.what()}), core::Sev::Error);
		}
		app::Redraw();
	}
	if (!g_quiet.empty()) app::WakeIn(50);
}

// ================================================================ Ask

void Ask::Start(const std::string& text, const std::string& title)
{
	if (st_ && st_->open.load()) return;
	st_ = std::make_shared<State>();
	st_->open = true;
	std::shared_ptr<State> st = st_;
	std::wstring t = ui::Wide(text), c = ui::Wide(title);
	HWND owner = app::Hwnd();
	std::thread([st, t, c, owner]() {
		// MessageBoxResult.Cancel is the default button (MB_DEFBUTTON2).
		int r = MessageBoxW(owner, t.c_str(), c.c_str(), MB_OKCANCEL | MB_ICONQUESTION | MB_DEFBUTTON2);
		st->answer = r == IDOK ? 1 : 0;
		st->open = false;
		app::Redraw();
	}).detach();
}

int Ask::Poll()
{
	if (!st_) return -1;
	if (st_->open.load())
	{
		app::WakeIn(100);
		return -1;
	}
	int a = st_->answer.load();
	st_.reset();
	return a;
}

// ================================================================ text

std::vector<std::string> Wrap(ui::Face f, ui::Weight w, float em, std::string_view s, float wrapW)
{
	std::vector<std::string> out;
	ImFont* font = ui::Font(f, w);
	float px = ui::FontPx(f, em);
	const char* b = s.data();
	const char* e = s.data() + s.size();
	if (b == e)
	{
		out.emplace_back();
		return out;
	}
	while (b < e)
	{
		const char* nl = std::find(b, e, '\n');
		const char* le = nl;
		if (b == le) out.emplace_back();
		while (b < le)
		{
			const char* brk = wrapW > 0 ? font->CalcWordWrapPosition(px, b, le, wrapW) : le;
			if (brk == b) brk = b + 1;
			std::string part(b, brk);
			while (!part.empty() && (part.back() == ' ' || part.back() == '\r')) part.pop_back();
			out.push_back(part);
			b = brk;
			while (b < le && *b == ' ') b++;
		}
		b = nl < e ? nl + 1 : e;
	}
	return out;
}

float WrappedHeight(ui::Face f, ui::Weight w, float em, std::string_view s, float wrapW, float lineH)
{
	float px = ui::FontPx(f, em);
	float lh = lineH > 0 ? std::max(lineH, px) : px;
	return lh * (float)Wrap(f, w, em, s, wrapW).size();
}

float DrawWrapped(ImDrawList* dl, ImVec2 pos, ui::Face f, ui::Weight w, float em, ImU32 col, std::string_view s, float wrapW, float lineH, bool center)
{
	float px = ui::FontPx(f, em);
	float lh = lineH > 0 ? std::max(lineH, px) : px;
	std::vector<std::string> lines = Wrap(f, w, em, s, wrapW);
	float y = pos.y + (lh - px) * 0.5f;
	for (const auto& l : lines)
	{
		float x = pos.x;
		if (center) x += (wrapW - ui::Measure(f, w, em, l).x) * 0.5f;
		ui::TextAt(dl, ImVec2(x, y), f, w, em, col, l);
		y += lh;
	}
	return lh * (float)lines.size();
}

// ================================================================ topology

ui::TopoView Topo(const json::Value& v)
{
	ui::TopoView t;
	if (!v.IsObj()) return t;
	std::vector<int> effs;
	for (const auto& s : v["sets"].Arr())
	{
		ui::LpSet x;
		x.lp = s["lp"].Int();
		x.core = s["core"].Int();
		x.llc = s["llc"].Int();
		x.group = s["group"].Int();
		t.sets.push_back(x);
		int e = s["eff"].Int();
		if (std::find(effs.begin(), effs.end(), e) == effs.end()) effs.push_back(e);
	}
	for (const auto& e : v["l3_by_llc"].Arr()) t.l3ByLlc.emplace_back(e["llc"].Int(), e["bytes"].U64());
	t.bigCacheLlc = v["big_cache_llc"].Int(-1);
	t.hybrid = effs.size() > 1;
	t.smt = v["smt"].Bool();
	t.physical = v["physical"].Int();
	t.logical = v["logical"].Int();
	t.valid = true;
	return t;
}

// ================================================================ panels

ImRect FillPanel(ImRect r, ui::Thick pad, bool brackets)
{
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ui::DrawBorder(dl, r, ui::C("Panel"), ui::C("Line"), 1, ui::Radius);
	if (brackets)
	{
		ui::Bracket(dl, r, true);
		ui::Bracket(dl, r, false);
	}
	float bw = std::max(1.0f, std::round(ui::S(1)));
	return ImRect(r.Min + ImVec2(bw + ui::S(pad.l), bw + ui::S(pad.t)), r.Max - ImVec2(bw + ui::S(pad.r), bw + ui::S(pad.b)));
}

// ================================================================ legends

void LegendRow(const std::vector<ui::Leg>& legs, const std::string& tail, ui::Thick margin)
{
	// WrapPanel: each Leg is a 10x10 square (margin right 5) and its text (FsNote Dim), margin right 14; the tail is
	// a plain Ui.Text(FsNote, Dim).
	ImVec2 p0 = ImGui::GetCursorScreenPos();
	ImVec2 p = p0 + ImVec2(ui::S(margin.l), ui::S(margin.t));
	float right = ui::BoxRight() - ui::S(margin.r);
	float x = p.x, y = p.y, lineH = 0;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float px = ui::FontPx(ui::Face::Ui, ui::FsNote);
	auto item = [&](const ui::Leg* l, const std::string& text) {
		float tw = ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsNote, text).x;
		float w = l ? ui::S(10) + ui::S(5) + tw + ui::S(14) : tw;
		float h = l ? std::max(ui::S(10), px) : px;
		if (x > p.x && x + w - (l ? ui::S(14) : 0) > right)
		{
			x = p.x;
			y += lineH;
			lineH = 0;
		}
		float tx = x;
		if (l)
		{
			float sy = std::round(y + (h - ui::S(10)) * 0.5f);
			ImRect sq(ImVec2(std::round(x), sy), ImVec2(std::round(x + ui::S(10)), sy + std::round(ui::S(10))));
			ui::DrawBorder(dl, sq, l->Look == "fill" ? l->C : 0, l->C, 2, 0);
			tx = x + ui::S(15);
		}
		ui::TextAt(dl, ImVec2(tx, y + (h - px) * 0.5f), ui::Face::Ui, ui::Weight::Normal, ui::FsNote, ui::C("Dim"), text);
		x += w;
		lineH = std::max(lineH, h);
	};
	for (const ui::Leg& l : legs) item(&l, l.Text);
	if (!tail.empty()) item(nullptr, tail);
	ImGui::SetCursorScreenPos(p0);
	ui::Place(ImVec2(std::max(0.0f, right - p.x), (y + lineH) - p.y), margin);
}

// ================================================================ heights

float Heights::Get(const char* key, float guessPx) const
{
	for (const auto& e : h_)
		if (e.first == key) return e.second;
	return guessPx;
}

void Heights::Set(const char* key, float px)
{
	for (auto& e : h_)
	{
		if (e.first == key)
		{
			if (std::fabs(e.second - px) > 0.5f)
			{
				e.second = px;
				app::Redraw();
			}
			return;
		}
	}
	h_.emplace_back(key, px);
	app::Redraw();
}

} // namespace pa
