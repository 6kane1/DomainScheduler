#include "chrome.h"

#include <dwmapi.h>

#include <cmath>
#include <vector>

namespace chrome {

namespace {
constexpr DWORD DWMWA_USE_IMMERSIVE_DARK_MODE_ = 20;
constexpr DWORD DWMWA_WINDOW_CORNER_PREFERENCE_ = 33;
constexpr DWORD DWMWA_BORDER_COLOR_ = 34;
constexpr int DWMWCP_ROUND_ = 2;

std::vector<ImRect> g_hits;

using GetSystemMetricsForDpiFn = int(WINAPI*)(int, UINT);
using GetDpiForWindowFn = UINT(WINAPI*)(HWND);

GetSystemMetricsForDpiFn MetricsForDpi()
{
	static GetSystemMetricsForDpiFn f = (GetSystemMetricsForDpiFn)(void*)GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetSystemMetricsForDpi");
	return f;
}

UINT WindowDpi(HWND hwnd)
{
	static GetDpiForWindowFn f = (GetDpiForWindowFn)(void*)GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetDpiForWindow");
	UINT d = f ? f(hwnd) : 96;
	return d ? d : 96;
}
} // namespace

bool Dwm(HWND hwnd)
{
	if (hwnd == nullptr) return false;
	BOOL dark = TRUE;
	DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE_, &dark, sizeof dark);
	int round = DWMWCP_ROUND_;
	bool win11 = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE_, &round, sizeof round) == S_OK;
	ImU32 c = ui::C("Line");
	COLORREF ref = RGB((c >> IM_COL32_R_SHIFT) & 0xFF, (c >> IM_COL32_G_SHIFT) & 0xFF, (c >> IM_COL32_B_SHIFT) & 0xFF);
	DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR_, &ref, sizeof ref);
	return win11;
}

ui::Thick MaximizedPadding(HWND hwnd)
{
	UINT d = WindowDpi(hwnd);
	double scale = d / 96.0;
	int fx, fy, pad;
	if (GetSystemMetricsForDpiFn f = MetricsForDpi())
	{
		fx = f(SM_CXSIZEFRAME, d);
		fy = f(SM_CYSIZEFRAME, d);
		pad = f(SM_CXPADDEDBORDER, d);
	}
	else
	{
		fx = GetSystemMetrics(SM_CXSIZEFRAME);
		fy = GetSystemMetrics(SM_CYSIZEFRAME);
		pad = GetSystemMetrics(SM_CXPADDEDBORDER);
	}
	double x = (fx + pad) / scale, y = (fy + pad) / scale;
	if (x <= 0.0 || y <= 0.0 || x > 32.0 || y > 32.0) return ui::Thick(8.0f);
	return ui::Thick((float)x, (float)y, (float)x, (float)y);
}

void CapButton(HWND hwnd, Cap kind, ImVec2 pos, const std::string& tip)
{
	const char* id = kind == Cap::Minimize ? "##cap_min" : kind == Cap::Maximize ? "##cap_max" : "##cap_close";
	ImGuiID gid = ImGui::GetID(id);
	ImRect r(pos, pos + ImVec2(std::round(ui::S(44)), std::round(ui::S(CaptionHeight - 1))));
	ImGui::ItemAdd(r, gid);
	AddHit(r);
	bool hovered = false, held = false;
	bool clicked = ImGui::ButtonBehavior(r, gid, &hovered, &held);
	ImDrawList* dl = ImGui::GetWindowDrawList();
	if (hovered && !clicked) dl->AddRectFilled(r.Min, r.Max, ui::C(kind == Cap::Close ? "CloseHover" : "Sel"));
	ImU32 col = ui::C(hovered && !clicked ? (kind == Cap::Close ? "Strong" : "Accent") : "Silver");
	// The glyph: a 10x10 path, Stretch.None, stroke 1, snapped to device pixels, centred.
	float g = std::round(ui::S(10));
	ImVec2 o(std::round(r.GetCenter().x - g / 2), std::round(r.GetCenter().y - g / 2));
	float t = std::max(1.0f, std::round(ui::S(1)));
	auto P = [&](float x, float y) { return o + ImVec2(ui::S(x), ui::S(y)); };
	bool maximized = IsZoomed(hwnd) != FALSE;
	switch (kind)
	{
	case Cap::Minimize:   // M0,5.5 L10,5.5
		dl->AddRectFilled(ImVec2(o.x, o.y + std::round(ui::S(5))), ImVec2(o.x + g, o.y + std::round(ui::S(5)) + t), col);
		break;
	case Cap::Close:   // M0.5,0.5 L9.5,9.5 M9.5,0.5 L0.5,9.5
		dl->AddLine(P(0.5f, 0.5f), P(9.5f, 9.5f), col, ui::S(1));
		dl->AddLine(P(9.5f, 0.5f), P(0.5f, 9.5f), col, ui::S(1));
		break;
	case Cap::Maximize:
		if (maximized)
		{
			// M2.5,2.5 L2.5,0.5 L9.5,0.5 L9.5,7.5 L7.5,7.5 M0.5,2.5 L7.5,2.5 L7.5,9.5 L0.5,9.5 Z
			ImVec2 a[5] = {P(2.5f, 2.5f), P(2.5f, 0.5f), P(9.5f, 0.5f), P(9.5f, 7.5f), P(7.5f, 7.5f)};
			dl->AddPolyline(a, 5, col, ImDrawFlags_None, t);
			ImVec2 b[4] = {P(0.5f, 2.5f), P(7.5f, 2.5f), P(7.5f, 9.5f), P(0.5f, 9.5f)};
			dl->AddPolyline(b, 4, col, ImDrawFlags_Closed, t);
		}
		else
		{
			// M0.5,0.5 L9.5,0.5 L9.5,9.5 L0.5,9.5 Z
			dl->AddRect(o, o + ImVec2(g, g), col, 0, 0, t);   // ImGui strokes 0.5 px inside: the 0.5..9.5 path
		}
		break;
	}
	if (hovered && !tip.empty()) ui::Tip(tip);
	if (!clicked) return;
	switch (kind)
	{
	case Cap::Minimize:
		PostMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
		break;
	case Cap::Maximize:
		PostMessageW(hwnd, WM_SYSCOMMAND, maximized ? SC_RESTORE : SC_MAXIMIZE, 0);
		break;
	default:
		PostMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
		break;
	}
}

void Logo(ImDrawList* dl, ImVec2 pos, float sizeDip)
{
	// Clip "M0,0 L18,0 L18,18 L5.4,18 L0,12.6 Z"; pink "M0,0 L18,0 L0,18 Z"; silver "M18,0 L18,18 L0,18 Z".
	// The two clipped parts, split along the diagonal x + y = 18 (the cut meets it at 2.7,15.3).
	float k = ui::S(sizeDip) / 18.0f;
	auto P = [&](float x, float y) { return ImVec2(pos.x + x * k, pos.y + y * k); };
	ImVec2 pink[4] = {P(0, 0), P(18, 0), P(2.7f, 15.3f), P(0, 12.6f)};
	ImVec2 silver[4] = {P(18, 0), P(18, 18), P(5.4f, 18), P(2.7f, 15.3f)};
	dl->AddConvexPolyFilled(pink, 4, ui::C("Accent"));
	dl->AddConvexPolyFilled(silver, 4, ui::C("Silver"));
}

void ClearHits() { g_hits.clear(); }

void AddHit(ImRect r) { g_hits.push_back(r); }

LRESULT HitTest(HWND hwnd, POINT pt, float captionPx, bool maximized)
{
	RECT rc;
	GetClientRect(hwnd, &rc);
	float b = ui::S(ResizeBorder);
	if (!maximized)
	{
		bool l = pt.x < b, r = pt.x >= rc.right - b, t = pt.y < b, d = pt.y >= rc.bottom - b;
		if (t && l) return HTTOPLEFT;
		if (t && r) return HTTOPRIGHT;
		if (d && l) return HTBOTTOMLEFT;
		if (d && r) return HTBOTTOMRIGHT;
		if (l) return HTLEFT;
		if (r) return HTRIGHT;
		if (t) return HTTOP;
		if (d) return HTBOTTOM;
	}
	ImVec2 p((float)pt.x, (float)pt.y);
	for (const ImRect& h : g_hits)
		if (h.Contains(p)) return HTCLIENT;
	if (pt.y < captionPx) return HTCAPTION;
	return HTCLIENT;
}

} // namespace chrome
