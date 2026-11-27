#include "theme.h"

#include <windows.h>

#include <cmath>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

#include "core.h"

namespace ui {

namespace {

float g_dpi = 1.0f;

struct Entry {
	const char* key;
	ImU32 c;
};

// Color.FromRgb / FromArgb of Ui.cs, in the same order.
#define RGB_(r, g, b) IM_COL32(r, g, b, 255)
#define ARGB_(a, r, g, b) IM_COL32(r, g, b, a)
const Entry kPalette[] = {
	{"Bg", RGB_(6, 6, 8)},
	{"Pop", RGB_(19, 20, 25)},
	{"Ink", RGB_(42, 24, 32)},
	{"Panel", ARGB_(190, 18, 17, 24)},   // 27.11.2026: translucent so the Aurora background shows through
	{"Panel2", ARGB_(10, 255, 255, 255)},
	{"Well", ARGB_(90, 0, 0, 0)},
	{"Line", RGB_(34, 35, 42)},
	{"Line2", RGB_(23, 24, 29)},
	{"Hover", ARGB_(14, 244, 182, 203)},
	{"Sel", ARGB_(30, 244, 182, 203)},
	{"Strong", RGB_(230, 232, 236)},
	{"Text", RGB_(201, 206, 214)},
	{"Dim", RGB_(125, 131, 141)},
	{"Faint", RGB_(93, 99, 108)},
	{"Silver", RGB_(201, 206, 214)},
	{"Steel", RGB_(143, 147, 160)},
	{"Accent", RGB_(244, 182, 203)},
	{"AccentText", RGB_(247, 198, 214)},
	{"AccentSoft", ARGB_(33, 244, 182, 203)},
	{"AccentLine", ARGB_(90, 244, 182, 203)},
	{"Bracket", ARGB_(204, 244, 182, 203)},
	{"On", RGB_(244, 182, 203)},
	{"Warn", RGB_(232, 194, 122)},
	{"WarnSoft", ARGB_(22, 232, 194, 122)},
	{"WarnLine", ARGB_(64, 232, 194, 122)},
	{"Error", RGB_(239, 138, 138)},
	{"CloseHover", RGB_(122, 46, 54)},
	{"Game", ARGB_(92, 244, 182, 203)},
	{"NonGame", ARGB_(56, 143, 147, 160)},
	{"Irq", RGB_(232, 194, 122)},
	{"Overlay", ARGB_(200, 6, 6, 8)},
	{"None", ARGB_(0, 0, 0, 0)},
	{"HazePink", ARGB_(33, 244, 182, 203)},
	{"HazeSilver", ARGB_(26, 190, 198, 214)},
};
#undef RGB_
#undef ARGB_

ImU32 Lookup(std::string_view key)
{
	for (const Entry& e : kPalette)
		if (key == e.key) return e.c;
	return kPalette[11].c;   // "Text"
}

// ---- fonts
struct FontSet {
	ImFont* f[3] = {nullptr, nullptr, nullptr};
	float ratio = 1.21f;   // (hhea ascent - descent) / unitsPerEm of the main font
};
FontSet g_ui, g_mono;
bool g_embedded = false;

struct Blob {
	const void* data = nullptr;
	int size = 0;
};

Blob Resource(const wchar_t* name)
{
	HMODULE m = GetModuleHandleW(nullptr);
	HRSRC r = FindResourceW(m, name, MAKEINTRESOURCEW(10));   // RT_RCDATA
	if (r == nullptr) return {};
	HGLOBAL g = LoadResource(m, r);
	if (g == nullptr) return {};
	return {LockResource(g), (int)SizeofResource(m, r)};
}

std::vector<std::vector<unsigned char>> g_files;   // system fallback fonts kept for the atlas

Blob SystemFont(const wchar_t* file)
{
	wchar_t dir[MAX_PATH];
	UINT n = GetWindowsDirectoryW(dir, MAX_PATH);
	if (n == 0 || n >= MAX_PATH) return {};
	std::wstring path = std::wstring(dir) + L"\\Fonts\\" + file;
	HANDLE h = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
	if (h == INVALID_HANDLE_VALUE) return {};
	LARGE_INTEGER size{};
	std::vector<unsigned char> buf;
	if (GetFileSizeEx(h, &size) && size.QuadPart > 0 && size.QuadPart < 64 * 1024 * 1024)
	{
		buf.resize((size_t)size.QuadPart);
		DWORD got = 0;
		if (!ReadFile(h, buf.data(), (DWORD)buf.size(), &got, nullptr) || got != buf.size()) buf.clear();
	}
	CloseHandle(h);
	if (buf.empty()) return {};
	g_files.push_back(std::move(buf));
	return {g_files.back().data(), (int)g_files.back().size()};
}

uint16_t Be16(const unsigned char* p) { return (uint16_t)((p[0] << 8) | p[1]); }
uint32_t Be32(const unsigned char* p) { return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3]; }

/// (hhea ascender - descender) / head.unitsPerEm: stb_truetype sizes a font by its ascent-descent, WPF by its em.
float EmRatio(const Blob& b)
{
	if (b.data == nullptr || b.size < 12) return 1.21f;
	const unsigned char* d = (const unsigned char*)b.data;
	uint32_t off = 0;
	if (Be32(d) == 0x74746366) off = Be32(d + 12);   // 'ttcf': the first font of a collection
	if ((int)off + 12 > b.size) return 1.21f;
	int tables = Be16(d + off + 4);
	int upm = 0, asc = 0, desc = 0;
	for (int i = 0; i < tables; i++)
	{
		const unsigned char* t = d + off + 12 + i * 16;
		if (t + 16 > d + b.size) break;
		uint32_t tag = Be32(t), at = Be32(t + 8);
		if (at + 8 > (uint32_t)b.size) continue;
		if (tag == 0x68656164 && at + 20 <= (uint32_t)b.size) upm = Be16(d + at + 18);                  // 'head'
		if (tag == 0x68686561) { asc = (int16_t)Be16(d + at + 4); desc = (int16_t)Be16(d + at + 6); }  // 'hhea'
	}
	if (upm <= 0 || asc - desc <= 0) return 1.21f;
	return (float)(asc - desc) / (float)upm;
}

ImFont* AddFace(const Blob& main, float mainRatio, const std::vector<Blob>& fallbacks)
{
	ImGuiIO& io = ImGui::GetIO();
	ImFontConfig cfg;
	cfg.FontDataOwnedByAtlas = false;
	cfg.SizePixels = 16.0f;
	cfg.OversampleH = 2;
	cfg.OversampleV = 1;
	ImFont* f = io.Fonts->AddFontFromMemoryTTF(const_cast<void*>(main.data), main.size, 16.0f, &cfg);
	if (f == nullptr) return nullptr;
	for (const Blob& b : fallbacks)
	{
		if (b.data == nullptr) continue;
		ImFontConfig m;
		m.FontDataOwnedByAtlas = false;
		m.MergeMode = true;
		// A merged glyph keeps the em size of the main font (see EmRatio): SizePixels relative to the main one.
		m.SizePixels = 16.0f * EmRatio(b) / mainRatio;
		m.OversampleH = 2;
		m.OversampleV = 1;
		io.Fonts->AddFontFromMemoryTTF(const_cast<void*>(b.data), b.size, m.SizePixels, &m);
	}
	return f;
}

std::vector<ImFont*> g_stack;

} // namespace

float Dpi() { return g_dpi; }

void SetDpi(float scale) { g_dpi = scale > 0.1f ? scale : 1.0f; }

ImU32 C(std::string_view key, float opacity) { return opacity >= 1.0f ? Lookup(key) : Fade(Lookup(key), opacity); }

ImVec4 C4(std::string_view key, float opacity) { return ImGui::ColorConvertU32ToFloat4(C(key, opacity)); }

ImU32 Fade(ImU32 c, float opacity)
{
	if (opacity >= 1.0f) return c;
	unsigned a = (c >> IM_COL32_A_SHIFT) & 0xFF;
	a = (unsigned)std::lround(a * (opacity < 0 ? 0 : opacity));
	return (c & ~IM_COL32_A_MASK) | (a << IM_COL32_A_SHIFT);
}

ImFont* Font(Face f, Weight w)
{
	FontSet& s = f == Face::Mono ? g_mono : g_ui;
	ImFont* r = s.f[(int)w];
	return r != nullptr ? r : ImGui::GetIO().Fonts->Fonts.Size > 0 ? ImGui::GetIO().Fonts->Fonts[0] : nullptr;
}

float FontPx(Face f, float emDip)
{
	const FontSet& s = f == Face::Mono ? g_mono : g_ui;
	return emDip * s.ratio * g_dpi;
}

void PushFont(Face f, Weight w, float emDip)
{
	ImFont* font = Font(f, w);
	g_stack.push_back(font);
	ImGui::PushFont(font, FontPx(f, emDip));
}

void PopFont()
{
	if (!g_stack.empty()) g_stack.pop_back();
	ImGui::PopFont();
}

bool EmbeddedFonts() { return g_embedded; }

void ApplyTheme()
{
	ImGuiIO& io = ImGui::GetIO();
	io.IniFilename = nullptr;
	io.LogFilename = nullptr;
	// The Mono family is "JetBrains Mono, Inter, Consolas": Inter covers the narrow spaces of the captions.
	// Inter is "Inter, <language fonts>, Segoe UI". Segoe UI Symbol covers the rest (arrows, check marks).
	Blob inter[3] = {Resource(L"INTER_REGULAR"), Resource(L"INTER_MEDIUM"), Resource(L"INTER_SEMIBOLD")};
	Blob mono[3] = {Resource(L"MONO_REGULAR"), Resource(L"MONO_MEDIUM"), Resource(L"MONO_SEMIBOLD")};
	Blob segoe = SystemFont(L"segoeui.ttf");
	Blob symbol = SystemFont(L"seguisym.ttf");
	Blob consolas = SystemFont(L"consola.ttf");
	g_embedded = inter[0].data != nullptr && mono[0].data != nullptr;
	if (g_embedded)
	{
		g_ui.ratio = EmRatio(inter[0]);
		g_mono.ratio = EmRatio(mono[0]);
		for (int w = 0; w < 3; w++)
		{
			const Blob& i = inter[w].data ? inter[w] : inter[0];
			const Blob& m = mono[w].data ? mono[w] : mono[0];
			g_ui.f[w] = AddFace(i, g_ui.ratio, {segoe, symbol});
			g_mono.f[w] = AddFace(m, g_mono.ratio, {i, consolas, segoe, symbol});
		}
	}
	else if (segoe.data != nullptr)
	{
		// The embedded fonts are missing: the system fonts, like Ui.Font() without Embedded.
		g_ui.ratio = EmRatio(segoe);
		g_mono.ratio = consolas.data ? EmRatio(consolas) : g_ui.ratio;
		for (int w = 0; w < 3; w++)
		{
			g_ui.f[w] = AddFace(segoe, g_ui.ratio, {symbol});
			g_mono.f[w] = consolas.data ? AddFace(consolas, g_mono.ratio, {segoe, symbol}) : g_ui.f[w];
		}
	}
	if (io.Fonts->Fonts.Size == 0) io.Fonts->AddFontDefault();
	if (g_ui.f[0] == nullptr)
	{
		for (int w = 0; w < 3; w++) g_ui.f[w] = g_mono.f[w] = io.Fonts->Fonts[0];
		g_ui.ratio = g_mono.ratio = 1.0f;
	}
	ImGuiStyle& st = ImGui::GetStyle();
	ImVec4* c = st.Colors;
	ImGui::StyleColorsDark(&st);
	c[ImGuiCol_Text] = C4("Text");
	c[ImGuiCol_TextDisabled] = C4("Dim");
	c[ImGuiCol_WindowBg] = C4("None");
	c[ImGuiCol_ChildBg] = C4("None");
	c[ImGuiCol_PopupBg] = C4("Pop");
	c[ImGuiCol_Border] = C4("AccentLine");
	c[ImGuiCol_BorderShadow] = C4("None");
	c[ImGuiCol_FrameBg] = C4("Panel2");
	c[ImGuiCol_FrameBgHovered] = C4("Panel2");
	c[ImGuiCol_FrameBgActive] = C4("Panel2");
	// ScrollBar (Theme.cs): 7 px, a transparent track, the thumb in Line, Dim on hover, Accent while dragged.
	c[ImGuiCol_ScrollbarBg] = C4("None");
	c[ImGuiCol_ScrollbarGrab] = C4("Line");
	c[ImGuiCol_ScrollbarGrabHovered] = C4("Dim");
	c[ImGuiCol_ScrollbarGrabActive] = C4("Accent");
	c[ImGuiCol_TextSelectedBg] = C4("Dim", 0.6f);   // TextBox SelectionBrush Dim (WPF draws it at 0.4 opacity)
	c[ImGuiCol_InputTextCursor] = C4("Strong");
	c[ImGuiCol_NavCursor] = C4("Accent");
	c[ImGuiCol_ModalWindowDimBg] = C4("Overlay");
	c[ImGuiCol_Header] = C4("Sel");
	c[ImGuiCol_HeaderHovered] = C4("Sel");
	c[ImGuiCol_HeaderActive] = C4("Sel");
	c[ImGuiCol_Button] = C4("Panel2");
	c[ImGuiCol_ButtonHovered] = C4("Hover");
	c[ImGuiCol_ButtonActive] = C4("Sel");
	c[ImGuiCol_CheckMark] = C4("Accent");
	c[ImGuiCol_SliderGrab] = C4("Accent");
	c[ImGuiCol_SliderGrabActive] = C4("Accent");
	c[ImGuiCol_Separator] = C4("Line");
	ScaleStyle();
}

void ScaleStyle()
{
	ImGuiStyle& st = ImGui::GetStyle();
	st.FontScaleDpi = 1.0f;   // the app scales every size itself (ui::S, FontPx)
	st.WindowPadding = ImVec2(0, 0);
	st.WindowRounding = 0;
	st.WindowBorderSize = 0;
	st.ChildRounding = 0;
	st.ChildBorderSize = 0;
	st.PopupRounding = S(3);
	st.PopupBorderSize = 1.0f;
	st.FramePadding = S(6, 0);
	st.FrameRounding = S(3);
	st.FrameBorderSize = 0;
	st.ItemSpacing = ImVec2(0, 0);
	st.ItemInnerSpacing = ImVec2(0, 0);
	st.CellPadding = ImVec2(0, 0);
	st.IndentSpacing = 0;
	st.ScrollbarSize = S(7);
	st.ScrollbarRounding = S(1);
	st.ScrollbarPadding = S(1);   // the thumb's Margin 1,0
	st.GrabMinSize = S(12);
	st.GrabRounding = 0;
	st.AntiAliasedLines = true;
	st.AntiAliasedFill = true;
	st.HoverDelayNormal = 0.40f;   // ToolTipService.InitialShowDelay (SystemParameters.MouseHoverTime, 400 ms)
	st.HoverDelayShort = 0.10f;
	st.HoverStationaryDelay = 0.0f;
	st.DisabledAlpha = 0.45f;      // the controls' IsEnabled=False opacity
	st.InputTextCursorSize = 1.0f;
}

} // namespace ui
