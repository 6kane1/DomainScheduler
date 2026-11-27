#include "ui.h"

#include <windows.h>

#include <algorithm>
#include <cfloat>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <map>
#include <unordered_map>

#include "app.h"
#include "core.h"

namespace ui {

// ================================================================ text helpers

std::wstring Wide(std::string_view s)
{
	if (s.empty()) return {};
	int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0);
	std::wstring w((size_t)n, L'\0');
	MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), w.data(), n);
	return w;
}

std::string Utf8(std::wstring_view s)
{
	if (s.empty()) return {};
	int n = WideCharToMultiByte(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0, nullptr, nullptr);
	std::string o((size_t)n, '\0');
	WideCharToMultiByte(CP_UTF8, 0, s.data(), (int)s.size(), o.data(), n, nullptr, nullptr);
	return o;
}

std::string Upper(std::string_view s, bool invariant)
{
	std::wstring w = Wide(s);
	if (w.empty()) return {};
	const wchar_t* loc = invariant ? LOCALE_NAME_INVARIANT : LOCALE_NAME_USER_DEFAULT;
	int n = LCMapStringEx(loc, LCMAP_UPPERCASE | (invariant ? 0 : LCMAP_LINGUISTIC_CASING), w.c_str(), (int)w.size(), nullptr, 0, nullptr, nullptr, 0);
	if (n <= 0) return std::string(s);
	std::wstring u((size_t)n, L'\0');
	LCMapStringEx(loc, LCMAP_UPPERCASE | (invariant ? 0 : LCMAP_LINGUISTIC_CASING), w.c_str(), (int)w.size(), u.data(), n, nullptr, nullptr, 0);
	return Utf8(u);
}

std::string Spaced(std::string_view s, bool invariant)
{
	if (s.empty()) return {};
	std::wstring up = Wide(Upper(s, invariant));
	std::wstring o;
	o.reserve(up.size() * 2);
	for (size_t i = 0; i < up.size(); i++)
	{
		if (i > 0) o += L' ';
		o += up[i];   // UTF-16 code unit by code unit, like the C# loop over string chars
	}
	return Utf8(o);
}

std::string Elapsed(double seconds)
{
	if (!(seconds > 0)) seconds = 0;
	long long total = (long long)std::floor(seconds);
	long long h = total / 3600;
	int m = (int)((total / 60) % 60), sec = (int)(total % 60);
	char b[64];
	snprintf(b, sizeof b, "%02lld:%02d:%02d", h, m, sec);
	return b;
}

std::string Hex(uint64_t m)
{
	char b[32];
	snprintf(b, sizeof b, "0x%llX", (unsigned long long)m);
	return b;
}

const std::string& DecimalSep()
{
	static std::string sep;
	static bool read = false;
	if (!read)
	{
		wchar_t b[8] = {};
		if (GetLocaleInfoEx(LOCALE_NAME_USER_DEFAULT, LOCALE_SDECIMAL, b, 8) > 0) sep = Utf8(b);
		if (sep.empty()) sep = ".";
		read = true;
	}
	return sep;
}

// .NET custom formats ("0", "0.0") take 15 significant digits first and round half away from zero.
std::string Num(double v, int decimals)
{
	if (std::isnan(v)) return "NaN";
	if (std::isinf(v)) return v > 0 ? "∞" : "-∞";
	bool neg = v < 0;
	double a = std::fabs(v);
	char e[64];
	snprintf(e, sizeof e, "%.14e", a);   // d.dddddddddddddde±x: 15 significant digits
	std::string digits;
	int exp10 = 0;
	{
		const char* p = e;
		for (; *p && *p != 'e'; p++)
			if (*p >= '0' && *p <= '9') digits += *p;
		if (*p == 'e') exp10 = atoi(p + 1);
	}
	// digits[0] is at 10^exp10. Keep the digits down to 10^-decimals and round half away from zero.
	int keep = exp10 + 1 + decimals;   // digits to keep
	std::string kept;
	if (keep < 0) kept = "";
	else
	{
		kept = digits.substr(0, (size_t)std::min<int>(keep, (int)digits.size()));
		while ((int)kept.size() < keep) kept += '0';
		bool up = keep < (int)digits.size() && digits[(size_t)keep] >= '5';
		if (up)
		{
			int i = (int)kept.size() - 1;
			while (i >= 0 && kept[(size_t)i] == '9') { kept[(size_t)i] = '0'; i--; }
			if (i >= 0) kept[(size_t)i]++;
			else { kept.insert(kept.begin(), '1'); exp10++; }
		}
	}
	// kept: the number * 10^decimals as an integer string.
	if (kept.empty()) kept = "0";
	while ((int)kept.size() < decimals + 1) kept.insert(kept.begin(), '0');
	std::string ip = kept.substr(0, kept.size() - (size_t)decimals);
	std::string fp = kept.substr(kept.size() - (size_t)decimals);
	size_t nz = ip.find_first_not_of('0');
	ip = nz == std::string::npos ? "0" : ip.substr(nz);
	bool zero = ip == "0" && fp.find_first_not_of('0') == std::string::npos;
	std::string o = (neg && !zero) ? "-" : "";
	o += ip;
	if (decimals > 0) o += DecimalSep() + fp;
	return o;
}

std::string Size(int64_t bytes)
{
	if (bytes < 1073741824LL)
	{
		if (bytes < 1048576LL) return Num((double)bytes / 1024.0, 0) + " KB";
		return Num((double)bytes / 1048576.0, 1) + " MB";
	}
	return Num((double)bytes / 1073741824.0, 1) + " GB";
}

// ================================================================ drawing primitives

namespace {
float Px1() { return std::max(1.0f, std::round(S(1))); }
}

ImVec2 Measure(Face f, Weight w, float em, std::string_view s, float wrapW)
{
	if (s.empty()) return ImVec2(0, FontPx(f, em));
	ImFont* font = Font(f, w);
	return font->CalcTextSizeA(FontPx(f, em), FLT_MAX, wrapW > 0 ? wrapW : 0.0f, s.data(), s.data() + s.size());
}

namespace {
// The text cut to maxW px with "…" at the end (TextTrimming.CharacterEllipsis).
std::string Trim(ImFont* font, float px, std::string_view s, float maxW)
{
	ImVec2 full = font->CalcTextSizeA(px, FLT_MAX, 0, s.data(), s.data() + s.size());
	if (full.x <= maxW + 0.5f) return std::string(s);
	const char* ell = "\xE2\x80\xA6";
	float ew = font->CalcTextSizeA(px, FLT_MAX, 0, ell, ell + 3).x;
	size_t cut = 0;
	float wsum = 0;
	size_t i = 0;
	while (i < s.size())
	{
		unsigned c = (unsigned char)s[i];
		size_t len = c < 0x80 ? 1 : (c >> 5) == 6 ? 2 : (c >> 4) == 14 ? 3 : 4;
		if (i + len > s.size()) break;
		float cw = font->CalcTextSizeA(px, FLT_MAX, 0, s.data() + i, s.data() + i + len).x;
		if (wsum + cw + ew > maxW) break;
		wsum += cw;
		i += len;
		cut = i;
	}
	return std::string(s.substr(0, cut)) + ell;
}
} // namespace

void TextAt(ImDrawList* dl, ImVec2 pos, Face f, Weight w, float em, ImU32 col, std::string_view s, float maxW, float wrapW)
{
	if (s.empty() || ((col >> IM_COL32_A_SHIFT) & 0xFF) == 0) return;
	ImFont* font = Font(f, w);
	float px = FontPx(f, em);
	pos = ImVec2(std::round(pos.x), std::round(pos.y));
	if (wrapW > 0)
	{
		dl->AddText(font, px, pos, col, s.data(), s.data() + s.size(), wrapW);
		return;
	}
	if (maxW > 0)
	{
		std::string t = Trim(font, px, s, maxW);
		dl->AddText(font, px, pos, col, t.data(), t.data() + t.size());
		return;
	}
	dl->AddText(font, px, pos, col, s.data(), s.data() + s.size());
}

void DrawBorder(ImDrawList* dl, ImRect r, ImU32 bg, ImU32 line, float thickDip, float radiusDip)
{
	float t = thickDip > 0 ? std::max(1.0f, std::round(S(thickDip))) : 0.0f;
	float rad = S(radiusDip);
	if ((bg >> IM_COL32_A_SHIFT) & 0xFF)
	{
		ImRect in(r.Min + ImVec2(t, t), r.Max - ImVec2(t, t));
		dl->AddRectFilled(in.Min, in.Max, bg, std::max(0.0f, rad - t * 0.5f));
	}
	if (t > 0 && ((line >> IM_COL32_A_SHIFT) & 0xFF))
		dl->AddRect(r.Min + ImVec2(t * 0.5f, t * 0.5f), r.Max - ImVec2(t * 0.5f, t * 0.5f), line, rad, 0, t);
}

void DrawEdges(ImDrawList* dl, ImRect r, ImU32 col, Thick t)
{
	auto th = [](float d) { return d > 0 ? std::max(1.0f, std::round(S(d))) : 0.0f; };
	float l = th(t.l), tp = th(t.t), rr = th(t.r), b = th(t.b);
	ImVec2 a = ImVec2(std::round(r.Min.x), std::round(r.Min.y)), z = ImVec2(std::round(r.Max.x), std::round(r.Max.y));
	if (tp > 0) dl->AddRectFilled(a, ImVec2(z.x, a.y + tp), col);
	if (b > 0) dl->AddRectFilled(ImVec2(a.x, z.y - b), z, col);
	if (l > 0) dl->AddRectFilled(a, ImVec2(a.x + l, z.y), col);
	if (rr > 0) dl->AddRectFilled(ImVec2(z.x - rr, a.y), z, col);
}

void DrawPath(ImDrawList* dl, ImVec2 origin, const std::vector<std::vector<ImVec2>>& figures, ImU32 col, float strokeDip, bool uniform, ImVec2 boxDip, bool closed)
{
	float sc = 1.0f;
	ImVec2 off(0, 0);
	if (uniform)
	{
		ImVec2 mn(FLT_MAX, FLT_MAX), mx(-FLT_MAX, -FLT_MAX);
		for (const auto& f : figures)
			for (ImVec2 p : f) { mn.x = std::min(mn.x, p.x); mn.y = std::min(mn.y, p.y); mx.x = std::max(mx.x, p.x); mx.y = std::max(mx.y, p.y); }
		float w = std::max(0.001f, mx.x - mn.x), h = std::max(0.001f, mx.y - mn.y);
		// Shape with Stretch.Uniform: the geometry's bounds go into the box minus the pen, centred.
		float aw = boxDip.x - strokeDip, ah = boxDip.y - strokeDip;
		sc = std::min(aw / w, ah / h);
		off = ImVec2((boxDip.x - w * sc) * 0.5f - mn.x * sc, (boxDip.y - h * sc) * 0.5f - mn.y * sc);
	}
	for (const auto& f : figures)
	{
		if (f.size() < 2) continue;
		std::vector<ImVec2> pts;
		pts.reserve(f.size());
		for (ImVec2 p : f) pts.push_back(origin + ImVec2(S(off.x + p.x * sc), S(off.y + p.y * sc)));
		dl->AddPolyline(pts.data(), (int)pts.size(), col, closed ? ImDrawFlags_Closed : ImDrawFlags_None, std::max(0.5f, S(strokeDip)));
	}
}

void DashedLine(ImDrawList* dl, ImVec2 a, ImVec2 b, ImU32 col, float thickPx, float dash, float gap)
{
	ImVec2 d = b - a;
	float len = std::sqrt(d.x * d.x + d.y * d.y);
	if (len <= 0) return;
	ImVec2 u = d / len;
	float on = dash * thickPx, off = gap * thickPx;
	if (on <= 0) { dl->AddLine(a, b, col, thickPx); return; }
	for (float t = 0; t < len; t += on + off)
	{
		float e = std::min(len, t + on);
		dl->AddLine(a + u * t, a + u * e, col, thickPx);
	}
}

void GridBackground(ImDrawList* dl, ImRect r, ImVec2 origin)
{
	// One 24x24 DIP tile: the first row and column in (11,11,14), the rest Bg.
	dl->AddRectFilled(r.Min, r.Max, C("Bg"));
	ImU32 line = Rgb(11, 11, 14);
	float step = S(24);
	float w = Px1();
	if (step < 4) return;
	float x = origin.x + std::ceil((r.Min.x - origin.x) / step) * step;
	for (; x < r.Max.x; x += step) dl->AddRectFilled(ImVec2(std::round(x), r.Min.y), ImVec2(std::round(x) + w, r.Max.y), line);
	float y = origin.y + std::ceil((r.Min.y - origin.y) / step) * step;
	for (; y < r.Max.y; y += step) dl->AddRectFilled(ImVec2(r.Min.x, std::round(y)), ImVec2(r.Max.x, std::round(y) + w), line);
}

namespace {
// A soft radial glow: an ellipse fan from `inner` at the centre to transparent at the edge (static: no redraws).
void Glow(ImDrawList* dl, ImVec2 c, float rx, float ry, ImU32 inner)
{
	ImU32 outer = inner & ~IM_COL32_A_MASK;
	const int segs = 96;
	const ImVec2 uv = dl->_Data->TexUvWhitePixel;
	dl->PrimReserve(segs * 3, segs + 2);
	unsigned base = dl->_VtxCurrentIdx;
	dl->PrimWriteVtx(c, uv, inner);
	for (int i = 0; i <= segs; i++)
	{
		float a = (float)i / segs * 6.2831853f;
		dl->PrimWriteVtx(ImVec2(c.x + std::cos(a) * rx, c.y + std::sin(a) * ry), uv, outer);
	}
	for (int i = 0; i < segs; i++)
	{
		dl->PrimWriteIdx((ImDrawIdx)base);
		dl->PrimWriteIdx((ImDrawIdx)(base + 1 + i));
		dl->PrimWriteIdx((ImDrawIdx)(base + 2 + i));
	}
}
}

void AuroraBackground(ImDrawList* dl, ImRect r)
{
	// Owner's choice 27.11.2026 ("Aurora", no grid): near-black with a pink glow at the top left, a violet one at the
	// bottom right and a faint pink one in the middle. Static, so an idle window costs nothing.
	dl->AddRectFilled(r.Min, r.Max, Rgb(9, 8, 12));
	ImVec2 sz = r.GetSize();
	dl->PushClipRect(r.Min, r.Max, true);
	for (int k = 0; k < 2; k++)       // two passes of each glow give a softer, wider falloff than one
	{
		Glow(dl, r.Min + ImVec2(0.18f * sz.x, 0.12f * sz.y), 0.55f * sz.x, 0.55f * sz.y, Rgb(244, 182, 203, 30));
		Glow(dl, r.Min + ImVec2(0.85f * sz.x, 0.90f * sz.y), 0.50f * sz.x, 0.60f * sz.y, Rgb(150, 120, 255, 24));
		Glow(dl, r.Min + ImVec2(0.70f * sz.x, 0.30f * sz.y), 0.35f * sz.x, 0.35f * sz.y, Rgb(244, 182, 203, 8));
	}
	dl->PopClipRect();
}

void Backdrop(ImDrawList* dl, ImRect r, bool pink)
{
	// RadialGradientBrush: centre and origin (0.1,0) pink / (1,1) silver, radii 0.75 of the size, colour -> transparent.
	ImVec2 sz = r.GetSize();
	ImVec2 c = r.Min + ImVec2((pink ? 0.1f : 1.0f) * sz.x, (pink ? 0.0f : 1.0f) * sz.y);
	float rx = 0.75f * sz.x, ry = 0.75f * sz.y;
	ImU32 inner = C(pink ? "HazePink" : "HazeSilver");
	ImU32 outer = inner & ~IM_COL32_A_MASK;
	dl->PushClipRect(r.Min, r.Max, true);
	const int segs = 96;
	const ImVec2 uv = dl->_Data->TexUvWhitePixel;
	dl->PrimReserve(segs * 3, segs + 2);
	unsigned base = dl->_VtxCurrentIdx;
	dl->PrimWriteVtx(c, uv, inner);
	for (int i = 0; i <= segs; i++)
	{
		float a = (float)i / segs * 6.2831853f;
		dl->PrimWriteVtx(ImVec2(c.x + std::cos(a) * rx, c.y + std::sin(a) * ry), uv, outer);
	}
	for (int i = 0; i < segs; i++)
	{
		dl->PrimWriteIdx((ImDrawIdx)base);
		dl->PrimWriteIdx((ImDrawIdx)(base + 1 + i));
		dl->PrimWriteIdx((ImDrawIdx)(base + 2 + i));
	}
	dl->PopClipRect();
}

void Bracket(ImDrawList* dl, ImRect r, bool top)
{
	// "M0.5,10 L0.5,0.5 L10,0.5" at the top-left, "M0,9.5 L9.5,9.5 L9.5,0" in the 10x10 box at the bottom-right.
	ImU32 col = C("Bracket");
	float t = Px1();
	if (top)
	{
		ImVec2 o(std::round(r.Min.x), std::round(r.Min.y));
		float len = std::round(S(10));
		dl->AddRectFilled(o, ImVec2(o.x + t, o.y + len), col);
		dl->AddRectFilled(o, ImVec2(o.x + len, o.y + t), col);
	}
	else
	{
		ImVec2 o(std::round(r.Max.x), std::round(r.Max.y));
		float len = std::round(S(10));
		dl->AddRectFilled(ImVec2(o.x - t, o.y - len), o, col);
		dl->AddRectFilled(ImVec2(o.x - len, o.y - t), o, col);
	}
}

// ================================================================ layout

namespace {
struct Frame {
	bool row = false;
	int items = 0;
	float right = -1;   // the box's right edge in px; -1 = the window's work rect
};
std::vector<Frame> g_frames;
ImRect g_last;

Frame& Top()
{
	if (g_frames.empty()) g_frames.push_back(Frame{});
	return g_frames.back();
}

void BeforeItem()
{
	Frame& f = Top();
	if (f.row && f.items > 0) ImGui::SameLine(0, 0);
}

void AfterItem() { Top().items++; }

float WindowRight() { return ImGui::GetCurrentWindow()->WorkRect.Max.x; }

struct PanelState {
	ImVec2 p;
	float x0, y0, x1;
	Thick pad, margin;
	bool brackets;
	ImDrawListSplitter split;
};
std::vector<PanelState*> g_panels;

struct GroupState {
	Thick m;
	float widthPx;
};
std::vector<GroupState> g_groups;
} // namespace

bool InRow() { return Top().row; }

float BoxRight()
{
	for (auto it = g_frames.rbegin(); it != g_frames.rend(); ++it)
	{
		if (it->right >= 0) return it->right;
		if (it->right == -2) break;   // a scroll area: its own window
	}
	return WindowRight();
}

float AvailW() { return std::max(0.0f, BoxRight() - ImGui::GetCursorScreenPos().x); }

ImRect LastRect() { return g_last; }

ImRect Place(ImVec2 size, Thick m, ImGuiID id)
{
	BeforeItem();
	ImVec2 p = ImGui::GetCursorScreenPos();
	ImRect r(p + ImVec2(S(m.l), S(m.t)), p + ImVec2(S(m.l), S(m.t)) + size);
	ImGui::ItemSize(ImVec2(S(m.l) + size.x + S(m.r), S(m.t) + size.y + S(m.b)));
	ImGui::ItemAdd(r, id);
	AfterItem();
	g_last = r;
	return r;
}

void Space(float dip)
{
	if (dip <= 0) return;
	Place(ImVec2(0, S(dip)));
}

static void OpenBlock(Thick m, float widthPx, bool row)
{
	BeforeItem();
	ImGui::BeginGroup();   // outer: the block with its margin
	ImVec2 p = ImGui::GetCursorScreenPos();
	float right = widthPx > 0 ? p.x + S(m.l) + widthPx : BoxRight() - S(m.r);
	ImGui::SetCursorScreenPos(p + ImVec2(S(m.l), S(m.t)));
	ImGui::BeginGroup();   // inner: the content
	ImGui::Dummy(ImVec2(widthPx > 0 ? widthPx : 0.0f, 0));
	if (widthPx > 0) ImGui::SetCursorScreenPos(p + ImVec2(S(m.l), S(m.t)));
	if (widthPx > 0) ImGui::Dummy(ImVec2(0, 0));
	Frame f;
	f.row = row;
	f.right = right;
	g_frames.push_back(f);
	g_groups.push_back(GroupState{m, widthPx});
}

static ImRect CloseBlock()
{
	GroupState gs = g_groups.back();
	g_groups.pop_back();
	if (g_frames.size() > 1) g_frames.pop_back();
	ImGui::EndGroup();
	ImRect r(ImGui::GetItemRectMin(), ImGui::GetItemRectMax());
	ImGui::SetCursorScreenPos(r.Max);
	ImGui::Dummy(ImVec2(S(gs.m.r), S(gs.m.b)));
	ImGui::EndGroup();
	AfterItem();
	g_last = r;
	return r;
}

Row::Row(Thick margin) : m(margin) { OpenBlock(margin, 0, true); }
Row::~Row() { CloseBlock(); }

Group::Group(float widthDip, Thick margin) : m(margin), width(widthDip)
{
	OpenBlock(margin, widthDip > 0 ? S(widthDip) : 0.0f, false);
}
Group::~Group() { rect = CloseBlock(); }

Box::Box(float x0, float x1)
{
	ImVec2 p = ImGui::GetCursorScreenPos();
	ImGui::SetCursorScreenPos(ImVec2(x0, p.y));
	ImGui::BeginGroup();
	ImGui::Dummy(ImVec2(0, 0));
	Frame f;
	f.right = x1;
	g_frames.push_back(f);
}
Box::~Box()
{
	if (g_frames.size() > 1) g_frames.pop_back();
	ImGui::EndGroup();
	g_last = ImRect(ImGui::GetItemRectMin(), ImGui::GetItemRectMax());
}

void BeginScroll(const char* id, ImVec2 size, bool horizontal)
{
	BeforeItem();
	if (size.x <= 0) size.x = AvailW();
	ImGui::BeginChild(id, size, ImGuiChildFlags_None, ImGuiWindowFlags_NoBackground | (horizontal ? ImGuiWindowFlags_HorizontalScrollbar : 0));
	Frame f;
	f.right = -2;
	g_frames.push_back(f);
}

void EndScroll()
{
	if (g_frames.size() > 1) g_frames.pop_back();
	ImGui::EndChild();
	AfterItem();
	g_last = ImRect(ImGui::GetItemRectMin(), ImGui::GetItemRectMax());
}

void BeginPanel(Thick pad, bool brackets, Thick margin)
{
	BeforeItem();
	auto* ps = new PanelState();
	ImGui::BeginGroup();
	ImVec2 p = ImGui::GetCursorScreenPos();
	ps->p = p;
	ps->x0 = p.x + S(margin.l);
	ps->y0 = p.y + S(margin.t);
	ps->x1 = std::max(ps->x0 + S(2 + pad.l + pad.r), BoxRight() - S(margin.r));
	ps->pad = pad;
	ps->margin = margin;
	ps->brackets = brackets;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ps->split.Split(dl, 2);
	ps->split.SetCurrentChannel(dl, 1);
	float bw = Px1();
	ImGui::SetCursorScreenPos(ImVec2(ps->x0 + bw + S(pad.l), ps->y0 + bw + S(pad.t)));
	ImGui::BeginGroup();
	ImGui::Dummy(ImVec2(0, 0));
	Frame f;
	f.right = ps->x1 - bw - S(pad.r);
	g_frames.push_back(f);
	g_panels.push_back(ps);
}

void EndPanel()
{
	if (g_panels.empty()) return;
	PanelState* ps = g_panels.back();
	g_panels.pop_back();
	if (g_frames.size() > 1) g_frames.pop_back();
	ImGui::EndGroup();
	float bw = Px1();
	ImRect c(ImGui::GetItemRectMin(), ImGui::GetItemRectMax());
	float y1 = std::max(c.Max.y, ps->y0 + bw + S(ps->pad.t)) + S(ps->pad.b) + bw;
	ImRect r(ImVec2(ps->x0, ps->y0), ImVec2(ps->x1, y1));
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ps->split.SetCurrentChannel(dl, 0);
	DrawBorder(dl, r, C("Panel"), C("Line"), 1, Radius);
	if (ps->brackets)
	{
		Bracket(dl, r, true);
		Bracket(dl, r, false);
	}
	ps->split.Merge(dl);
	ImGui::SetCursorScreenPos(r.Min);
	ImGui::Dummy(r.GetSize());
	ImGui::SetCursorScreenPos(r.Max);
	ImGui::Dummy(ImVec2(S(ps->margin.r), S(ps->margin.b)));
	ImGui::EndGroup();
	AfterItem();
	g_last = r;
	delete ps;
}

void BeginCard(float pad) { BeginPanel(Thick(pad + 3.0f, pad + 1.0f, pad + 3.0f, pad + 1.0f), true, Thick(0, 0, 0, 10)); }
void EndCard() { EndPanel(); }

// ================================================================ text widgets

namespace {
std::vector<std::string> WrapLines(ImFont* font, float px, std::string_view s, float wrapW)
{
	std::vector<std::string> out;
	size_t start = 0;
	while (start <= s.size())
	{
		size_t nl = s.find('\n', start);
		std::string_view line = s.substr(start, nl == std::string_view::npos ? std::string_view::npos : nl - start);
		if (!line.empty() && line.back() == '\r') line.remove_suffix(1);
		if (line.empty()) out.emplace_back();
		const char* b = line.data();
		const char* e = line.data() + line.size();
		while (b < e)
		{
			const char* brk = wrapW > 0 ? font->CalcWordWrapPosition(px, b, e, wrapW) : e;
			if (brk == b) brk = b + 1;
			std::string_view part(b, (size_t)(brk - b));
			while (!part.empty() && part.back() == ' ') part.remove_suffix(1);
			out.emplace_back(part);
			b = brk;
			while (b < e && *b == ' ') b++;
		}
		if (nl == std::string_view::npos) break;
		start = nl + 1;
	}
	return out;
}
} // namespace

void TextEx(std::string_view s, const TextStyle& st)
{
	float em = st.size > 0 ? st.size : FsBody;
	ImFont* font = Font(st.face, st.weight);
	float px = FontPx(st.face, em);
	float lh = st.lineHeight > 0 ? std::max(S(st.lineHeight), px) : px;
	BeforeItem();
	ImVec2 p = ImGui::GetCursorScreenPos();
	float avail = std::max(0.0f, BoxRight() - p.x - S(st.margin.l) - S(st.margin.r));
	float boxW = st.width > 0 ? S(st.width) : avail;
	ImU32 col = C(st.brush);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ImVec2 o = p + ImVec2(S(st.margin.l), S(st.margin.t));
	ImVec2 size;
	if (st.wrap)
	{
		std::vector<std::string> lines = WrapLines(font, px, s, boxW);
		float w = 0;
		for (const auto& l : lines) w = std::max(w, font->CalcTextSizeA(px, FLT_MAX, 0, l.c_str()).x);
		size = ImVec2(st.width > 0 ? boxW : w, lh * (float)std::max<size_t>(1, lines.size()));
		float y = o.y + (lh - px) * 0.5f;
		for (const auto& l : lines)
		{
			float lw = font->CalcTextSizeA(px, FLT_MAX, 0, l.c_str()).x;
			float x = o.x + (st.center ? (size.x - lw) * 0.5f : st.right ? size.x - lw : 0.0f);
			TextAt(dl, ImVec2(x, y), st.face, st.weight, em, col, l);
			y += lh;
		}
	}
	else
	{
		float w = font->CalcTextSizeA(px, FLT_MAX, 0, s.data(), s.data() + s.size()).x;
		float shown = std::min(w, boxW);
		size = ImVec2(st.width > 0 ? boxW : shown, lh);
		float x = o.x + (st.center ? (size.x - shown) * 0.5f : st.right ? size.x - shown : 0.0f);
		// A trimmed text (a long process name in a narrow window) shows the whole text on hover.
		TextFit(dl, ImVec2(x, o.y + (lh - px) * 0.5f), st.face, st.weight, em, col, s, boxW, ImGui::GetID(s.data(), s.data() + s.size()));
	}
	ImRect r(o, o + size);
	ImGui::ItemSize(ImVec2(S(st.margin.l) + size.x + S(st.margin.r), S(st.margin.t) + size.y + S(st.margin.b)));
	ImGui::ItemAdd(r, 0);
	AfterItem();
	g_last = r;
}

void Text(std::string_view s, float size, const char* brush, bool bold, bool wrap, Thick margin)
{
	TextStyle st;
	st.size = size;
	st.brush = brush;
	st.weight = bold ? Weight::Semi : Weight::Normal;
	st.wrap = wrap;
	st.margin = margin;
	TextEx(s, st);
}

void Note(std::string_view s, Thick margin) { Text(s, FsNote, "Dim", false, true, margin); }

void Caps(std::string_view s, float size, const char* brush, bool semi, Thick margin)
{
	TextStyle st;
	st.size = size > 0 ? size : FsSmall;
	st.brush = brush;
	st.face = Face::Mono;
	st.weight = semi ? Weight::Semi : Weight::Medium;
	st.margin = margin;
	TextEx(Spaced(s), st);
}

ImVec2 MeasureRuns(const std::vector<Run>& runs, float size)
{
	float w = 0, h = 0;
	for (const Run& r : runs)
	{
		float em = r.size > 0 ? r.size : size;
		ImVec2 m = Measure(r.face, r.weight, em, r.text);
		w += m.x;
		h = std::max(h, FontPx(r.face, em));
	}
	return ImVec2(w, h);
}

void Runs(const std::vector<Run>& runs, float size, Thick margin, float maxWDip)
{
	BeforeItem();
	ImVec2 p = ImGui::GetCursorScreenPos();
	ImVec2 o = p + ImVec2(S(margin.l), S(margin.t));
	float limit = maxWDip > 0 ? S(maxWDip) : std::max(0.0f, BoxRight() - o.x - S(margin.r));
	// One baseline: the largest ascent of the runs.
	float asc = 0, desc = 0;
	for (const Run& r : runs)
	{
		float em = r.size > 0 ? r.size : size;
		ImFontBaked* b = Font(r.face, r.weight)->GetFontBaked(FontPx(r.face, em));
		asc = std::max(asc, b->Ascent);
		desc = std::max(desc, -b->Descent);
	}
	float h = asc + desc;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float x = o.x;
	for (const Run& r : runs)
	{
		if (r.text.empty()) continue;
		float em = r.size > 0 ? r.size : size;
		ImFontBaked* b = Font(r.face, r.weight)->GetFontBaked(FontPx(r.face, em));
		float left = o.x + limit - x;
		if (left <= 0) break;
		float w = Measure(r.face, r.weight, em, r.text).x;
		TextAt(dl, ImVec2(x, o.y + asc - b->Ascent), r.face, r.weight, em, C(r.brush), r.text, left);
		x += std::min(w, left);
	}
	ImRect rr(o, ImVec2(x, o.y + h));
	ImGui::ItemSize(ImVec2(S(margin.l) + (x - o.x) + S(margin.r), S(margin.t) + h + S(margin.b)));
	ImGui::ItemAdd(rr, 0);
	AfterItem();
	g_last = rr;
}

void Head(std::string_view s, Thick margin)
{
	// "▍ " in Accent, then the spaced caption; mono FsSmall SemiBold Dim.
	std::vector<Run> runs(2);
	runs[0].text = "\xE2\x96\x8D ";
	runs[0].brush = "Accent";
	runs[0].face = Face::Mono;
	runs[0].weight = Weight::Semi;
	runs[1].text = Spaced(s);
	runs[1].brush = "Dim";
	runs[1].face = Face::Mono;
	runs[1].weight = Weight::Semi;
	Runs(runs, FsSmall, margin);
}

void Value(std::string_view value, std::string_view unit, float size, const char* brush, Thick margin)
{
	// The block's own font is the caller's (mono numbers); the unit is max(FsSmall, size*0.42), Medium, Dim.
	std::vector<Run> runs;
	Run v;
	v.text = std::string(value);
	v.brush = brush;
	v.face = Face::Mono;
	v.weight = Weight::Semi;
	runs.push_back(v);
	if (!unit.empty())
	{
		Run u;
		u.text = " " + std::string(unit);
		u.brush = "Dim";
		u.face = Face::Mono;
		u.weight = Weight::Medium;
		u.size = std::max(FsSmall, size * 0.42f);
		runs.push_back(u);
	}
	Runs(runs, size, margin);
}

void Pair(std::string_view k, std::string_view v)
{
	// DockPanel, margin MCell: the key (FsNote Dim, margin right 8) docked left, the value wrapped in the rest.
	BeforeItem();
	ImGui::BeginGroup();
	ImVec2 p = ImGui::GetCursorScreenPos();
	ImVec2 o = p + ImVec2(S(MCell.l), S(MCell.t));
	float right = BoxRight() - S(MCell.r);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ImFont* kf = Font(Face::Ui, Weight::Normal);
	float kpx = FontPx(Face::Ui, FsNote), vpx = FontPx(Face::Ui, FsBody);
	float kw = kf->CalcTextSizeA(kpx, FLT_MAX, 0, k.data(), k.data() + k.size()).x;
	float avail = std::max(0.0f, right - o.x);
	kw = std::min(kw, avail);
	float vx = o.x + kw + S(8);
	float vw = std::max(1.0f, right - vx);
	std::vector<std::string> lines = WrapLines(kf, vpx, v, vw);
	float vh = vpx * (float)std::max<size_t>(1, lines.size());
	float h = std::max(kpx, vh);
	// Both texts are VerticalAlignment.Center in the row.
	TextAt(dl, ImVec2(o.x, o.y + (h - kpx) * 0.5f), Face::Ui, Weight::Normal, FsNote, C("Dim"), k, kw);
	float y = o.y + (h - vh) * 0.5f;
	for (const auto& l : lines)
	{
		TextAt(dl, ImVec2(vx, y), Face::Ui, Weight::Normal, FsBody, C("Text"), l);
		y += vpx;
	}
	ImGui::Dummy(ImVec2(right - p.x + S(MCell.r), S(MCell.t) + h + S(MCell.b)));
	ImGui::EndGroup();
	AfterItem();
	g_last = ImRect(o, ImVec2(right, o.y + h));
}

void Kv(std::string_view k, std::string_view v)
{
	// Border: a line below, padding 0,8; the value (mono Strong) docked right, the key (Text) in the rest.
	BeforeItem();
	ImVec2 p = ImGui::GetCursorScreenPos();
	float right = BoxRight();
	float px = FontPx(Face::Ui, FsBody);
	float mpx = FontPx(Face::Mono, FsBody);
	float h = std::max(px, mpx);
	ImVec2 vs = Measure(Face::Mono, Weight::Normal, FsBody, v);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float y = p.y + S(8);
	TextAt(dl, ImVec2(right - vs.x, y + (h - mpx) * 0.5f), Face::Mono, Weight::Normal, FsBody, C("Strong"), v);
	TextAt(dl, ImVec2(p.x, y + (h - px) * 0.5f), Face::Ui, Weight::Normal, FsBody, C("Text"), k, std::max(0.0f, right - vs.x - p.x));
	float total = S(8) + h + S(8) + Px1();
	ImRect r(p, ImVec2(right, p.y + total));
	DrawEdges(dl, r, C("Line"), Thick(0, 0, 0, 1));
	ImGui::ItemSize(r.GetSize());
	ImGui::ItemAdd(r, 0);
	AfterItem();
	g_last = r;
}

// ================================================================ tooltips

namespace {
void TipWindow(std::string_view text)
{
	ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, S(9, 6));
	ImGui::PushStyleVar(ImGuiStyleVar_PopupRounding, S(3));
	ImGui::PushStyleVar(ImGuiStyleVar_PopupBorderSize, 1.0f);
	ImGui::PushStyleColor(ImGuiCol_PopupBg, C("Pop"));
	ImGui::PushStyleColor(ImGuiCol_Border, C("AccentLine"));
	ImGui::PushStyleColor(ImGuiCol_Text, C("Text"));
	if (ImGui::BeginTooltip())
	{
		PushFont(Face::Ui, Weight::Normal, FsBody);
		ImGui::TextUnformatted(text.data(), text.data() + text.size());
		PopFont();
		ImGui::EndTooltip();
	}
	ImGui::PopStyleColor(3);
	ImGui::PopStyleVar(3);
}
} // namespace

void Tip(std::string_view text)
{
	if (text.empty()) return;
	if (!ImGui::IsItemHovered(ImGuiHoveredFlags_AllowWhenDisabled)) return;
	if (ImGui::IsItemHovered(ImGuiHoveredFlags_DelayNormal | ImGuiHoveredFlags_AllowWhenDisabled))
		TipWindow(text);
	else
		app::WakeIn(450);   // rendering is on demand: come back when the delay is over
}

ImRect TextFit(ImDrawList* dl, ImVec2 pos, Face f, Weight w, float em, ImU32 col, std::string_view s, float maxW, ImGuiID id)
{
	ImVec2 sz = Measure(f, w, em, s);
	float shown = maxW > 0 ? std::min(sz.x, maxW) : sz.x;
	TextAt(dl, pos, f, w, em, col, s, maxW);
	ImRect r(pos, ImVec2(pos.x + shown, pos.y + sz.y));
	// Its own 400 ms hover delay: the text often lies on a row that already owns the hover (tables, list rows).
	static ImGuiID hotId = 0;
	static double hotSince = 0;
	bool over = maxW > 0 && sz.x > maxW + 0.5f && ImGui::IsMouseHoveringRect(r.Min, r.Max) && ImGui::IsWindowHovered(ImGuiHoveredFlags_ChildWindows | ImGuiHoveredFlags_AllowWhenBlockedByActiveItem);
	if (over)
	{
		if (hotId != id)
		{
			hotId = id;
			hotSince = ImGui::GetTime();
		}
		if (ImGui::GetTime() - hotSince >= 0.4) TipWindow(s);
		else app::WakeIn(450);
	}
	else if (hotId == id) hotId = 0;
	return r;
}

std::vector<float> FitColumns(const std::vector<float>& dip, float totalPx, float starMinDip, float minShare)
{
	std::vector<float> w(dip.size(), 0);
	float fixed = 0, fixedMin = 0;
	int stars = 0;
	for (size_t i = 0; i < dip.size(); i++)
	{
		if (dip[i] > 0)
		{
			w[i] = S(dip[i]);
			fixed += w[i];
			fixedMin += w[i] * minShare;
		}
		else stars++;
	}
	float starMin = S(starMinDip) * (float)stars;
	float rest = totalPx - fixed;
	if (stars > 0 && rest < starMin && fixed > 0)
	{
		// Shrink the fixed columns (all by the same share) so the name column keeps its minimum.
		float want = std::max(fixedMin, totalPx - starMin);
		float k = std::clamp(want / fixed, minShare, 1.0f);
		fixed = 0;
		for (size_t i = 0; i < dip.size(); i++)
			if (dip[i] > 0)
			{
				w[i] *= k;
				fixed += w[i];
			}
		rest = totalPx - fixed;
		if (rest < starMin)
		{
			// Still too narrow: the name column takes up to 40 % and the other columns share the rest (trimmed
			// with "…", the full text on hover); nothing is drawn over a neighbour.
			float star = std::max(std::max(0.0f, rest), std::min(starMin, totalPx * 0.4f));
			float k2 = fixed > 0 ? std::max(0.0f, totalPx - star) / fixed : 0;
			for (size_t i = 0; i < dip.size(); i++)
				if (dip[i] > 0) w[i] *= k2;
			rest = star;
		}
	}
	rest = std::max(0.0f, rest);
	for (size_t i = 0; i < dip.size(); i++)
		if (dip[i] <= 0) w[i] = stars > 0 ? rest / (float)stars : 0;
	return w;
}

void TipRect(ImRect r, std::string_view text, ImGuiID id)
{
	if (text.empty()) return;
	ImGui::ItemAdd(r, id, nullptr, ImGuiItemFlags_NoNav);
	Tip(text);
}

// ================================================================ buttons

ButtonStyle BtnStyle(float minWidth)
{
	ButtonStyle s;
	s.minWidth = minWidth;
	return s;
}

ButtonStyle& Primary(ButtonStyle& s, bool on)
{
	s.bg = on ? "Accent" : "Panel2";
	s.fg = on ? "Ink" : "Text";
	s.line = on ? "Accent" : "Line";
	if (on) s.weight = Weight::Semi;
	return s;
}

namespace {
struct ButtonResult {
	bool clicked;
	bool hovered;
	ImRect inner;
};

ButtonResult ButtonFrame(ImGuiID id, ImVec2 contentPx, const ButtonStyle& s)
{
	float w = contentPx.x + S(s.padding.l) + S(s.padding.r);
	if (s.minWidth > 0) w = std::max(w, S(s.minWidth));
	if (s.width > 0) w = S(s.width);
	if (s.width < 0) w = std::max(w, AvailW() - S(s.margin.l) - S(s.margin.r));
	float h = s.height > 0 ? S(s.height) : contentPx.y + S(s.padding.t) + S(s.padding.b);
	ImRect r = Place(ImVec2(std::round(w), std::round(h)), s.margin, id);
	bool hovered = false, held = false, clicked = false;
	if (s.enabled)
		clicked = ImGui::ButtonBehavior(r, id, &hovered, &held);
	float op = !s.enabled ? 0.45f : (held && hovered) ? 0.8f : 1.0f;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	ImGuiContext& g = *ImGui::GetCurrentContext();
	bool focused = s.enabled && g.NavId == id && g.NavCursorVisible;
	ImU32 line = C(hovered ? "AccentLine" : s.line);
	if (focused) line = C("Accent");
	// "Soft" buttons (owner's choice 27.11.2026): 8 DIP corners, a translucent fill and line; hover tints pink.
	bool plain = std::string_view(s.bg) == "Panel2";
	bool primary = std::string_view(s.bg) == "Accent";
	ImU32 fill = plain ? Rgb(255, 255, 255, 14) : C(s.bg);
	if (plain && !focused) line = hovered ? Rgb(244, 182, 203, 90) : Rgb(255, 255, 255, 23);
	if (primary && hovered)
	{
		fill = Rgb(247, 198, 214);
		for (int k = 3; k >= 1; k--)      // a soft pink shadow under the primary button
			dl->AddRectFilled(r.Min + ImVec2(-S(k * 2.0f), S(4.0f)), r.Max + ImVec2(S(k * 2.0f), S(4.0f + k * 3.0f)), Rgb(244, 182, 203, 10), S(8 + k * 2.0f));
	}
	DrawBorder(dl, r, Fade(fill, op), Fade(line, op), 1, 8);
	if (hovered && plain) dl->AddRectFilled(r.Min, r.Max, Fade(Rgb(244, 182, 203, 26), op), S(8));
	ImRect inner(r.Min + ImVec2(S(s.padding.l), S(s.padding.t)), r.Max - ImVec2(S(s.padding.r), S(s.padding.b)));
	if (s.tip && *s.tip) Tip(s.tip);
	return {clicked, hovered, inner};
}
} // namespace

bool Button(std::string_view label, const ButtonStyle& s)
{
	std::string full(label);
	size_t hash = full.find("##");
	std::string shown = hash == std::string::npos ? full : full.substr(0, hash);
	ImGuiID id = ImGui::GetID(full.c_str());
	float em = s.fontSize > 0 ? s.fontSize : FsBody;
	ImVec2 ts = Measure(Face::Ui, s.weight, em, shown);
	ButtonResult br = ButtonFrame(id, ts, s);
	float op = !s.enabled ? 0.45f : 1.0f;
	if (ImGui::IsItemActive() && br.hovered) op = 0.8f;
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float avail = std::max(0.0f, br.inner.GetWidth());
	float tw = std::min(ts.x, avail);
	ImVec2 pos(br.inner.Min.x + (br.inner.GetWidth() - tw) * 0.5f, br.inner.Min.y + (br.inner.GetHeight() - ts.y) * 0.5f);
	TextAt(dl, pos, Face::Ui, s.weight, em, Fade(C(s.fg), op), shown, avail);
	return br.clicked;
}

bool ButtonCustom(const char* id, ImVec2 contentDip, const ButtonStyle& s, const std::function<void(ImRect, bool)>& content)
{
	ButtonResult br = ButtonFrame(ImGui::GetID(id), S(contentDip.x, contentDip.y), s);
	if (content) content(br.inner, br.hovered);
	return br.clicked;
}

bool Btn(std::string_view label, float minWidth) { return Button(label, BtnStyle(minWidth)); }

namespace {
struct Confirm {
	bool armed = false;
	double since = 0;
	int lastFrame = -10;
};
std::unordered_map<ImGuiID, Confirm> g_confirm;
}

bool ConfirmBtn(const char* id, std::string_view label, ButtonStyle s)
{
	ImGuiID key = ImGui::GetID(id);
	Confirm& c = g_confirm[key];
	int frame = ImGui::GetFrameCount();
	// A button that was not on screen in the last frame is a new one (WPF builds the page again; Unloaded stops
	// the timer).
	if (c.lastFrame < frame - 1) c.armed = false;
	c.lastFrame = frame;
	double now = app::Now();
	if (c.armed && now - c.since >= 4.0) c.armed = false;   // the 4 s DispatcherTimer: Disarm
	ButtonStyle look = s;
	std::string text = std::string(label);
	if (c.armed)
	{
		text = L::T("Click again to confirm");
		look.fg = "Warn";
		look.bg = "WarnSoft";
		look.line = "Warn";
	}
	text += "##";
	text += id;
	bool clicked = Button(text, look);
	if (!clicked)
	{
		if (c.armed) app::WakeIn((int)std::ceil((4.0 - (now - c.since)) * 1000.0) + 10);
		return false;
	}
	if (!c.armed)
	{
		c.armed = true;
		c.since = now;
		app::WakeIn(4010);
		return false;
	}
	c.armed = false;
	return true;
}

// ================================================================ small widgets

void Pill(std::string_view text, bool accent, Thick margin)
{
	std::string t = Spaced(text);
	float em = FsSmall * 0.95f;
	ImVec2 ts = Measure(Face::Mono, Weight::Medium, em, t);
	float bw = Px1();
	ImVec2 size(ts.x + S(12) + 2 * bw, ts.y + S(4) + 2 * bw);
	float avail = AvailW() - S(margin.l) - S(margin.r);
	if (size.x > avail && avail > 0) size.x = avail;
	ImRect r = Place(ImVec2(std::round(size.x), std::round(size.y)), margin);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	DrawBorder(dl, r, C(accent ? "AccentSoft" : "Panel2"), C(accent ? "AccentLine" : "Line"), 1, 2);
	TextAt(dl, r.Min + ImVec2(bw + S(6), bw + S(2)), Face::Mono, Weight::Medium, em, C(accent ? "AccentText" : "Text"), t, r.GetWidth() - S(12) - 2 * bw);
}

void Bar(float share, bool accent, float heightDip, Thick margin, float widthDip)
{
	share = std::clamp(share, 0.0f, 1.0f);
	float w = widthDip > 0 ? S(widthDip) : AvailW() - S(margin.l) - S(margin.r);
	float h = S(heightDip);
	ImRect r = Place(ImVec2(std::max(0.0f, w), h), margin);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	dl->AddRectFilled(r.Min, r.Max, C("Line"));
	if (share > 0)
	{
		float fw = std::max(h, r.GetWidth() * share);   // MinWidth = height when share > 0
		dl->AddRectFilled(r.Min, ImVec2(std::min(r.Max.x, r.Min.x + fw), r.Max.y), C(accent ? "Accent" : "Steel"));
	}
}

namespace {
int ResizeCb(ImGuiInputTextCallbackData* d)
{
	if (d->EventFlag == ImGuiInputTextFlags_CallbackResize)
	{
		auto* s = (std::string*)d->UserData;
		s->resize((size_t)d->BufTextLen);
		d->Buf = s->data();
	}
	return 0;
}
}

bool Input(const char* id, std::string& text, float widthDip, Thick margin, int maxLen)
{
	BeforeItem();
	ImGui::BeginGroup();
	ImVec2 p = ImGui::GetCursorScreenPos();
	ImGui::SetCursorScreenPos(p + ImVec2(S(margin.l), S(margin.t)));
	float h = S(30);
	float px = FontPx(Face::Ui, FsBody);
	PushFont(Face::Ui, Weight::Normal, FsBody);
	ImGui::PushStyleVar(ImGuiStyleVar_FramePadding, ImVec2(S(6) + Px1(), std::max(0.0f, (h - px) * 0.5f)));
	ImGui::PushStyleVar(ImGuiStyleVar_FrameRounding, S(3));
	ImGui::PushStyleColor(ImGuiCol_FrameBg, C("Panel2"));
	ImGui::PushStyleColor(ImGuiCol_FrameBgHovered, C("Panel2"));
	ImGui::PushStyleColor(ImGuiCol_FrameBgActive, C("Panel2"));
	ImGui::PushStyleColor(ImGuiCol_Text, C("Strong"));
	ImGui::SetNextItemWidth(S(widthDip));
	if (text.capacity() < 16) text.reserve(16);
	bool changed = ImGui::InputText(id, text.data(), text.capacity() + 1, ImGuiInputTextFlags_CallbackResize, ResizeCb, &text);
	if (changed && maxLen > 0 && (int)text.size() > maxLen) text.resize((size_t)maxLen);
	ImRect r(ImGui::GetItemRectMin(), ImGui::GetItemRectMax());
	bool hovered = ImGui::IsItemHovered();
	bool active = ImGui::IsItemActive();
	bool disabled = (ImGui::GetItemFlags() & ImGuiItemFlags_Disabled) != 0;
	ImGui::PopStyleColor(4);
	ImGui::PopStyleVar(2);
	PopFont();
	ImU32 line = C(active ? "Accent" : hovered ? "AccentLine" : "Line");
	if (disabled) line = Fade(line, 0.45f);
	DrawBorder(ImGui::GetWindowDrawList(), r, 0, line, 1, 3);
	ImGui::SetCursorScreenPos(r.Max);
	ImGui::Dummy(ImVec2(S(margin.r), S(margin.b)));
	ImGui::EndGroup();
	AfterItem();
	g_last = r;
	return changed;
}

bool Slider(const char* id, double& value, double min, double max, float widthDip, double step)
{
	ImGuiID gid = ImGui::GetID(id);
	float w = widthDip > 0 ? S(widthDip) : AvailW();
	ImRect r = Place(ImVec2(w, S(22)), {}, gid);
	bool hovered = false, held = false;
	ImGui::ButtonBehavior(r, gid, &hovered, &held, ImGuiButtonFlags_PressedOnClick);
	bool changed = false;
	float thumb = S(12);
	if (held && max > min)
	{
		float t = std::clamp((ImGui::GetIO().MousePos.x - r.Min.x - thumb * 0.5f) / std::max(1.0f, r.GetWidth() - thumb), 0.0f, 1.0f);
		double v = min + t * (max - min);
		if (step > 0) v = min + std::round((v - min) / step) * step;
		v = std::clamp(v, min, max);
		if (v != value) { value = v; changed = true; }
	}
	ImDrawList* dl = ImGui::GetWindowDrawList();
	float t = max > min ? (float)std::clamp((value - min) / (max - min), 0.0, 1.0) : 0.0f;
	float cy = std::round(r.GetCenter().y);
	float tx = r.Min.x + t * (r.GetWidth() - thumb);
	float th = std::max(1.0f, std::round(S(2)));
	dl->AddRectFilled(ImVec2(r.Min.x, cy - th * 0.5f), ImVec2(tx + thumb * 0.5f, cy + th * 0.5f), C("Accent"));
	dl->AddRectFilled(ImVec2(tx + thumb * 0.5f, cy - th * 0.5f), ImVec2(r.Max.x, cy + th * 0.5f), C("Line"));
	ImRect tr(ImVec2(std::round(tx), std::round(cy - thumb * 0.5f)), ImVec2(std::round(tx + thumb), std::round(cy + thumb * 0.5f)));
	dl->AddRectFilled(tr.Min, tr.Max, C((hovered || held) ? "Accent" : "Bg"));
	float bt = S(1.5f);
	dl->AddRect(tr.Min + ImVec2(bt * 0.5f, bt * 0.5f), tr.Max - ImVec2(bt * 0.5f, bt * 0.5f), C("Accent"), 0, 0, bt);
	return changed;
}

void Separator(Thick margin)
{
	float w = AvailW() - S(margin.l) - S(margin.r);
	ImRect r = Place(ImVec2(std::max(0.0f, w), Px1()), margin);
	ImGui::GetWindowDrawList()->AddRectFilled(r.Min, r.Max, C("Line"));
}

// ================================================================ Ui.LpMap

int LpMap(const char* id, const TopoView& t, const std::function<LpLook(const LpSet&)>& look, bool clickable, float cell)
{
	int clicked = -1;
	ImGui::PushID(id);
	bool stacked = t.valid && (t.hybrid || t.smt);   // LayoutTemplate.For: hybrid / smt are Stacked, flat is not
	std::vector<int> llcs;
	for (const LpSet& s : t.sets)
		if (s.group == 0 && std::find(llcs.begin(), llcs.end(), s.llc) == llcs.end()) llcs.push_back(s.llc);
	std::sort(llcs.begin(), llcs.end());
	Group col;
	for (int llc : llcs)
	{
		uint64_t l3 = 0;
		for (const auto& kv : t.l3ByLlc)
			if (kv.first == llc) l3 = kv.second;
		std::string cap = L::F("L3 domain {0}", {std::to_string(llc)});
		if (l3 != 0) cap += " \xC2\xB7 " + std::to_string(l3 >> 20) + " MB";
		if (t.bigCacheLlc == llc) cap += " \xC2\xB7 " + L::T("big cache");
		Text(cap, FsSmall, "Dim");
		std::vector<int> cores;
		for (const LpSet& s : t.sets)
			if (s.group == 0 && s.llc == llc && std::find(cores.begin(), cores.end(), s.core) == cores.end()) cores.push_back(s.core);
		std::sort(cores.begin(), cores.end());
		// The WrapPanel (margin 0,3,0,8): core boxes left to right, a new line when the next one does not fit.
		Space(3);
		float x0 = ImGui::GetCursorScreenPos().x, right = BoxRight();
		float x = x0, y = ImGui::GetCursorScreenPos().y, lineH = 0;
		ImDrawList* dl = ImGui::GetWindowDrawList();
		float c = S(cell), m1 = S(1), bw = Px1();
		for (int core : cores)
		{
			std::vector<const LpSet*> lps;
			for (const LpSet& s : t.sets)
				if (s.group == 0 && s.llc == llc && s.core == core) lps.push_back(&s);
			std::sort(lps.begin(), lps.end(), [](const LpSet* a, const LpSet* b) { return a->lp < b->lp; });
			float n = (float)lps.size();
			float inW = stacked ? c + 2 * m1 : n * (c + 2 * m1);
			float inH = stacked ? n * (c + 2 * m1) : c + 2 * m1;
			float boxW = inW + 2 * S(2) + 2 * bw, boxH = inH + 2 * S(2) + 2 * bw;
			float outW = boxW + S(5), outH = boxH + S(5);
			if (x > x0 && x + boxW > right)
			{
				x = x0;
				y += lineH;
				lineH = 0;
			}
			ImRect box(ImVec2(x, y), ImVec2(x + boxW, y + boxH));
			DrawBorder(dl, box, 0, C("Line"), 1, 3);
			float cx = box.Min.x + bw + S(2), cy = box.Min.y + bw + S(2);
			for (const LpSet* s : lps)
			{
				LpLook lk = look ? look(*s) : LpLook{};
				ImRect cr(ImVec2(cx + m1, cy + m1), ImVec2(cx + m1 + c, cy + m1 + c));
				ImGuiID cid = ImGui::GetID(s->lp);
				ImGui::ItemAdd(cr, cid, nullptr, ImGuiItemFlags_NoNav);
				if (clickable)
				{
					bool hov = false, held = false;
					if (ImGui::ButtonBehavior(cr, cid, &hov, &held)) clicked = s->lp;
					if (hov) ImGui::SetMouseCursor(ImGuiMouseCursor_Hand);
				}
				DrawBorder(dl, cr, lk.fill, lk.ring, 2, 2);
				std::string num = std::to_string(s->lp);
				ImVec2 ts = Measure(Face::Mono, Weight::Normal, FsSmall, num);
				TextAt(dl, cr.GetCenter() - ts * 0.5f, Face::Mono, Weight::Normal, FsSmall, C("Strong"), num);
				if (!lk.tip.empty()) Tip(lk.tip);
				if (stacked) cy += c + 2 * m1;
				else cx += c + 2 * m1;
			}
			x += outW;
			lineH = std::max(lineH, outH);
		}
		float total = (y + lineH) - ImGui::GetCursorScreenPos().y;
		Place(ImVec2(std::max(0.0f, right - x0), std::max(0.0f, total)), Thick(0, 0, 0, 8));
	}
	ImGui::PopID();
	return clicked;
}

} // namespace ui
