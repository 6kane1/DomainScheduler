using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Shapes;
using System.Windows.Threading;

namespace DomainScheduler.App;

/// <summary>
/// The only place with colours, sizes and fonts (section 13 tokens). Everything else asks
/// Ui.B(key), GameC / NonGameC and the size tokens.
/// </summary>
static class Ui
{
    static readonly Dictionary<string, Color> Palette = new()
    {
        ["Bg"] = Color.FromRgb(0x1A, 0x1C, 0x1F), ["Panel"] = Color.FromRgb(0x22, 0x25, 0x28), ["Panel2"] = Color.FromRgb(0x2B, 0x2E, 0x32),
        ["Line"] = Color.FromRgb(0x3C, 0x40, 0x46), ["Hover"] = Color.FromRgb(0x35, 0x39, 0x3E), ["Sel"] = Color.FromRgb(0x40, 0x45, 0x4B),
        ["Text"] = Color.FromRgb(0xE6, 0xE7, 0xE9), ["Dim"] = Color.FromRgb(0x9A, 0x9F, 0xA6), ["Faint"] = Color.FromRgb(0x6C, 0x71, 0x78),
        ["On"] = Color.FromRgb(0x4F, 0xA8, 0x6A), ["Warn"] = Color.FromRgb(0xD9, 0xA4, 0x41), ["Error"] = Color.FromRgb(0xE0, 0x64, 0x5C),
        ["Game"] = Color.FromRgb(0x57, 0xB2, 0x6E), ["NonGame"] = Color.FromRgb(0x6F, 0x78, 0x84), ["Irq"] = Color.FromRgb(0xD9, 0xA4, 0x41),
        ["Overlay"] = Color.FromArgb(0xB4, 0x1A, 0x1C, 0x1F), ["None"] = Color.FromArgb(0, 0, 0, 0),
    };
    static readonly Dictionary<string, SolidColorBrush> Brushes = new();

    public static SolidColorBrush B(string key)
    {
        if (Brushes.TryGetValue(key, out var b)) return b;
        b = new SolidColorBrush(Palette.TryGetValue(key, out var c) ? c : Palette["Text"]);
        b.Freeze();
        return Brushes[key] = b;
    }

    public static Brush GameC => B("Game");
    public static Brush NonGameC => B("NonGame");

    /// <summary>Font sizes are given in points (8.5 / 8.75 / 10 / 48) and used in WPF units.</summary>
    static double Pt(double pt) => pt * 96.0 / 72.0;
    public static readonly double FsSmall = Pt(8.5), FsNote = Pt(8.75), FsBody = Pt(10), FsHero = Pt(48);
    public static readonly Thickness MTop = new(0, 15, 0, 0), MCell = new(0, 1, 6, 0), MRow = new(0, 0, 0, 6);
    public const double RowH = 28, Gap = 10, IconScale = 0.46;

    /// <summary>The palette as application resources: the control templates of Theme refer to them by key.</summary>
    public static void Register(ResourceDictionary res)
    {
        foreach (var key in Palette.Keys) res[key] = B(key);
    }

    public static FontFamily Font()
    {
        string f = L.Family;
        try { return f.Length > 0 ? new FontFamily(f) : SystemFonts.MessageFontFamily; }
        catch (Exception) { return SystemFonts.MessageFontFamily; }
    }

    public static string Hex(ulong m) => "0x" + m.ToString("X", CultureInfo.InvariantCulture);

    public static string Size(long bytes) =>
        bytes >= 1L << 30 ? (bytes / (double)(1L << 30)).ToString("0.0", CultureInfo.CurrentCulture) + " GB"
        : bytes >= 1L << 20 ? (bytes / (double)(1L << 20)).ToString("0.0", CultureInfo.CurrentCulture) + " MB"
        : (bytes / 1024.0).ToString("0", CultureInfo.CurrentCulture) + " KB";

    public static TextBlock Text(string s, double size = 0, string brush = "Text", bool bold = false, bool wrap = false) => new()
    {
        Text = s ?? "", FontSize = size > 0 ? size : FsBody, Foreground = B(brush), FontWeight = bold ? FontWeights.SemiBold : FontWeights.Normal,
        TextWrapping = wrap ? TextWrapping.Wrap : TextWrapping.NoWrap, TextTrimming = wrap ? TextTrimming.None : TextTrimming.CharacterEllipsis,
        VerticalAlignment = VerticalAlignment.Center,
    };

    public static TextBlock Head(string s) => new()
    {
        Text = s, FontSize = FsBody, FontWeight = FontWeights.SemiBold, Foreground = B("Text"), Margin = MRow,
    };

    public static TextBlock Note(string s) => Text(s, FsNote, "Dim", wrap: true);

    public static Button Btn(string text, Action click, double minWidth = 96)
    {
        var b = new Button { Content = text, MinWidth = minWidth, Height = RowH, Margin = new Thickness(0, 0, 6, 0), FontSize = FsBody };
        b.Click += (_, _) => click?.Invoke();
        return b;
    }

    /// <summary>A button that asks for a second click instead of a dialog.</summary>
    public static Button ConfirmBtn(string text, Action act)
    {
        var b = new Button { Content = text, MinWidth = 96, Height = RowH, Margin = new Thickness(0, 0, 6, 0), FontSize = FsBody };
        bool armed = false;
        var timer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(4) };
        void Disarm()
        {
            timer.Stop();
            armed = false;
            b.Content = text;
            b.Foreground = B("Text");
        }
        timer.Tick += (_, _) => Disarm();
        b.Click += (_, _) =>
        {
            if (!armed)
            {
                armed = true;
                b.Content = L.T("Click again to confirm");
                b.Foreground = B("Warn");
                timer.Start();
                return;
            }
            Disarm();
            act?.Invoke();
        };
        b.Unloaded += (_, _) => timer.Stop();
        return b;
    }

    public static Border Card(UIElement child, double pad = 12) => new()
    {
        Background = B("Panel"), BorderBrush = B("Line"), BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(6),
        Padding = new Thickness(pad), Margin = new Thickness(0, 0, 0, Gap), Child = child,
    };

    public static StackPanel Row(params UIElement[] items)
    {
        var p = new StackPanel { Orientation = Orientation.Horizontal, Margin = MRow };
        foreach (var i in items) if (i != null) p.Children.Add(i);
        return p;
    }

    public static StackPanel Col(params UIElement[] items)
    {
        var p = new StackPanel();
        foreach (var i in items) if (i != null) p.Children.Add(i);
        return p;
    }

    public static ScrollViewer Scroll(UIElement child) => new()
    {
        Content = child, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
        Focusable = false,
    };

    /// <summary>Key and value side by side.</summary>
    public static UIElement Pair(string k, string v)
    {
        var g = new DockPanel { Margin = MCell };
        var key = Text(k, FsNote, "Dim");
        key.Margin = new Thickness(0, 0, 8, 0);
        DockPanel.SetDock(key, Dock.Left);
        g.Children.Add(key);
        g.Children.Add(Text(v, FsBody, wrap: true));
        return g;
    }

    public static TextBox Input(string text, double width, Action<string> changed)
    {
        var t = new TextBox
        {
            Text = text ?? "", Width = width, Height = RowH, Background = B("Panel2"), Foreground = B("Text"), BorderBrush = B("Line"),
            CaretBrush = B("Text"), SelectionBrush = B("Dim"), VerticalContentAlignment = VerticalAlignment.Center, Padding = new Thickness(6, 0, 6, 0),
            FontSize = FsBody, Margin = new Thickness(0, 0, 6, 0),
        };
        t.TextChanged += (_, _) => changed?.Invoke(t.Text);
        return t;
    }

    /// <summary>A map of the logical processors: a row per L3 domain, a box per core, a cell per processor.</summary>
    public static UIElement LpMap(Topo t, Func<CpuSet, (Brush Fill, Brush Ring, string Tip)> look, Action<CpuSet> click = null, double cell = 26)
    {
        var tpl = LayoutTemplate.For(t);
        var root = new StackPanel();
        foreach (var llc in t.Sets.Where(s => s.Group == 0).GroupBy(s => s.LLC).OrderBy(g => g.Key))
        {
            ulong size = t.L3ByLlc.TryGetValue(llc.Key, out var sz) ? sz : 0;
            string label = L.F("L3 domain {0}", llc.Key) + (size > 0 ? " · " + (size >> 20).ToString(CultureInfo.InvariantCulture) + " MB" : "") +
                           (t.BigCacheLlc == llc.Key ? " · " + L.T("big cache") : "");
            root.Children.Add(Text(label, FsSmall, "Dim"));
            var wrap = new WrapPanel { Margin = new Thickness(0, 3, 0, 8) };
            foreach (var core in llc.GroupBy(s => s.Core).OrderBy(g => g.Key))
            {
                var box = new StackPanel { Orientation = tpl.Stacked ? Orientation.Vertical : Orientation.Horizontal };
                foreach (var lp in core.OrderBy(s => s.LP))
                {
                    var (fill, ring, tip) = look(lp);
                    var c = new Border
                    {
                        Width = cell, Height = cell, Margin = new Thickness(1), CornerRadius = new CornerRadius(3), Background = fill,
                        BorderBrush = ring ?? B("None"), BorderThickness = new Thickness(2), ToolTip = string.IsNullOrEmpty(tip) ? null : tip,
                        Child = new TextBlock
                        {
                            Text = lp.LP.ToString(CultureInfo.InvariantCulture), FontSize = FsSmall, Foreground = B("Text"),
                            HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
                        },
                    };
                    if (click != null)
                    {
                        var captured = lp;
                        c.Cursor = Cursors.Hand;
                        c.MouseLeftButtonUp += (_, _) => click(captured);
                    }
                    box.Children.Add(c);
                }
                wrap.Children.Add(new Border
                {
                    BorderBrush = B("Line"), BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(4), Padding = new Thickness(1),
                    Margin = new Thickness(0, 0, 4, 4), Child = box,
                });
            }
            root.Children.Add(wrap);
        }
        return root;
    }
}

/// <summary>How the processors of one core are laid out in a map.</summary>
sealed class LayoutTemplate
{
    public string Key = "flat";
    public bool Stacked;

    public static LayoutTemplate For(Topo t) =>
        t == null ? new LayoutTemplate()
        : t.Hybrid ? new LayoutTemplate { Key = "hybrid", Stacked = true }
        : t.SMT ? new LayoutTemplate { Key = "smt", Stacked = true }
        : new LayoutTemplate { Key = "flat" };
}

/// <summary>A two-column grid of facts.</summary>
sealed class FactGrid : UniformGrid
{
    public FactGrid(int columns = 2) => Columns = columns;

    public FactGrid Add(string k, string v)
    {
        Children.Add(Ui.Pair(k, v));
        return this;
    }
}

/// <summary>A switch with a label and a hint line.</summary>
sealed class CheckRow : Border
{
    bool _checked;
    readonly TextBlock _hint;
    readonly Border _box;
    readonly TextBlock _mark;
    public event Action<bool> Changed;

    public CheckRow(string label, string hint, bool value)
    {
        _checked = value;
        _box = new Border
        {
            Width = 18, Height = 18, CornerRadius = new CornerRadius(4), BorderThickness = new Thickness(1), BorderBrush = Ui.B("Line"),
            Margin = new Thickness(0, 1, 9, 0), VerticalAlignment = VerticalAlignment.Top,
        };
        _mark = new TextBlock { Text = "✓", FontSize = Ui.FsBody, Foreground = Ui.B("Bg"), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        _box.Child = _mark;
        _hint = Ui.Note(hint);
        _hint.Visibility = string.IsNullOrEmpty(hint) ? Visibility.Collapsed : Visibility.Visible;
        var text = Ui.Col(Ui.Text(label, wrap: true), _hint);
        var dock = new DockPanel();
        DockPanel.SetDock(_box, Dock.Left);
        dock.Children.Add(_box);
        dock.Children.Add(text);
        Child = dock;
        Background = Ui.B("None");
        Margin = Ui.MRow;
        Cursor = Cursors.Hand;
        MouseLeftButtonUp += (_, _) =>
        {
            if (!IsEnabled) return;
            _checked = !_checked;
            Paint();
            Changed?.Invoke(_checked);
        };
        IsEnabledChanged += (_, _) => Opacity = IsEnabled ? 1 : 0.5;
        Paint();
    }

    public bool Checked => _checked;

    /// <summary>Sets the value without raising Changed (programmatic load).</summary>
    public void Set(bool value)
    {
        _checked = value;
        Paint();
    }

    public void Hint(string text)
    {
        _hint.Text = text ?? "";
        _hint.Visibility = string.IsNullOrEmpty(text) ? Visibility.Collapsed : Visibility.Visible;
    }

    void Paint()
    {
        _box.Background = _checked ? Ui.B("On") : Ui.B("Panel2");
        _mark.Visibility = _checked ? Visibility.Visible : Visibility.Hidden;
    }
}

sealed class Opt
{
    public string Key = "", Label = "";
    public Opt(string key, string label)
    {
        Key = key;
        Label = label;
    }
}

static class Combo
{
    public static ComboBox New(double width, params Opt[] opts)
    {
        var cb = new ComboBox { Width = width, Height = Ui.RowH, FontSize = Ui.FsBody, Margin = new Thickness(0, 0, 6, 0) };
        foreach (var o in opts) cb.Items.Add(new ComboBoxItem { Content = o.Label, Tag = o.Key });
        if (cb.Items.Count > 0) cb.SelectedIndex = 0;
        return cb;
    }

    /// <summary>Selects by key; an unknown key selects nothing new and returns false.</summary>
    public static bool Select(ComboBox cb, string key)
    {
        for (int i = 0; i < cb.Items.Count; i++)
            if (cb.Items[i] is ComboBoxItem it && string.Equals(it.Tag as string, key, StringComparison.OrdinalIgnoreCase))
            {
                cb.SelectedIndex = i;
                return true;
            }
        return false;
    }

    public static string Key(ComboBox cb) => (cb.SelectedItem as ComboBoxItem)?.Tag as string ?? "";

    public static void OnChange(ComboBox cb, Action<string> changed) => cb.SelectionChanged += (_, _) => changed(Key(cb));
}

/// <summary>A legend entry: colour, look ("fill" or "ring") and text.</summary>
sealed class Leg
{
    public Brush C;
    public string Look = "fill", Text = "";

    public UIElement Build()
    {
        var sw = new Border
        {
            Width = 12, Height = 12, CornerRadius = new CornerRadius(2), Margin = new Thickness(0, 0, 5, 0), VerticalAlignment = VerticalAlignment.Center,
            Background = Look == "fill" ? C : Ui.B("None"), BorderBrush = C, BorderThickness = new Thickness(2),
        };
        var p = new StackPanel { Orientation = Orientation.Horizontal, Margin = new Thickness(0, 0, 14, 0) };
        p.Children.Add(sw);
        p.Children.Add(Ui.Text(Text, Ui.FsNote, "Dim"));
        return p;
    }

    public static UIElement Line(params Leg[] legs)
    {
        var p = new WrapPanel { Margin = Ui.MRow };
        foreach (var l in legs) p.Children.Add(l.Build());
        return p;
    }
}

/// <summary>A plain table: a header, rows of cells, one selected row.</summary>
sealed class Table
{
    readonly (string Head, double Width)[] _cols;      // Width: > 0 pixels, 0 = the rest
    readonly StackPanel _rows = new();
    readonly List<(object Tag, Border Row)> _items = new();
    public readonly DockPanel Root = new();
    public object Selected { get; private set; }
    public event Action SelectionChanged;

    public Table(params (string Head, double Width)[] cols)
    {
        _cols = cols;
        var head = MakeRow(cols.Select(c => (UIElement)Ui.Text(c.Head, Ui.FsSmall, "Dim")).ToArray());
        var hb = new Border { Child = head, BorderBrush = Ui.B("Line"), BorderThickness = new Thickness(0, 0, 0, 1), Padding = new Thickness(6, 0, 14, 4) };
        DockPanel.SetDock(hb, Dock.Top);
        Root.Children.Add(hb);
        Root.Children.Add(Ui.Scroll(_rows));
    }

    Grid MakeRow(UIElement[] cells)
    {
        var g = new Grid();
        for (int i = 0; i < _cols.Length; i++)
        {
            g.ColumnDefinitions.Add(new ColumnDefinition { Width = _cols[i].Width > 0 ? new GridLength(_cols[i].Width) : new GridLength(1, GridUnitType.Star) });
            if (i >= cells.Length || cells[i] == null) continue;
            if (cells[i] is FrameworkElement fe && fe.Margin == default) fe.Margin = new Thickness(0, 0, 8, 0);
            Grid.SetColumn(cells[i], i);
            g.Children.Add(cells[i]);
        }
        return g;
    }

    /// <summary>Replaces the rows; the selection is dropped (a rebuilt list has no selected row).</summary>
    public void SetRows(IEnumerable<(object Tag, UIElement[] Cells)> rows)
    {
        _rows.Children.Clear();
        _items.Clear();
        Selected = null;
        foreach (var (tag, cells) in rows)
        {
            var b = new Border { Child = MakeRow(cells), Padding = new Thickness(6, 3, 6, 3), Background = Ui.B("None"), CornerRadius = new CornerRadius(3), MinHeight = 24 };
            object captured = tag;
            b.MouseLeftButtonUp += (_, _) => Select(captured);
            b.MouseEnter += (_, _) => { if (!Equals(Selected, captured)) b.Background = Ui.B("Hover"); };
            b.MouseLeave += (_, _) => { if (!Equals(Selected, captured)) b.Background = Ui.B("None"); };
            _items.Add((tag, b));
            _rows.Children.Add(b);
        }
    }

    public void Select(object tag, bool raise = true)
    {
        Selected = tag;
        foreach (var (t, row) in _items) row.Background = Equals(t, tag) ? Ui.B("Sel") : Ui.B("None");
        if (raise) SelectionChanged?.Invoke();
    }

    public int Count => _items.Count;
}
