// CheckRow, Combo + Opt, Leg, FactGrid, Table, LayoutTemplate (ports of the C# files of the same names) and the
// topology view the processor maps draw from. Owner: app shell; used by the pages.
#pragma once
#include <functional>
#include <string>
#include <string_view>
#include <vector>

#include "json.h"
#include "ui.h"

namespace ui {

// ---------------------------------------------------------------- CheckRow.cs
/// A check box row: 16 px box (pink with a check mark when on), the label (wrapped) and a hint below it.
/// Clicking anywhere on the row flips `value`; returns true when it was flipped (the Changed event).
/// Disabled rows are drawn at 0.5 opacity and do not react.
bool CheckRow(const char* id, std::string_view label, std::string_view hint, bool& value, bool enabled = true);

// ---------------------------------------------------------------- Opt.cs + Combo.cs
struct Opt {
	std::string Key;
	std::string Label;
	Opt() = default;
	Opt(std::string key, std::string label) : Key(std::move(key)), Label(std::move(label)) {}
};

/// Combo.New(width, opts): height 32, FsBody, margin right 6; the first option selected.
class Combo {
public:
	Combo() = default;
	Combo(float widthDip, std::vector<Opt> opts);
	void SetItems(std::vector<Opt> opts);
	/// Combo.Select: selects the option whose key matches (ignoring case); false when there is none.
	bool Select(std::string_view key);
	/// Combo.Key: the selected option's key ("" when none).
	std::string Key() const;
	int Index() const { return sel_; }
	void SetIndex(int i) { sel_ = (i >= 0 && i < (int)opts_.size()) ? i : -1; }
	const std::vector<Opt>& Items() const { return opts_; }
	float width = 160;
	bool enabled = true;
	Thick margin = Thick(0, 0, 6, 0);
	const char* tip = nullptr;
	/// Draws it; true when the user picked another option (Combo.OnChange; then Key() is the new key).
	bool Draw(const char* id);

private:
	std::vector<Opt> opts_;
	int sel_ = -1;
	int hot_ = -1;
};

// ---------------------------------------------------------------- Leg.cs
struct Leg {
	ImU32 C = 0;
	std::string Look = "fill";   // "fill" = a filled square, else only the 2 px outline
	std::string Text;
};
/// Leg.Line: the legend items in a WrapPanel (margin MRow).
void LegLine(const std::vector<Leg>& legs);

// ---------------------------------------------------------------- FactGrid.cs
/// A UniformGrid of Ui.Pair cells (every cell the size of the largest one).
class FactGrid {
public:
	explicit FactGrid(int columns = 2) : cols_(columns < 1 ? 1 : columns) {}
	FactGrid& Add(std::string k, std::string v)
	{
		items_.emplace_back(std::move(k), std::move(v));
		return *this;
	}
	void Clear() { items_.clear(); }
	void Draw();

private:
	int cols_;
	std::vector<std::pair<std::string, std::string>> items_;
};

// ---------------------------------------------------------------- Table.cs
struct Cell {
	std::string text;
	const char* brush = "Text";
	Face face = Face::Ui;
	Weight weight = Weight::Normal;
	float size = 0;                    // em DIP, 0 = FsBody
	bool wrap = false;
	float heightDip = 0;               // a custom cell's height
	std::function<void(ImRect)> draw;  // a custom cell (icon, pill, bar ...) drawn into its rect
	std::string tip;
	static Cell T(std::string s, const char* brush = "Text") { Cell c; c.text = std::move(s); c.brush = brush; return c; }
	static Cell Mono(std::string s, const char* brush = "Text") { Cell c; c.text = std::move(s); c.brush = brush; c.face = Face::Mono; return c; }
};
struct TableRow {
	std::string tag;
	std::vector<Cell> cells;
};

/// Table.cs: a caption row (spaced capitals, a line below) over a scrolling list of rows; a click selects a row
/// (Sel), hover tints it (Hover). Columns are (head, width DIP); width 0 = a star column.
class Table {
public:
	Table() = default;
	explicit Table(std::vector<std::pair<std::string, float>> cols) : cols_(std::move(cols)) {}
	void SetColumns(std::vector<std::pair<std::string, float>> cols) { cols_ = std::move(cols); }
	/// SetRows: replaces the rows, clears the selection.
	void SetRows(std::vector<TableRow> rows);
	/// Select(tag, raise): marks the row; raise = call SelectionChanged.
	void Select(const std::string& tag, bool raise = true);
	const std::string& Selected() const { return selected_; }
	bool HasSelection() const { return hasSel_; }
	int Count() const { return (int)rows_.size(); }
	std::function<void()> SelectionChanged;
	/// Draws the table into `sizePx` (0 = the rest of the box / window, like the DockPanel filling its parent).
	void Draw(const char* id, ImVec2 sizePx = ImVec2(0, 0));

private:
	std::vector<std::pair<std::string, float>> cols_;
	std::vector<TableRow> rows_;
	std::string selected_;
	bool hasSel_ = false;
	std::vector<float> Widths(float total) const;
};

// ---------------------------------------------------------------- LayoutTemplate.cs
struct LayoutTemplate {
	std::string Key = "flat";
	bool Stacked = false;
	static LayoutTemplate For(const TopoView* t);
};

/// The topology the processor maps draw, from the core's JSON:
/// {"sets":[{"lp":0,"core":0,"llc":0,"group":0}...], "l3_by_llc":{"0":33554432}, "big_cache_llc":-1,
///  "hybrid":false, "smt":true, "physical":8, "logical":16}  (null / missing = no topology).
TopoView ParseTopo(const json::Value& v);

} // namespace ui
