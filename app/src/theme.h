// The HUD look (port of Ui.cs palette/metrics and Theme.cs styles): colours, font sizes, fonts, the DPI scale.
// Owner: app shell.
//
// Units: every size in this app is written in WPF device-independent units (1/96 inch), exactly the numbers of
// the C# code; ui::S() turns them into pixels of the window's monitor (per-monitor DPI v2).
#pragma once
#include <string_view>

#include "imgui.h"

namespace ui {

// ---- DPI
float Dpi();                         // the window's scale (1.0 at 96 DPI)
void SetDpi(float scale);
inline float S(float dip) { return dip * Dpi(); }
inline ImVec2 S(float x, float y) { return ImVec2(x * Dpi(), y * Dpi()); }

// ---- palette (Ui.Palette, 1.14.0 "Cockpit")
/// The colour of a palette key ("Bg", "Accent", ...); an unknown key gives "Text" like Ui.B.
ImU32 C(std::string_view key, float opacity = 1.0f);
ImVec4 C4(std::string_view key, float opacity = 1.0f);
inline ImU32 Rgb(int r, int g, int b, int a = 255) { return IM_COL32(r, g, b, a); }
/// Multiplies the alpha of a colour (WPF Opacity on an element).
ImU32 Fade(ImU32 c, float opacity);

// ---- metrics (Ui.cs); font sizes are WPF em sizes in DIP: Pt(x) = x * 96 / 72
constexpr float Pt(float pt) { return pt * 96.0f / 72.0f; }
constexpr float FsSmall = Pt(8.5f);
constexpr float FsNote = Pt(8.75f);
constexpr float FsBody = Pt(10.0f);
constexpr float FsHero = Pt(48.0f);
constexpr float RowH = 32.0f;
constexpr float Gap = 12.0f;
constexpr float IconScale = 0.46f;
constexpr float Radius = 4.0f;

struct Thick {
	float l = 0, t = 0, r = 0, b = 0;
	constexpr Thick() = default;
	constexpr Thick(float all) : l(all), t(all), r(all), b(all) {}
	constexpr Thick(float h, float v) : l(h), t(v), r(h), b(v) {}
	constexpr Thick(float l_, float t_, float r_, float b_) : l(l_), t(t_), r(r_), b(b_) {}
};
constexpr Thick MTop{0, 15, 0, 0};
constexpr Thick MCell{0, 1, 6, 0};
constexpr Thick MRow{0, 0, 0, 6};

// ---- fonts (embedded Inter and JetBrains Mono, SIL OFL; Segoe UI / Segoe UI Symbol merged in as the fallback
// for glyphs the two do not have, like WPF's font fallback)
enum class Face { Ui, Mono };
enum class Weight { Normal, Medium, Semi };
/// The ImGui font for a face and weight.
ImFont* Font(Face f, Weight w);
/// The ImGui size (line height in px) that draws glyphs at WPF em size `emDip` (scaled by the DPI).
float FontPx(Face f, float emDip);
/// Pushes a font at a WPF em size in DIP; pop with PopFont() or use FontScope.
void PushFont(Face f, Weight w, float emDip);
void PopFont();
struct FontScope {
	FontScope(Face f, Weight w, float emDip) { PushFont(f, w, emDip); }
	~FontScope() { PopFont(); }
	FontScope(const FontScope&) = delete;
	FontScope& operator=(const FontScope&) = delete;
};

/// Loads the fonts and sets the ImGui style (Theme.Apply). Call once after ImGui::CreateContext.
void ApplyTheme();
/// Re-applies the sizes of the ImGui style after a DPI change.
void ScaleStyle();
/// True when the embedded fonts were found (Ui.EmbeddedFonts).
bool EmbeddedFonts();

} // namespace ui
