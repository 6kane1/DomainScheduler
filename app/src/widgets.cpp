#include "widgets.h"

#include <algorithm>
#include <cfloat>
#include <cmath>

#include "app.h"
#include "core.h"

namespace ui {

namespace {
float Px1() { return std::max(1.0f, std::round(S(1))); }

bool EqualsIgnoreCase(std::string_view a, std::string_view b)
{
	if (a.size() != b.size()) return false;
	for (size_t i = 0; i < a.size(); i++)
	{
		char x = a[i], y = b[i];
		if (x >= 'A' && x <= 'Z') x = (char)(x + 32);
		if (y >= 'A' && y <= 'Z') y = (char)(y + 32);
		if (x != y) return false;
	}
	return true;
}

std::vector<std::string> Lines(Face f, Weight w, float em, std::string_view s, float wrapW)
{
	std::vector<std::string> out;
	ImFont* font = Font(f, w);
	float px = FontPx(f, em);
	const char* b = s.data();
	const char* e = s.data() + s.size();
	if (b == e) { out.emplace_back(); return out; }
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
} // namespace

// ================================================================ CheckRow

bool CheckRow(const char* id, std::string_view label, std::string_view hint, bool& value, bool enabled)
{
	ImGuiID gid = ImGui::GetID(id);
	float op = enabled ? 1.0f : 0.5f;
	ImVec2 p = ImGui::GetCursorScreenPos();
	float right = BoxRight();
	// DockPanel: the box (16x16, margin 0,1,10,0, top) docked left; label (wrapped) and hint (Note) in the rest.
	float boxX = p.x, boxY = p.y + S(1);
	float tx = p.x + S(16) + S(10);
	float tw = std::max(1.0f, right - tx);
	auto lab = Lines(Face::Ui, Weight::Normal, FsBody, label, tw);
	std::vector<std::string> hin;
	if (!hint.empty()) hin = Lines(Face::Ui, Weight::Normal, FsNote, hint, tw);
	float lpx = FontPx(Face::Ui, FsBody), hpx = FontPx(Face::Ui, FsNote);
	float textH = lpx * (float)lab.size() + hpx * (float)hin.size();
	float h = std::max(S(1) + S(16), textH);
	ImRect r = Place(ImVec2(right - p.x, h), MRow, gid);
	(void)boxX;
	bool hovered = false, held = false, changed = false;
	if (enabled)
	{
		if (ImGui::ButtonBehavior(r, gid, &hovered, &held, ImGuiButtonFlags_PressedOnRelease))
		{
			value = !value;
			changed = true;
		}
		if (hovered) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
	}
	ImDrawList* dl = ImGui::GetWindowDrawList();
	boxY = r.Min.y + S(1);
	ImRect box(ImVec2(std::round(r.Min.x), std::round(boxY)), ImVec2(std::round(r.Min.x + S(16)), std::round(boxY + S(16))));
	// Paint(): on = Accent fill and line, off = Panel fill and a Dim line; hover over an unchecked box = Accent line.
	ImU32 fill = C(value ? "Accent" : "Panel", op);
	ImU32 line = C(value ? "Accent" : (hovered && enabled ? "Accent" : "Dim"), op);
	DrawBorder(dl, box, fill, line, 1, 2);
	if (value)
	{
		const char* mark = "\xE2\x9C\x93";   // ✓, mono FsBody*0.9 in Ink
		ImVec2 ms = Measure(Face::Mono, Weight::Normal, FsBody * 0.9f, mark);
		TextAt(dl, box.GetCenter() - ms * 0.5f, Face::Mono, Weight::Normal, FsBody * 0.9f, C("Ink", op), mark);
	}
	float y = r.Min.y + (h - textH) * 0.5f;   // the text column is VerticalAlignment.Center (Ui.Text)
	if (textH >= h) y = r.Min.y;
	for (const auto& l : lab)
	{
		TextAt(dl, ImVec2(r.Min.x + S(26), y), Face::Ui, Weight::Normal, FsBody, C("Text", op), l);
		y += lpx;
	}
	for (const auto& l : hin)
	{
		TextAt(dl, ImVec2(r.Min.x + S(26), y), Face::Ui, Weight::Normal, FsNote, C("Dim", op), l);
		y += hpx;
	}
	return changed;
}

// ================================================================ Combo

Combo::Combo(float widthDip, std::vector<Opt> opts) : width(widthDip)
{
	SetItems(std::move(opts));
}

void Combo::SetItems(std::vector<Opt> opts)
{
	opts_ = std::move(opts);
	sel_ = opts_.empty() ? -1 : 0;
}

bool Combo::Select(std::string_view key)
{
	for (size_t i = 0; i < opts_.size(); i++)
		if (EqualsIgnoreCase(opts_[i].Key, key))
		{
			sel_ = (int)i;
			return true;
		}
	return false;
}

std::string Combo::Key() const { return sel_ >= 0 && sel_ < (int)opts_.size() ? opts_[(size_t)sel_].Key : std::string(); }

bool Combo::Draw(const char* id)
{
	ImGuiID gid = ImGui::GetID(id);
	ImRect r = Place(ImVec2(std::round(S(width)), std::round(S(32))), margin, gid);
	ImGui::PushID(id);
	bool open = ImGui::IsPopupOpen("##pop");
	bool hovered = false, held = false;
	bool pressed = false;
	if (enabled) pressed = ImGui::ButtonBehavior(r, gid, &hovered, &held, ImGuiButtonFlags_PressedOnClick);   // ClickMode.Press
	float op = enabled ? 1.0f : 0.45f;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	const char* bg = hovered ? "Hover" : "Panel2";
	const char* line = open ? "Accent" : hovered ? "AccentLine" : "Line";
	DrawBorder(dl, r, C(bg, op), C(line, op), 1, 3);
	// The arrow: "M0,0 L4,4 L8,0", Accent, 1.2, right margin 10, centred.
	ImVec2 a(r.Max.x - S(10) - S(8), std::round(r.GetCenter().y - S(2)));
	ImVec2 pts[3] = {a, a + S(4, 4), a + S(8, 0)};
	dl->AddPolyline(pts, 3, C("Accent", op), ImDrawFlags_None, S(1.2f));
	if (sel_ >= 0 && sel_ < (int)opts_.size())
	{
		const std::string& t = opts_[(size_t)sel_].Label;
		float px = FontPx(Face::Ui, FsBody);
		float maxW = std::max(0.0f, r.GetWidth() - S(10) - S(26));
		TextAt(dl, ImVec2(r.Min.x + S(10), r.GetCenter().y - px * 0.5f), Face::Ui, Weight::Normal, FsBody, C("Strong", op), t, maxW);
	}
	if (tip) Tip(tip);
	if (pressed)
	{
		if (open) ImGui::CloseCurrentPopup();
		else
		{
			ImGui::OpenPopup("##pop");
			hot_ = sel_;
		}
	}
	bool changed = false;
	// The drop-down: below, 3 px down, at least as wide as the box, at most 320 high; Pop, AccentLine, radius 3, padding 3.
	ImGui::SetNextWindowPos(ImVec2(r.Min.x, r.Max.y + S(3)));
	ImGui::SetNextWindowSizeConstraints(ImVec2(r.GetWidth(), 0), ImVec2(FLT_MAX, S(320)));
	ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, S(3, 3));
	ImGui::PushStyleVar(ImGuiStyleVar_PopupRounding, S(3));
	ImGui::PushStyleVar(ImGuiStyleVar_PopupBorderSize, 1.0f);
	ImGui::PushStyleColor(ImGuiCol_PopupBg, C("Pop"));
	ImGui::PushStyleColor(ImGuiCol_Border, C("AccentLine"));
	if (ImGui::BeginPopup("##pop", ImGuiWindowFlags_NoMove | ImGuiWindowFlags_AlwaysAutoResize))
	{
		float px = FontPx(Face::Ui, FsBody);
		float itemH = px + S(12);
		float maxText = 0;
		for (const Opt& o : opts_) maxText = std::max(maxText, Measure(Face::Ui, Weight::Normal, FsBody, o.Label).x);
		float w = std::max(r.GetWidth() - S(6) - 2.0f, maxText + S(18));
		ImDrawList* pdl = ImGui::GetWindowDrawList();
		for (size_t i = 0; i < opts_.size(); i++)
		{
			ImGui::PushID((int)i);
			ImVec2 ip = ImGui::GetCursorScreenPos();
			ImRect ir(ip, ip + ImVec2(w, itemH));
			ImGuiID iid = ImGui::GetID("##it");
			ImGui::ItemSize(ir.GetSize());
			ImGui::ItemAdd(ir, iid);
			bool ih = false, ihd = false;
			if (ImGui::ButtonBehavior(ir, iid, &ih, &ihd))
			{
				if (sel_ != (int)i) changed = true;
				sel_ = (int)i;
				ImGui::CloseCurrentPopup();
			}
			if (ih) hot_ = (int)i;
			if (hot_ == (int)i) pdl->AddRectFilled(ir.Min, ir.Max, C("Sel"), S(2));
			TextAt(pdl, ip + ImVec2(S(9), S(6)), Face::Ui, Weight::Normal, FsBody, C("Text"), opts_[i].Label);
			ImGui::PopID();
		}
		ImGui::EndPopup();
	}
	ImGui::PopStyleColor(2);
	ImGui::PopStyleVar(3);
	ImGui::PopID();
	return changed;
}

// ================================================================ Leg

void LegLine(const std::vector<Leg>& legs)
{
	// WrapPanel, margin MRow; each item: a 10x10 square (margin right 5) and the text (FsNote Dim), margin right 14.
	ImVec2 p = ImGui::GetCursorScreenPos();
	float right = BoxRight();
	float x = p.x, y = p.y, lineH = 0;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float px = FontPx(Face::Ui, FsNote);
	for (const Leg& l : legs)
	{
		float tw = Measure(Face::Ui, Weight::Normal, FsNote, l.Text).x;
		float w = S(10) + S(5) + tw + S(14);
		float h = std::max(S(10), px);
		if (x > p.x && x + w - S(14) > right)
		{
			x = p.x;
			y += lineH;
			lineH = 0;
		}
		float sy = std::round(y + (h - S(10)) * 0.5f);
		ImRect sq(ImVec2(std::round(x), sy), ImVec2(std::round(x + S(10)), sy + std::round(S(10))));
		DrawBorder(dl, sq, l.Look == "fill" ? l.C : 0, l.C, 2, 0);
		TextAt(dl, ImVec2(x + S(15), y + (h - px) * 0.5f), Face::Ui, Weight::Normal, FsNote, C("Dim"), l.Text);
		x += w;
		lineH = std::max(lineH, h);
	}
	Place(ImVec2(std::max(0.0f, right - p.x), (y + lineH) - p.y), MRow);
}

// ================================================================ FactGrid

void FactGrid::Draw()
{
	if (items_.empty()) return;
	ImVec2 p = ImGui::GetCursorScreenPos();
	float right = BoxRight();
	float total = std::max(0.0f, right - p.x);
	float cw = total / (float)cols_;
	// Every cell gets the size of the largest (UniformGrid): measure each Pair in the cell's width.
	float kpx = FontPx(Face::Ui, FsNote), vpx = FontPx(Face::Ui, FsBody);
	float cellH = 0;
	for (const auto& kv : items_)
	{
		float inner = cw - S(MCell.l) - S(MCell.r);
		float kw = std::min(Measure(Face::Ui, Weight::Normal, FsNote, kv.first).x, std::max(0.0f, inner));
		float vw = std::max(1.0f, inner - kw - S(8));
		auto ls = Lines(Face::Ui, Weight::Normal, FsBody, kv.second, vw);
		float h = std::max(kpx, vpx * (float)ls.size()) + S(MCell.t) + S(MCell.b);
		cellH = std::max(cellH, h);
	}
	size_t rows = (items_.size() + (size_t)cols_ - 1) / (size_t)cols_;
	for (size_t i = 0; i < items_.size(); i++)
	{
		size_t rr = i / (size_t)cols_, cc = i % (size_t)cols_;
		float x0 = p.x + cw * (float)cc, y0 = p.y + cellH * (float)rr;
		ImGui::SetCursorScreenPos(ImVec2(x0, y0));
		Box b(x0, x0 + cw);
		Pair(items_[i].first, items_[i].second);
	}
	ImGui::SetCursorScreenPos(p);
	Place(ImVec2(total, cellH * (float)rows));
}

// ================================================================ Table

void Table::SetRows(std::vector<TableRow> rows)
{
	rows_ = std::move(rows);
	selected_.clear();
	hasSel_ = false;
}

void Table::Select(const std::string& tag, bool raise)
{
	selected_ = tag;
	hasSel_ = true;
	if (raise && SelectionChanged) SelectionChanged();
}

std::vector<float> Table::Widths(float total) const
{
	// Native-only: the name (star) column keeps room in a narrow window; the number columns shrink first.
	std::vector<float> dip;
	for (const auto& c : cols_) dip.push_back(c.second);
	return FitColumns(dip, total, 150.0f, 0.55f);
}

void Table::Draw(const char* id, ImVec2 size)
{
	ImGui::PushID(id);
	ImVec2 p = ImGui::GetCursorScreenPos();
	if (size.x <= 0) size.x = AvailW();
	if (size.y <= 0) size.y = std::max(S(40), ImGui::GetContentRegionAvail().y);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	// The caption row: padding 8,0,16,7, a line below; each cell has a right margin of 8.
	float capEm = FsSmall * 0.95f;
	float capPx = FontPx(Face::Mono, capEm);
	float headH = capPx + S(7) + Px1();
	{
		std::vector<float> w = Widths(size.x - S(8) - S(16));
		float x = p.x + S(8);
		for (size_t i = 0; i < cols_.size(); i++)
		{
			if (w[i] - S(8) >= 1)   // 0 would mean "no limit" to TextAt
				TextFit(dl, ImVec2(x, p.y), Face::Mono, Weight::Semi, capEm, C("Dim"), Spaced(cols_[i].first), w[i] - S(8), ImGui::GetID((int)(3000 + i)));
			x += w[i];
		}
		DrawEdges(dl, ImRect(p, ImVec2(p.x + size.x, p.y + headH)), C("Line"), Thick(0, 0, 0, 1));
	}
	ImGui::SetCursorScreenPos(ImVec2(p.x, p.y + headH));
	ImGui::Dummy(ImVec2(0, 0));
	BeginScroll("##rows", ImVec2(size.x, std::max(1.0f, size.y - headH)));
	{
		ImDrawList* rl = ImGui::GetWindowDrawList();
		float rowW = std::max(0.0f, BoxRight() - ImGui::GetCursorScreenPos().x);
		std::vector<float> w = Widths(rowW - S(16));
		std::string clickedTag;
		bool clicked = false;
		float minH = S(30);
		for (size_t ri = 0; ri < rows_.size(); ri++)
		{
			const TableRow& row = rows_[ri];
			float ch = 0;
			for (size_t i = 0; i < row.cells.size() && i < cols_.size(); i++)
			{
				const Cell& c = row.cells[i];
				float em = c.size > 0 ? c.size : FsBody;
				float h = c.draw ? S(c.heightDip) : FontPx(c.face, em);
				if (c.wrap && !c.draw)
					h = FontPx(c.face, em) * (float)Lines(c.face, c.weight, em, c.text, std::max(1.0f, w[i] - S(8))).size();
				ch = std::max(ch, h);
			}
			float h = std::max(minH, ch + S(12));
			ImVec2 rp = ImGui::GetCursorScreenPos();
			ImRect rr(rp, ImVec2(rp.x + rowW, rp.y + h));
			ImGui::PushID((int)ri);
			ImGuiID rid = ImGui::GetID("##row");
			ImGui::ItemSize(rr.GetSize());
			bool visible = ImGui::ItemAdd(rr, rid);
			bool hov = false, held = false;
			if (ImGui::ButtonBehavior(rr, rid, &hov, &held, ImGuiButtonFlags_PressedOnRelease))
			{
				clicked = true;
				clickedTag = row.tag;
			}
			if (visible)
			{
				bool sel = hasSel_ && selected_ == row.tag;
				if (sel) rl->AddRectFilled(rr.Min, rr.Max, C("Sel"), S(2));
				else if (hov) rl->AddRectFilled(rr.Min, rr.Max, C("Hover"), S(2));
				float x = rr.Min.x + S(8);
				for (size_t i = 0; i < row.cells.size() && i < cols_.size(); i++)
				{
					const Cell& c = row.cells[i];
					float cw = std::max(0.0f, w[i] - S(8));
					float em = c.size > 0 ? c.size : FsBody;
					if (cw < 1) {}   // a column with no room left draws nothing (TextAt's 0 means "no limit")
					else if (c.draw)
					{
						float chh = S(c.heightDip);
						ImRect cr(ImVec2(x, rr.Min.y + (h - chh) * 0.5f), ImVec2(x + cw, rr.Min.y + (h + chh) * 0.5f));
						rl->PushClipRect(ImVec2(x, rr.Min.y), ImVec2(x + cw + S(4), rr.Max.y), true);   // never over the next column
						c.draw(cr);
						rl->PopClipRect();
					}
					else if (c.wrap)
					{
						auto ls = Lines(c.face, c.weight, em, c.text, std::max(1.0f, cw));
						float lp = FontPx(c.face, em);
						float y = rr.Min.y + (h - lp * (float)ls.size()) * 0.5f;
						for (const auto& l : ls)
						{
							TextAt(rl, ImVec2(x, y), c.face, c.weight, em, C(c.brush), l, cw);
							y += lp;
						}
					}
					else
					{
						float lp = FontPx(c.face, em);
						if (c.tip.empty())
							TextFit(rl, ImVec2(x, rr.Min.y + (h - lp) * 0.5f), c.face, c.weight, em, C(c.brush), c.text, cw, ImGui::GetID((int)(2000 + i)));
						else
							TextAt(rl, ImVec2(x, rr.Min.y + (h - lp) * 0.5f), c.face, c.weight, em, C(c.brush), c.text, cw);
					}
					if (!c.tip.empty())
					{
						ImRect tr(ImVec2(x, rr.Min.y), ImVec2(x + cw, rr.Max.y));
						if (ImGui::IsMouseHoveringRect(tr.Min, tr.Max) && hov) TipRect(tr, c.tip, ImGui::GetID((int)(1000 + i)));
					}
					x += w[i];
				}
			}
			ImGui::PopID();
		}
		if (clicked) Select(clickedTag);
	}
	EndScroll();
	ImGui::SetCursorScreenPos(p);
	ImGui::Dummy(ImVec2(0, 0));
	ImGui::SetCursorScreenPos(p);
	ImGui::PopID();
	Place(size);
}

// ================================================================ LayoutTemplate

LayoutTemplate LayoutTemplate::For(const TopoView* t)
{
	if (t != nullptr && t->valid)
	{
		if (!t->hybrid)
		{
			if (!t->smt) return LayoutTemplate{"flat", false};
			return LayoutTemplate{"smt", true};
		}
		return LayoutTemplate{"hybrid", true};
	}
	return LayoutTemplate{};
}

TopoView ParseTopo(const json::Value& v)
{
	TopoView t;
	if (!v.IsObj()) return t;
	for (const auto& s : v["sets"].Arr())
	{
		LpSet x;
		x.lp = s["lp"].Int();
		x.core = s["core"].Int();
		x.llc = s["llc"].Int();
		x.group = s["group"].Int();
		t.sets.push_back(x);
	}
	for (const auto& kv : v["l3_by_llc"].Obj()) t.l3ByLlc.emplace_back(atoi(kv.first.c_str()), kv.second.U64());
	t.bigCacheLlc = v["big_cache_llc"].Int(-1);
	t.hybrid = v["hybrid"].Bool();
	t.smt = v["smt"].Bool();
	t.physical = v["physical"].Int();
	t.logical = v["logical"].Int();
	t.valid = true;
	return t;
}

} // namespace ui
