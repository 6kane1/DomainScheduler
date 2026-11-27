// GpuPage.cs (1.14.0 HUD look; AMD Radeon since 1.15.0 Test 1.5): the readings tiles, the overclocking panel with
// segmented sliders and step buttons, the profiles as a table, the card and its driver. Owner: app pages B.
// Same logic as C#: the feature flag, sliders that change nothing until Apply, Apply with confirmation, Reset to
// stock, profiles (save / load / apply / at start / rename / delete), the crash-guard warning, the values read back
// from the driver. Every value and decision lives in the core (get "gpu", do "gpu_*", start "gpu_read" / "gpu_apply"
// / "gpu_reset"); the page keeps only the slider widgets, the typed names and whether the rename row is open.
#include <map>

#include "hud_charts.h"
#include "pages_b.h"

namespace {

const char* const NvCleanstall = "https://www.techpowerup.com/download/techpowerup-nvcleanstall/";

class GpuPage : public Page {
public:
	std::string Title() override { return L::T("Graphics card"); }
	const char* Section() override { return "Hardware"; }

	void Build() override
	{
		data_ = json::Value();
		loaded_ = false;
		sliders_.clear();
		name_.clear();
		renameOpen_ = false;
		temp_ = "\xE2\x80\x94";   // "—"
		tempUnit_.clear();
		tempBrush_ = "Faint";
	}

	void Shown() override
	{
		uint64_t id = bridge::GpuRead();
		if (id == 0) return;
		S->Busy(L::T("Reading the hardware…"), id, [this](const json::Value& r) { Loaded(r); });
	}

	void Tick() override
	{
		if (!loaded_ || !data_["flag_open"].Bool()) return;
		int t = bridge::GpuTemp();
		if (t > 0)
		{
			temp_ = bridge::Num(t, "0");
			tempUnit_ = "\xC2\xB0" "C";
			tempBrush_ = "Accent";
		}
		else
		{
			temp_ = "\xE2\x80\x94";
			tempUnit_.clear();
			tempBrush_ = "Faint";
		}
	}

	void Draw(ImRect area) override
	{
		pagesb::Scroll sc("##gpu", area);
		if (loaded_) Tiles();
		pagesb::Wrap(L::T("The card, its driver and the official download pages. Overclocking is an advanced function behind its own feature flag."), ui::FsNote, "Dim", ui::Thick(0, 0, 0, 10));
		// Overclocking at the left; profiles and the card's driver at the right (under it when the page is narrow).
		bool wide = pagesb::AvailDip() >= 740.0f;
		if (wide)
		{
			float left = std::max(1.0f, pagesb::AvailDip() - 12.0f - 290.0f);
			ui::Row row(ui::Thick(0));
			{
				ui::Group g(left, ui::Thick(0, 0, 12, 0));
				Oc();
			}
			{
				ui::Group g(290);
				Side();
			}
		}
		else
		{
			Oc();
			Side();
		}
	}

private:
	struct Slider {
		hud::SegSlider seg;
		std::string text;   // the value as the stepper shows it
		int min = 0, max = 0, step = 1;
	};

	void Loaded(const json::Value& r)
	{
		data_ = r;
		loaded_ = r["loaded"].Bool();
		sliders_.clear();
		Sync();
		Fresh();
		Tick();   // DrawTiles ends with Tick(): the temperature at once
	}

	/// Reads the page's state again and puts the values into the sliders (SyncSliders).
	void Reload()
	{
		data_ = bridge::Gpu();
		Sync();
	}

	/// DrawOc(facts): the panel and the profiles are built again, so the typed name, the rename box and its text
	/// start over.
	void Redraw()
	{
		Reload();
		Fresh();
	}

	void Fresh()
	{
		name_.clear();
		renameOpen_ = false;
		newName_ = data_["selected"].Str();
	}

	/// The slider widgets follow the core's values (SliderRow / SyncSliders).
	void Sync()
	{
		std::string sel = data_["selected"].Str();
		if (sel != lastSel_)
		{
			lastSel_ = sel;
			renameOpen_ = false;
			newName_ = sel;
		}
		for (const json::Value& it : data_["items"].Arr())
		{
			const json::Value& sl = it["slider"];
			if (!sl.IsObj()) continue;
			std::string key = sl["key"].Str();
			int mn = sl["min"].Int(), mx = sl["max"].Int(), st = sl["step"].Int(1), v = sl["value"].Int();
			auto f = sliders_.find(key);
			if (f == sliders_.end() || f->second.min != mn || f->second.max != mx || f->second.step != st)
			{
				Slider s;
				s.min = mn;
				s.max = mx;
				s.step = st;
				s.seg = hud::SegSlider(mn, mx, st, v, nullptr);
				sliders_[key] = std::move(s);
			}
			Slider& s = sliders_[key];
			s.seg.SetValue(v);
			s.text = sl["value_text"].Str();
		}
	}

	/// A slider value goes to the core, which snaps it to the range (GpuRange.Snap) and keeps it.
	void Set(const std::string& key, int v)
	{
		json::Value r = bridge::GpuSet(key.c_str(), v);
		if (r.Has("error")) return;
		Slider& s = sliders_[key];
		s.seg.SetValue(r["value"].Int());
		s.text = r["value_text"].Str();
	}

	// ------------------------------------------------------------ tiles

	/// The readings across the top: the clocks the driver reports (stock + offset), temperature, voltage boost.
	void Tiles()
	{
		const auto& tiles = data_["tiles"].Arr();
		if (tiles.empty()) return;
		// UniformGrid, one row, margin -5,0,-5,10; each tile margin 5,0,5,0.
		int n = (int)tiles.size();
		float cell = (pagesb::AvailDip() + 10.0f) / (float)n;
		ui::Row row(ui::Thick(0, 0, 0, 10));
		for (int i = 0; i < n; i++)
		{
			const json::Value& t = tiles[(size_t)i];
			ui::Group g(std::max(1.0f, cell - 10.0f), ui::Thick(0, 0, i + 1 < n ? 10.0f : 0.0f, 0));
			ui::BeginPanel(ui::Thick(14, 11, 14, 11));
			ui::Caps(t["caption"].Str());
			if (t["temp"].Bool()) ui::Value(temp_, tempUnit_, 22, tempBrush_, ui::Thick(0, 6, 0, 2));
			else ui::Value(t["value"].Str(), t["unit"].Str(), 22, "Strong", ui::Thick(0, 6, 0, 2));
			ui::TextStyle s;
			s.size = ui::FsSmall;
			s.brush = "Faint";
			s.face = ui::Face::Mono;
			ui::TextEx(t["sub"].Str(), s);
			ui::EndPanel();
		}
	}

	// ------------------------------------------------------------ the right column

	void Side()
	{
		if (!loaded_) return;
		if (data_["oc"].Int() == 3)
		{
			ui::BeginCard(11);
			Profiles();
			ui::EndCard();
		}
		int n = 0;
		for (const json::Value& g : data_["gpus"].Arr())
		{
			ImGui::PushID(n++);
			ui::BeginCard(11);
			ui::Head(L::T("Graphics card"));
			pagesb::Medium(g["name"].Str(), "Strong", true);
			{
				ui::FactGrid f(1);
				f.Add(L::T("Driver"), g["driver"].Str());
				f.Draw();
			}
			{
				pagesb::WrapRow w(ui::Thick(0, 8, 0, 0));
				ui::ButtonStyle b = ui::BtnStyle(0);
				b.margin = ui::Thick(0, 0, 6, 6);
				if (g["nv_link"].Bool() && pagesb::WrapButton(w, L::T("NVCleanstall (official page)") + "##nv", b)) bridge::OpenLink(NvCleanstall);
				std::string maker = g["maker_url"].Str();
				if (!maker.empty() && pagesb::WrapButton(w, L::T("Drivers on the maker's site") + "##maker", b)) bridge::OpenLink(maker);
			}
			ui::EndCard();
			ImGui::PopID();
		}
		if (data_["no_gpu"].Bool())
		{
			ui::BeginCard(11);
			ui::Note(L::T("No graphics card was found."));
			ui::EndCard();
		}
	}

	// ------------------------------------------------------------ overclocking

	static void MonoWrap(const std::string& s, const char* brush, ui::Thick margin)
	{
		ui::TextStyle st;
		st.size = ui::FsSmall;
		st.brush = brush;
		st.face = ui::Face::Mono;
		st.wrap = true;
		st.margin = margin;
		ui::TextEx(s, st);
	}

	void Oc()
	{
		if (!loaded_) return;
		int oc = data_["oc"].Int();
		ui::BeginCard(11);
		ui::Head(L::T("Overclocking"));
		if (oc == 0 || oc == 1)
		{
			ui::Note(data_["oc_text"].Str());
			ui::EndCard();
			return;
		}
		if (oc == 2)
		{
			// Nothing is offered and nothing is written: the driver's own answer says why.
			pagesb::Wrap(data_["oc_text"].Str(), ui::FsNote, "Warn");
			ui::EndCard();
			return;
		}
		// The crash guard kept a profile from coming back at the start: say so until the user reacts.
		std::string skipped = data_["skipped"].Str();
		if (!skipped.empty())
		{
			pagesb::BoxFrame box("WarnSoft", "WarnLine", 3, ui::Thick(12, 9, 12, 10), ui::Thick(0, 0, 0, 10));
			pagesb::Wrap(skipped, ui::FsNote, "Warn");
			ui::ButtonStyle b = ui::BtnStyle(110);
			b.margin = ui::Thick(0, 8, 0, 0);
			if (ui::Button(L::T("Understood") + "##skipped", b))
			{
				bridge::GpuSkippedOk();
				Redraw();
			}
		}
		ui::Note(L::T("Offsets are added to the card's own clocks. Raise in small steps and test in a game after each one; artefacts, a black screen or a driver restart mean the value is too high. Moving a slider changes nothing until you press \"Apply\"."));
		if (!data_["amd_note"].Str().empty()) ui::Note(data_["amd_note"].Str());
		{
			ui::TextStyle st;
			st.size = ui::FsSmall;
			st.brush = "Silver";
			st.face = ui::Face::Mono;
			st.wrap = true;
			st.margin = ui::Thick(0, 8, 0, 0);
			ui::TextEx(data_["now_text"].Str(), st);
		}
		for (const json::Value& it : data_["items"].Arr())
		{
			if (it["slider"].IsObj()) SliderRow(it["slider"]);
			else ui::Note(it["note"].Str());
		}
		ui::Note(data_["limits"].Str(), ui::Thick(0, 12, 0, 0));
		MonoWrap(data_["risk"].Str(), "Warn", ui::Thick(0, 8, 0, 0));
		// The buttons at the right of a bar with a line on top (margin top 14, padding top 12).
		{
			pagesb::BoxFrame bar(nullptr, "Line", 0, ui::Thick(0, 12, 0, 0), ui::Thick(0, 14, 0, 0), ui::Thick(0, 1, 0, 0));
			ui::ButtonStyle reset = ui::BtnStyle(130);
			reset.height = 34;
			reset.margin = ui::Thick(0, 0, 10, 0);
			ui::ButtonStyle apply = ui::BtnStyle(130);
			ui::Primary(apply);
			apply.height = 34;
			apply.margin = ui::Thick(0);
			std::string rl = L::T("Reset to stock"), al = L::T("Apply");
			float w = pagesb::ButtonW(rl, reset) + ui::S(10) + pagesb::ButtonW(al, apply);
			ImVec2 p = ImGui::GetCursorScreenPos();
			ImGui::SetCursorScreenPos(ImVec2(std::max(p.x, ui::BoxRight() - w), p.y));
			ui::Row row(ui::Thick(0));
			if (ui::Button(rl + "##reset", reset))
			{
				uint64_t id = bridge::GpuReset();
				if (id != 0) S->Busy(L::T("Writing…"), id, [this](const json::Value& r) { Loaded(r); });
			}
			if (ui::ConfirmBtn("apply", al, apply))
			{
				uint64_t id = bridge::GpuApply("");
				if (id != 0) S->Busy(L::T("Writing…"), id, [this](const json::Value& r) { Loaded(r); });
			}
		}
		ui::Note(data_["stay"].Str(), ui::Thick(0, 10, 0, 0));
		ui::EndCard();
	}

	/// A segmented slider with − / + step buttons and the value between them; it only changes the value in the
	/// page (the core keeps it): nothing is written until "Apply".
	void SliderRow(const json::Value& sl)
	{
		std::string key = sl["key"].Str();
		auto f = sliders_.find(key);
		if (f == sliders_.end()) return;
		Slider& s = f->second;
		ImGui::PushID(key.c_str());
		ui::Group g(0, ui::Thick(0, 14, 0, 0));
		// ---- the head: name and span at the left, the stepper (34 | value | 34, height 34) at the right
		ImVec2 p = ImGui::GetCursorScreenPos();
		float right = ui::BoxRight();
		std::string unit = sl["unit"].Str();
		std::vector<ui::Run> runs = pagesb::ValueRuns(s.text, unit, 17, "Strong");
		ImVec2 vs = ui::MeasureRuns(runs, 17);
		float midW = std::round(std::max(ui::S(104), vs.x) + ui::S(16));
		float stepW = ui::S(34) * 2 + midW;
		float nameEm = ui::FsBody * 1.05f;
		float leftH = ui::FontPx(ui::Face::Ui, nameEm) + ui::S(2) + ui::FontPx(ui::Face::Mono, ui::FsSmall);
		float h = std::max(leftH, std::round(ui::S(34)));
		float sx = right - stepW, sy = p.y + (h - std::round(ui::S(34))) * 0.5f;
		ImGui::BeginGroup();
		ImGui::SetCursorScreenPos(ImVec2(sx, sy));
		{
			ui::Box b(sx, right);
			ui::Row row(ui::Thick(0));
			ui::ButtonStyle step = ui::BtnStyle(0);
			step.width = 34;
			step.height = 34;
			step.padding = ui::Thick(0);
			step.margin = ui::Thick(0);
			step.fontSize = 15;
			if (ui::Button("\xE2\x88\x92##minus", step)) Set(key, s.seg.Value() - s.step);
			ImRect mid = ui::Place(ImVec2(midW, std::round(ui::S(34))));
			ImDrawList* dl = ImGui::GetWindowDrawList();
			dl->AddRectFilled(mid.Min, mid.Max, ui::C("Panel"));
			ui::DrawEdges(dl, mid, ui::C("Line"), ui::Thick(0, 1, 0, 1));
			float th = 0;
			for (const ui::Run& r : runs) th = std::max(th, ui::FontPx(r.face, r.size > 0 ? r.size : 17));
			pagesb::RunsAt(dl, ImVec2(mid.Min.x + (mid.GetWidth() - vs.x) * 0.5f, mid.Min.y + (mid.GetHeight() - th) * 0.5f), runs, 17);
			if (ui::Button("+##plus", step)) Set(key, s.seg.Value() + s.step);
		}
		ImGui::SetCursorScreenPos(ImVec2(p.x, p.y + (h - leftH) * 0.5f));
		{
			ui::Box b(p.x, std::max(p.x + 1, sx - ui::S(10)));
			ui::TextStyle n;
			n.size = nameEm;
			n.brush = "Strong";
			ui::TextEx(sl["label"].Str(), n);
			ui::TextStyle sp;
			sp.size = ui::FsSmall;
			sp.brush = "Faint";
			sp.face = ui::Face::Mono;
			sp.margin = ui::Thick(0, 2, 0, 0);
			ui::TextEx(sl["span"].Str(), sp);
		}
		ImGui::SetCursorScreenPos(p);
		ImGui::Dummy(ImVec2(right - p.x, h));
		ImGui::EndGroup();
		// ---- the bar
		if (s.seg.Draw("##seg", 0, ui::Thick(0, 10, 0, 0))) Set(key, s.seg.Value());
		// ---- the scale under the bar: the ends, and zero where it falls inside the range
		{
			ImVec2 q = ImGui::GetCursorScreenPos() + ImVec2(0, ui::S(4));
			float w = std::max(0.0f, ui::BoxRight() - q.x);
			float em = ui::FsSmall * 0.95f;
			float fh = ui::FontPx(ui::Face::Mono, em);
			ImDrawList* dl = ImGui::GetWindowDrawList();
			ImU32 col = ui::C("Faint");
			ui::TextAt(dl, q, ui::Face::Mono, ui::Weight::Normal, em, col, sl["lo"].Str());
			std::string hi = sl["hi"].Str();
			float hw = ui::Measure(ui::Face::Mono, ui::Weight::Normal, em, hi).x;
			ui::TextAt(dl, ImVec2(q.x + w - hw, q.y), ui::Face::Mono, ui::Weight::Normal, em, col, hi);
			if (sl["zero"].Bool())
			{
				double z = std::clamp(sl["zero_at"].Num(), 0.0, 1.0);
				ui::TextAt(dl, ImVec2(q.x + (float)(w * std::max(z, 0.0001) / (std::max(z, 0.0001) + std::max(1.0 - z, 0.0001))), q.y), ui::Face::Mono, ui::Weight::Normal, em, col, "0");
			}
			ui::Place(ImVec2(w, fh), ui::Thick(0, 4, 0, 0));
		}
		ui::TextStyle note;
		note.size = ui::FsSmall;
		note.brush = "Faint";
		note.wrap = true;
		note.margin = ui::Thick(0, 3, 0, 0);
		ui::TextEx(sl["hint"].Str(), note);
		ImGui::PopID();
	}

	// ------------------------------------------------------------ profiles

	/// The columns of the profile grid: star, 50, 58, 34 (DIP), inside the padding 10 .. 8.
	struct Cols {
		float x[4];
		float w[4];
	};
	static Cols Columns(float x0, float x1)
	{
		Cols c;
		c.w[3] = ui::S(34);
		c.w[2] = ui::S(58);
		c.w[1] = ui::S(50);
		c.w[0] = std::max(1.0f, (x1 - x0) - c.w[1] - c.w[2] - c.w[3]);
		c.x[0] = x0;
		c.x[1] = x0 + c.w[0];
		c.x[2] = c.x[1] + c.w[1];
		c.x[3] = c.x[2] + c.w[2];
		return c;
	}

	/// Named profiles as a table: pick a row, then load, apply, mark for the start, rename or delete it.
	void Profiles()
	{
		ui::Head(L::T("Profiles"));
		ImDrawList* dl = ImGui::GetWindowDrawList();
		// ---- the header (Caps FsSmall*0.9 Faint), padding 10,0,8,6, a line below
		{
			ImVec2 p = ImGui::GetCursorScreenPos();
			float right = ui::BoxRight();
			Cols c = Columns(p.x + ui::S(10), right - ui::S(8));
			float em = ui::FsSmall * 0.9f;
			float fh = ui::FontPx(ui::Face::Mono, em);
			const std::string heads[4] = {L::T("Name"), L::T("Core"), L::T("Mem."), data_["last_header"].Str()};
			for (int i = 0; i < 4; i++)
			{
				std::string t = ui::Spaced(heads[i]);
				float tw = ui::Measure(ui::Face::Mono, ui::Weight::Semi, em, t).x;
				float x = i == 0 ? c.x[0] : c.x[i] + c.w[i] - tw;
				ui::TextAt(dl, ImVec2(x, p.y), ui::Face::Mono, ui::Weight::Semi, em, ui::C("Faint"), t, i == 0 ? c.w[0] : 0);
			}
			float h = fh + ui::S(6) + std::max(1.0f, std::round(ui::S(1)));
			ImRect r(p, ImVec2(right, p.y + h));
			ui::DrawEdges(dl, r, ui::C("Line"), ui::Thick(0, 0, 0, 1));
			ui::Place(r.GetSize());
		}
		const auto& profiles = data_["profiles"].Arr();
		if (profiles.empty()) ui::Note(L::T("No profiles yet."), ui::Thick(10, 10, 0, 4));
		int n = 0;
		for (const json::Value& pr : profiles)
		{
			ImGui::PushID(n++);
			ProfileRow(pr);
			ImGui::PopID();
		}
		const json::Value& actions = data_["actions"];
		if (actions.IsObj()) Actions(actions);
		else if (!profiles.empty()) ui::Note(L::T("Click a profile to load, apply, rename or delete it."), ui::Thick(0, 8, 0, 0));
		// Save the sliders: a box with the name above the button.
		{
			pagesb::BoxFrame box(nullptr, "Line", 3, ui::Thick(8), ui::Thick(0, 12, 0, 0));
			ui::Input("##newprofile", name_, std::max(1.0f, pagesb::AvailDip()), ui::Thick(0, 0, 0, 6));
			ui::ButtonStyle b = ui::BtnStyle(0);
			b.width = -1;
			b.margin = ui::Thick(0);
			if (ui::Button("+ " + L::T("Save sliders as profile") + "##save", b))
			{
				json::Value r = bridge::GpuProfileSave(name_);
				if (r["ok"].Bool()) Redraw();
			}
		}
		ui::Note(L::T("The profile marked \"at start\" is set again every time the program starts (the driver forgets overclocking at every restart). It is NOT re-applied only after Windows crashed (a blue screen or a sudden power-off) — that keeps an unstable value from coming back; normal restarts keep it."), ui::Thick(0, 10, 0, 0));
	}

	void ProfileRow(const json::Value& pr)
	{
		bool selected = pr["selected"].Bool();
		bool atStart = pr["at_start"].Bool();
		std::string name = pr["name"].Str();
		ImVec2 p = ImGui::GetCursorScreenPos();
		float right = ui::BoxRight();
		Cols c = Columns(p.x + ui::S(10), right - ui::S(8));
		// The name (FsBody Medium, wrapped) and "  ▶ at start" (FsSmall, Accent) after it.
		std::string mark = atStart ? "  \xE2\x96\xB6 " + L::T("at start") : "";
		float nameW = c.w[0];
		ImVec2 ns = ui::Measure(ui::Face::Ui, ui::Weight::Medium, ui::FsBody, name, nameW);
		float mw = atStart ? ui::Measure(ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, mark).x : 0;
		float lineH = ui::FontPx(ui::Face::Ui, ui::FsBody);
		bool markInline = atStart && ns.y <= lineH * 1.5f && ns.x + mw <= nameW;
		float textH = ns.y + (atStart && !markInline ? ui::FontPx(ui::Face::Ui, ui::FsSmall) : 0);
		float line = std::max(1.0f, std::round(ui::S(1)));
		float h = std::max(ui::S(36), ui::S(6) + textH + ui::S(6) + line);
		ImGuiID id = ImGui::GetID("##row");
		ImRect r = ui::Place(ImVec2(right - p.x, std::round(h)), {}, id);
		bool hovered = false, held = false;
		bool clicked = ImGui::ButtonBehavior(r, id, &hovered, &held, ImGuiButtonFlags_PressedOnRelease);
		if (hovered) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
		ui::Tip(pr["tip"].Str());
		ImDrawList* dl = ImGui::GetWindowDrawList();
		if (selected) dl->AddRectFilled(r.Min, r.Max, ui::C("Sel"));
		else if (hovered) dl->AddRectFilled(r.Min, r.Max, ui::C("Hover"));
		if (selected) dl->AddRectFilled(r.Min, ImVec2(r.Min.x + ui::S(2), r.Max.y), ui::C("Accent"));
		ui::DrawEdges(dl, r, ui::C("Line2"), ui::Thick(0, 0, 0, 1));
		float inTop = r.Min.y + ui::S(6), inBottom = r.Max.y - ui::S(6) - line;
		float ty = inTop + std::max(0.0f, (inBottom - inTop - textH) * 0.5f);
		ui::TextAt(dl, ImVec2(c.x[0], ty), ui::Face::Ui, ui::Weight::Medium, ui::FsBody, ui::C(selected ? "AccentText" : "Strong"), name, 0, nameW);
		if (atStart)
		{
			float sh = ui::FontPx(ui::Face::Ui, ui::FsSmall);
			ImVec2 mp = markInline ? ImVec2(c.x[0] + ns.x, ty + (lineH - sh) * 0.75f) : ImVec2(c.x[0], ty + ns.y);
			ui::TextAt(dl, mp, ui::Face::Ui, ui::Weight::Normal, ui::FsSmall, ui::C("Accent"), mark);
		}
		const std::string nums[3] = {pr["core"].Str(), pr["mem"].Str(), pr["last"].Str()};
		float nh = ui::FontPx(ui::Face::Mono, ui::FsNote);
		float ny = inTop + std::max(0.0f, (inBottom - inTop - nh) * 0.5f);
		for (int i = 0; i < 3; i++)
		{
			float tw = ui::Measure(ui::Face::Mono, ui::Weight::Normal, ui::FsNote, nums[i]).x;
			ui::TextAt(dl, ImVec2(c.x[i + 1] + c.w[i + 1] - tw, ny), ui::Face::Mono, ui::Weight::Normal, ui::FsNote, ui::C(selected ? "AccentText" : "Text"), nums[i]);
		}
		if (clicked)
		{
			bridge::GpuSelect(name);   // the core unselects a row clicked again
			Redraw();
		}
	}

	/// What can be done with the selected profile.
	void Actions(const json::Value& a)
	{
		std::string sel = data_["selected"].Str();
		{
			pagesb::WrapRow w(ui::Thick(0, 10, 0, 0));
			ui::ButtonStyle b = ui::BtnStyle(0);
			b.margin = ui::Thick(0, 0, 6, 6);
			if (pagesb::WrapButton(w, L::T("Load into sliders") + "##load", b))
			{
				bridge::GpuProfileLoad();
				Reload();
			}
			ui::ButtonStyle ap = b;
			ui::Primary(ap);
			std::string al = L::T("Apply");
			w.Next(pagesb::ButtonW(al, ap) + ui::S(6));
			bool apply = ui::ConfirmBtn("papply", al, ap);
			w.After(ui::S(6));
			if (apply)
			{
				uint64_t id = bridge::GpuApply(sel);
				if (id != 0) S->Busy(L::T("Writing…"), id, [this](const json::Value& r) { Loaded(r); });
			}
			if (pagesb::WrapButton(w, a["at_start_label"].Str() + "##atstart", b)) AtStart();
			if (pagesb::WrapButton(w, L::T("Rename…") + "##renameopen", b)) renameOpen_ = !renameOpen_;
			std::string dl = L::T("Delete");
			w.Next(pagesb::ButtonW(dl, b) + ui::S(6));
			bool del = ui::ConfirmBtn("pdelete", dl, b);
			w.After(ui::S(6));
			if (del)
			{
				bridge::GpuProfileDelete();
				Redraw();
			}
		}
		if (renameOpen_)
		{
			ui::Row row;
			ui::Input("##rename", newName_, 160);
			if (ui::Btn(L::T("Rename"), 90))
			{
				json::Value r = bridge::GpuProfileRename(newName_);
				if (r["ok"].Bool()) Redraw();
			}
		}
		std::string note = a["note"].Str();
		if (!note.empty())
		{
			if (a["note_warn"].Bool()) pagesb::Wrap(note, ui::FsNote, "Warn", ui::Thick(0, 4, 0, 0));
			else ui::Note(note, ui::Thick(0, 4, 0, 0));
		}
	}

	/// "Apply at start": when the same values (or higher) already crashed Windows the core asks first; "No" is the
	/// default (1.16.0).
	void AtStart()
	{
		json::Value r = bridge::GpuProfileAtStart(false);
		if (r["ask"].IsStr())
		{
			bool yes = pagesb::AskYesNo(r["ask"].Str(), r["title"].Str());
			r = yes ? bridge::GpuProfileAtStart(true) : bridge::GpuProfileAtStart(false, true);
			if (!yes)
			{
				Reload();   // "No": the C# handler returned before DrawOc
				return;
			}
		}
		Redraw();
	}

	json::Value data_;
	bool loaded_ = false;
	std::map<std::string, Slider> sliders_;
	std::string name_, newName_, lastSel_;
	bool renameOpen_ = false;
	std::string temp_, tempUnit_;
	const char* tempBrush_ = "Faint";
};

} // namespace

DOS_REGISTER_PAGE(PageId::Gpu, GpuPage)
