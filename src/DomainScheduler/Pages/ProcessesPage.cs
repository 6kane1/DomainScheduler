using System.Collections.Concurrent;
using System.Drawing;
using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace DomainScheduler.App;

/// <summary>Program icons: read once per file off the UI thread, frozen, the GDI icon disposed.</summary>
static class IconCache
{
    static readonly ConcurrentDictionary<string, ImageSource> Cache = new(StringComparer.OrdinalIgnoreCase);

    public static void Load(string path, int pixels, System.Windows.Controls.Image target)
    {
        if (string.IsNullOrEmpty(path)) return;
        if (Cache.TryGetValue(path, out var hit))
        {
            target.Source = hit;
            return;
        }
        Task.Run(() =>
        {
            ImageSource src = null;
            try
            {
                using var icon = Icon.ExtractAssociatedIcon(path);
                if (icon != null)
                {
                    var bmp = Imaging.CreateBitmapSourceFromHIcon(icon.Handle, Int32Rect.Empty, BitmapSizeOptions.FromWidthAndHeight(pixels, pixels));
                    bmp.Freeze();
                    src = bmp;
                }
            }
            catch (Exception) { }
            Cache[path] = src;
            if (src != null) target.Dispatcher.BeginInvoke(() => target.Source = src);
        });
    }
}

/// <summary>Processes: who is on the watchlist, what affinity they have, a per-processor mask editor.</summary>
sealed class ProcessesPage : Page
{
    sealed class Row
    {
        public uint Pid;
        public string Exe = "", Path = "";
        public Engine.ProcInfo Info = new();
        public bool Watched;
    }

    Table _table;
    StackPanel _info;
    Button _apply, _reset;
    List<Row> _rows = new();
    Row _sel;
    ulong _edit, _loaded;
    bool _dirty;
    Topo _topo;

    public ProcessesPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Processes");

    /// <summary>The process the Threads page opens with.</summary>
    public (uint Pid, string Exe) Picked => _sel == null ? (0u, "") : (_sel.Pid, _sel.Exe);

    public override FrameworkElement Build()
    {
        _topo = TopoBuilder.Live();
        _table = new Table(("", 24), (L.T("Process"), 0), (L.T("PID"), 62), (L.T("Class"), 96), (L.T("Affinity"), 110), (L.T("Processors"), 96), (L.T("Watch"), 58));
        _table.SelectionChanged += () =>
        {
            _sel = _table.Selected as Row;
            LoadEdit();
            BuildInfo();
        };
        _info = new StackPanel();
        _apply = Ui.Btn(L.T("Apply"), Apply);
        _reset = Ui.Btn(L.T("Reset"), Reset);
        var bar = Ui.Row(Ui.Btn(L.T("Refresh"), Refresh), _apply, _reset);
        var grid = new Grid();
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(318) });
        var side = Ui.Card(Ui.Scroll(_info));
        side.Margin = new Thickness(Ui.Gap, 0, 0, Ui.Gap);
        Grid.SetColumn(side, 1);
        grid.Children.Add(_table.Root);
        grid.Children.Add(side);
        var dock = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        dock.Children.Add(bar);
        dock.Children.Add(grid);
        _sel = null;
        _dirty = false;
        BuildInfo();
        return dock;
    }

    public override void Shown() => Refresh();

    static string ClassText(uint cls) => cls switch
    {
        64 => L.T("Idle"), 16384 => L.T("Below normal"), 32 => L.T("Normal"), 32768 => L.T("Above normal"), 128 => L.T("High"), 256 => L.T("Realtime"), _ => "",
    };

    void Refresh() => S.Busy(L.T("Reading processes…"), () =>
    {
        var list = new List<Row>();
        foreach (var (pid, p) in H.Engine.ProcessesNow())
        {
            if (pid is 0 or 4) continue;
            var r = new Row { Pid = pid, Exe = p.Name, Watched = H.Watch.Contains(p.Name) };
            r.Info = H.Engine.ReadProcess(pid, p.Name);
            if (!r.Info.Denied) r.Path = Native.ImagePath(pid);
            list.Add(r);
        }
        return list.OrderByDescending(r => r.Watched).ThenBy(r => r.Exe, StringComparer.OrdinalIgnoreCase).ThenBy(r => r.Pid).ToList();
    }, list =>
    {
        _rows = list ?? new();
        int px = (int)Math.Round(32 * Ui.IconScale * S.DpiScaleX);
        _table.SetRows(_rows.Select(r =>
        {
            var img = new System.Windows.Controls.Image { Width = 32 * Ui.IconScale, Height = 32 * Ui.IconScale };
            IconCache.Load(r.Path, Math.Max(16, px), img);
            string brush = r.Info.Denied ? "Faint" : r.Watched ? "Text" : "Dim";
            return ((object)r, new UIElement[]
            {
                img,
                Ui.Text(r.Exe + (r.Info.Changed ? "  •" : ""), brush: brush),
                Ui.Text(r.Pid.ToString(CultureInfo.InvariantCulture), Ui.FsNote, "Dim"),
                Ui.Text(r.Info.Denied ? L.T("denylist") : r.Info.Ok ? ClassText(r.Info.Class) : "", Ui.FsNote, "Dim"),
                Ui.Text(r.Info.Ok ? Ui.Hex(r.Info.Affinity) : "", Ui.FsNote, "Dim"),
                Ui.Text(r.Info.Ok ? Masks.Ranges(r.Info.Affinity) : "", Ui.FsNote, "Dim"),
                Ui.Text(r.Watched ? "✓" : "", brush: "On"),
            });
        }));
        _sel = null;          // a rebuilt list has no selected row
        _dirty = false;
        BuildInfo();
    });

    void LoadEdit()
    {
        _loaded = _sel != null && _sel.Info.Ok && _topo != null ? _sel.Info.Affinity & _topo.FullMask() : 0;
        _edit = _loaded;
        _dirty = false;
    }

    void BuildInfo()
    {
        _info.Children.Clear();
        _apply.IsEnabled = false;
        _reset.IsEnabled = false;
        if (_sel == null)
        {
            _info.Children.Add(Ui.Note(L.T("Select a process. Only processes on the watchlist are changed; denylisted ones are never opened.")));
            return;
        }
        var r = _sel;
        _info.Children.Add(Ui.Head(r.Exe));
        _info.Children.Add(Ui.Pair(L.T("PID"), r.Pid.ToString(CultureInfo.InvariantCulture)));
        if (r.Info.Denied)
        {
            _info.Children.Add(Ui.Note(L.T("On the denylist (system-critical or integrity-sensitive): never opened, never changed.")));
            return;
        }
        var watch = new CheckRow(L.T("On the watchlist"), L.T("The program changes scheduling only for watched processes."), r.Watched);
        watch.Changed += on =>
        {
            H.SetWatched(r.Exe, on);
            foreach (var x in _rows.Where(x => x.Exe.Equals(r.Exe, StringComparison.OrdinalIgnoreCase))) x.Watched = on;
            BuildInfo();
            S.Refresh();
        };
        _info.Children.Add(watch);
        if (!r.Info.Ok || _topo == null)
        {
            _info.Children.Add(Ui.Note(L.T("The process cannot be read (no access).")));
            return;
        }
        var split = _topo.Split;
        _info.Children.Add(Ui.Pair(L.T("Affinity"), Ui.Hex(_edit) + "  " + Masks.Ranges(_edit)));
        if (split.CanSplit)
            _info.Children.Add(Ui.Pair(L.T("Check"), (_edit & ~split.Game) == 0 ? L.T("only game cores") : L.T("also non-game cores")));
        _info.Children.Add(Ui.LpMap(_topo, lp =>
        {
            bool on = lp.LP < 64 && ((_edit >> lp.LP) & 1) != 0;
            var fill = !on ? Ui.B("Panel2") : split.CanSplit && ((split.Game >> lp.LP) & 1) == 0 ? Ui.NonGameC : Ui.GameC;
            return (fill, null, null);
        }, r.Watched ? lp =>
        {
            ulong next = _edit ^ (1UL << lp.LP);
            if (next == 0) return;          // an empty mask is not a mask
            _edit = next;
            _dirty = _edit != _loaded;
            BuildInfo();
        } : null, 24));
        if (split.CanSplit)
            _info.Children.Add(Leg.Line(new Leg { C = Ui.GameC, Text = L.T("game cores") }, new Leg { C = Ui.NonGameC, Text = L.T("non-game cores") }));
        _info.Children.Add(Ui.Note(r.Watched
            ? L.T("Click processors to edit the mask, then Apply. The mask is saved as this program's rule and kept while it runs.")
            : L.T("Put the process on the watchlist to edit its mask.")));
        string why = H.Eligibility(r.Exe);
        _apply.IsEnabled = _dirty && why == null && !S.AutoOn;
        _reset.IsEnabled = r.Watched && !S.AutoOn;
        if (S.AutoOn) _info.Children.Add(Ui.Note(L.T("Auto is on: masks are set by Auto.")));
    }

    void Apply()
    {
        if (_sel == null || !_dirty) return;
        var r = _sel;
        ulong mask = _edit;
        Settings.SaveAffRule(r.Exe, Ui.Hex(mask));
        H.Configure();
        H.Paused = false;
        H.Opt.Run(() => H.Sys.Pass(H.Auto.GamePid), _ =>
        {
            S.Did(L.F("{0}: mask {1} ({2}) saved and applied", r.Exe, Ui.Hex(mask), Masks.Ranges(mask)));
            Refresh();
        });
    }

    void Reset()
    {
        if (_sel == null) return;
        var r = _sel;
        Settings.SaveAffRule(r.Exe, "");
        H.Configure();
        H.Opt.Run(() => H.Engine.Revert(r.Pid), n =>
        {
            S.Did(L.F("{0}: rule removed, {1} values put back", r.Exe, n));
            Refresh();
        });
    }
}
