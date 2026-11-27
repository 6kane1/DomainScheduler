// Chrome.cs: our own title bar. The system keeps drag, double-click, snap and resize (WM_NCHITTEST answers
// HTCAPTION / the resize edges); the caption buttons are drawn in the HUD style; DWM gives the dark frame, the
// rounded corners and the border colour on Windows 11. Owner: app shell.
#pragma once
#include <windows.h>

#include <functional>
#include <string>

#include "ui.h"

namespace chrome {

constexpr float CaptionHeight = 46.0f;   // DIP
constexpr float ResizeBorder = 6.0f;     // WindowChrome.ResizeBorderThickness

/// Dark frame, round corners, a border in the panel line colour. True when Windows 11 drew the border (then the
/// window needs no border of its own).
bool Dwm(HWND hwnd);
/// A maximized window reaches past the screen by the sizing frame: this padding (DIP) keeps the content on it.
ui::Thick MaximizedPadding(HWND hwnd);

enum class Cap { Minimize, Maximize, Close };
/// A caption button (44 x 45 DIP) at `pos` px: thin glyph, a pink-tinted hover, a muted red hover for close.
/// Runs the system command itself on click.
void CapButton(HWND hwnd, Cap kind, ImVec2 pos, const std::string& tip);

/// The logo: a square cut at the lower-left corner, pink over silver along the diagonal.
void Logo(ImDrawList* dl, ImVec2 pos, float sizeDip = 18.0f);

/// The interactive parts of the caption (buttons): WM_NCHITTEST answers HTCLIENT there. Rebuilt every frame.
void ClearHits();
void AddHit(ImRect r);
/// WM_NCHITTEST for a point in client pixels: HTCLIENT, HTCAPTION or one of the resize edges.
LRESULT HitTest(HWND hwnd, POINT client, float captionPx, bool maximized);

} // namespace chrome
