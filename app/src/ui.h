// Ui.cs + the Theme.cs control templates as Dear ImGui widgets. Owner: app shell; used by every page.
//
// Layout model (a small copy of WPF's StackPanel / DockPanel behaviour on top of ImGui's cursor):
//  - widgets go below each other (a vertical StackPanel) unless they are inside a ui::Row (horizontal StackPanel);
//  - every widget takes a margin in DIP (ui::Thick, the WPF Margin) and its size in DIP;
//  - ui::Group makes one block of several widgets (a nested StackPanel), ui::Box gives it a fixed width;
//  - ui::BeginCard / EndCard draw the HUD panel around whatever is drawn between them;
//  - ui::BeginScroll / EndScroll is a ScrollViewer (vertical, 7 px HUD scrollbar).
// Every size passed in is DIP; the functions scale with ui::S. Colours are palette keys ("Accent", "Dim" ...).
#pragma once
#include <cstdint>
#include <functional>
#include <string>
#include <string_view>
#include <vector>

#include "imgui.h"
#include "imgui_internal.h"
#include "theme.h"

namespace ui {

// ---------------------------------------------------------------- text helpers (HudMath.Spaced, Ui.Hex, Ui.Size)
/// Upper case in the current culture (invariant = CultureInfo.InvariantCulture).
std::string Upper(std::string_view s, bool invariant = false);
/// HudMath.Spaced: upper case with a narrow no-break space (U+202F) after every character.
std::string Spaced(std::string_view s, bool invariant = false);
/// HudMath.Elapsed: hh:mm:ss, hours keep counting past 24.
std::string Elapsed(double seconds);
/// Ui.Hex: "0x" + upper-case hex.
std::string Hex(uint64_t m);
/// Ui.Size: KB / MB / GB with the current culture's decimal separator.
std::string Size(int64_t bytes);
/// A number in the current culture with `decimals` digits (the "0", "0.0" ... formats of the C# code).
std::string Num(double v, int decimals);
/// The culture's decimal separator (",", "." ...).
const std::string& DecimalSep();
std::wstring Wide(std::string_view s);
std::string Utf8(std::wstring_view s);

// ---------------------------------------------------------------- drawing primitives (pixels)
/// The size of a text in px (wrapW > 0: wrapped at that width in px).
ImVec2 Measure(Face f, Weight w, float em, std::string_view s, float wrapW = 0);
/// Draws a text with its top-left at pos; maxW > 0 trims it with "…" (TextTrimming.CharacterEllipsis);
/// wrapW > 0 wraps it.
void TextAt(ImDrawList* dl, ImVec2 pos, Face f, Weight w, float em, ImU32 col, std::string_view s, float maxW = 0, float wrapW = 0);
/// TextAt trimmed to maxW (px); when the text had to be trimmed, hovering its rect shows the full text as a tooltip
/// (native-only: names stay readable in a narrow window). Returns the rect it took.
ImRect TextFit(ImDrawList* dl, ImVec2 pos, Face f, Weight w, float em, ImU32 col, std::string_view s, float maxW, ImGuiID id);
/// Column widths in px for `totalPx` from widths in DIP (0 = a star column, the name column). Star columns keep at
/// least `starMinDip` each: the fixed (number) columns shrink first, down to `minShare` of their width, before the
/// star columns get less. Never wider than totalPx in sum while that is possible.
std::vector<float> FitColumns(const std::vector<float>& dip, float totalPx, float starMinDip = 160.0f, float minShare = 0.5f);
/// A WPF Border: background inside the line, a line of `thick` DIP, corner radius in DIP.
void DrawBorder(ImDrawList* dl, ImRect r, ImU32 bg, ImU32 line, float thickDip, float radiusDip);
/// Partial border lines (BorderThickness l,t,r,b; no radius), each one DIP-scaled and pixel-snapped.
void DrawEdges(ImDrawList* dl, ImRect r, ImU32 col, Thick t);
/// A path of straight segments in a box of `size` DIP placed at `origin` px, the WPF way: Stretch.None draws the
/// points as they are; Stretch.Uniform scales the geometry's bounds into the box minus the stroke.
void DrawPath(ImDrawList* dl, ImVec2 origin, const std::vector<std::vector<ImVec2>>& figures, ImU32 col, float strokeDip, bool uniform, ImVec2 boxDip, bool closed = false);
/// A dashed line (WPF DashStyle: dash/gap in multiples of the pen width).
void DashedLine(ImDrawList* dl, ImVec2 a, ImVec2 b, ImU32 col, float thickPx, float dash, float gap);
/// The window background: near-black with a faint 24 px grid (Ui.GridBackground).
void GridBackground(ImDrawList* dl, ImRect r, ImVec2 origin);
/// Ui.Backdrop: a radial haze (pink from the top-left, silver from the bottom-right) over a rectangle.
void Backdrop(ImDrawList* dl, ImRect r, bool pink);
/// A 10 px corner bracket (Ui.Bracket) at the top-left or bottom-right of r.
void Bracket(ImDrawList* dl, ImRect r, bool top);

// ---------------------------------------------------------------- layout
/// Allocates a widget of `size` px with a margin in DIP at the cursor (row-aware). Returns the widget's rect.
ImRect Place(ImVec2 sizePx, Thick margin = {}, ImGuiID id = 0);
/// The width left from the cursor to the right edge of the current box, in px.
float AvailW();
/// The right edge of the current box, in px (screen coordinates).
float BoxRight();
/// Adds vertical space (DIP).
void Space(float dip);
/// True inside a Row (widgets go side by side).
bool InRow();

/// A horizontal StackPanel (Ui.Row: Margin = MRow unless given). Widgets inside go side by side.
struct Row {
	explicit Row(Thick margin = MRow);
	~Row();
	Row(const Row&) = delete;
	Row& operator=(const Row&) = delete;
	Thick m;
};

/// A vertical block treated as one widget by the parent (a nested StackPanel / DockPanel child).
/// width > 0: a fixed width in DIP (WPF Width); otherwise it takes what the content needs.
struct Group {
	explicit Group(float widthDip = 0, Thick margin = {});
	~Group();
	Group(const Group&) = delete;
	Group& operator=(const Group&) = delete;
	Thick m;
	ImVec2 start;
	float width;
	ImRect rect;   // valid after the destructor ran (use ui::LastRect)
};
/// The rect of the last finished Group / Card / widget.
ImRect LastRect();
/// Puts the next widget at a fixed width: the box inside is limited to that width (for wrap / trimming).
struct Box {
	Box(float x0Px, float x1Px);
	~Box();
	Box(const Box&) = delete;
	Box& operator=(const Box&) = delete;
};

/// A ScrollViewer: vertical scrolling inside `size` px (0 = the rest of the box / window).
void BeginScroll(const char* id, ImVec2 sizePx = ImVec2(0, 0), bool horizontal = false);
void EndScroll();

/// Ui.Card: the HUD panel with the padding pad+3, pad+1, a 10 DIP margin below. Content goes between.
void BeginCard(float pad = 12.0f);
void EndCard();
/// Ui.Panel: the panel without a margin, with the padding given; brackets on the corners by default.
void BeginPanel(Thick pad, bool brackets = true, Thick margin = {});
void EndPanel();

// ---------------------------------------------------------------- text widgets
struct TextStyle {
	float size = 0;              // em in DIP; 0 = FsBody
	const char* brush = "Text";
	Face face = Face::Ui;
	Weight weight = Weight::Normal;
	bool wrap = false;           // TextWrapping.Wrap; else NoWrap + CharacterEllipsis
	Thick margin = {};
	float lineHeight = 0;        // TextBlock.LineHeight in DIP (0 = the font's)
	float width = 0;             // a fixed width in DIP (0 = what is left / what the text needs)
	bool center = false;         // HorizontalAlignment.Center inside the width
	bool right = false;          // HorizontalAlignment.Right inside the width
};
/// Ui.Text.
void Text(std::string_view s, float size = 0, const char* brush = "Text", bool bold = false, bool wrap = false, Thick margin = {});
void TextEx(std::string_view s, const TextStyle& st);
/// Ui.Note: FsNote, Dim, wrapped.
void Note(std::string_view s, Thick margin = {});
/// Ui.Caps: spaced capitals in the mono font.
void Caps(std::string_view s, float size = 0, const char* brush = "Dim", bool semi = true, Thick margin = {});
/// Ui.Head: a panel caption, a pink bar then spaced capitals (margin below 10).
void Head(std::string_view s, Thick margin = Thick(0, 0, 0, 10));
/// A run of a multi-part TextBlock (Inlines).
struct Run {
	std::string text;
	const char* brush = "Text";
	Face face = Face::Ui;
	Weight weight = Weight::Normal;
	float size = 0;   // 0 = the block's size
};
/// A TextBlock made of runs on one baseline (no wrap; trimmed to the width left).
void Runs(const std::vector<Run>& runs, float size, Thick margin = {}, float maxWDip = 0);
ImVec2 MeasureRuns(const std::vector<Run>& runs, float size);
/// Ui.SetValue: a big mono number with a small unit after it ("436 FPS").
void Value(std::string_view value, std::string_view unit, float size, const char* brush = "Strong", Thick margin = {});
/// Ui.Pair: a dim key, then the value (wrapped), margin MCell.
void Pair(std::string_view k, std::string_view v);
/// Ui.Kv: key left, mono value right, a line below.
void Kv(std::string_view k, std::string_view v);

// ---------------------------------------------------------------- tooltips
/// Shows a HUD tooltip for the last widget when it is hovered (WPF ToolTip, 400 ms delay).
void Tip(std::string_view text);
/// The window background ("Aurora": near-black with soft pink and violet glows, no grid).
void AuroraBackground(ImDrawList* dl, ImRect r);
/// The same for a rectangle that is not an ImGui item.
void TipRect(ImRect r, std::string_view text, ImGuiID id);

// ---------------------------------------------------------------- buttons
struct ButtonStyle {
	float minWidth = 0;          // DIP
	float width = 0;             // fixed width in DIP; -1 = stretch to the box
	float height = 30;           // DIP
	Thick padding = Thick(12, 0);
	Thick margin = Thick(0, 0, 6, 0);
	float fontSize = 0;          // em DIP, 0 = FsBody
	Weight weight = Weight::Normal;
	const char* bg = "Panel2";
	const char* fg = "Text";
	const char* line = "Line";
	bool enabled = true;
	const char* tip = nullptr;
};
/// The style of Ui.Btn (MinWidth 96, height 30, margin right 6).
ButtonStyle BtnStyle(float minWidth = 96.0f);
/// Ui.Primary: the pink filled button (on) or the plain one.
ButtonStyle& Primary(ButtonStyle& s, bool on = true);
/// A button with the Theme.cs template; returns true when clicked. The id is the label unless "##id" is used.
bool Button(std::string_view label, const ButtonStyle& s);
/// A button whose content the caller draws (icon + text); content(rect) is called with the inner rect.
bool ButtonCustom(const char* id, ImVec2 contentDip, const ButtonStyle& s, const std::function<void(ImRect inner, bool hovered)>& content);
/// Ui.Btn.
bool Btn(std::string_view label, float minWidth = 96.0f);
/// Ui.ConfirmBtn: the first click arms it ("Click again to confirm", amber) for 4 s; the second runs it.
bool ConfirmBtn(const char* id, std::string_view label, ButtonStyle s = BtnStyle());

// ---------------------------------------------------------------- small widgets
/// Ui.Pill.
void Pill(std::string_view text, bool accent, Thick margin = {});
/// Ui.Bar: a thin bar, `share` of it in Accent (or Steel), the rest in Line. width 0 = the box.
void Bar(float share, bool accent, float heightDip = 3.0f, Thick margin = {}, float widthDip = 0);
/// Ui.Input: a text box (Panel2, Strong text, AccentLine on hover, Accent with focus). Returns true when changed.
bool Input(const char* id, std::string& text, float widthDip, Thick margin = Thick(0, 0, 6, 0), int maxLen = 1024);
/// The plain slider of Theme.cs (2 px track, pink up to the value, 12 px square thumb). True when changed.
bool Slider(const char* id, double& value, double min, double max, float widthDip, double step = 0);
/// A separator line (1 DIP, Line).
void Separator(Thick margin = {});

// ---------------------------------------------------------------- processor map (Ui.LpMap)
struct LpSet {
	int lp = 0;
	int core = 0;
	int llc = 0;
	int group = 0;
};
struct TopoView {
	std::vector<LpSet> sets;
	std::vector<std::pair<int, uint64_t>> l3ByLlc;   // LLC id -> L3 bytes
	int bigCacheLlc = -1;
	bool hybrid = false;
	bool smt = false;
	int physical = 0;
	int logical = 0;
	bool valid = false;
};
struct LpLook {
	ImU32 fill = 0;
	ImU32 ring = 0;   // 0 = none
	std::string tip;
};
/// One block per L3 domain, one box per core, one cell per logical processor (stacked on SMT / hybrid).
/// Returns the lp clicked (-1 = none) when clickable.
int LpMap(const char* id, const TopoView& t, const std::function<LpLook(const LpSet&)>& look, bool clickable = false, float cell = 26.0f);

} // namespace ui
