using System.Globalization;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>The cache cleaner: Scan first, then Clean. Closed while Auto is on.</summary>
sealed class CleanerPage : Page
{
    List<CleanItem> _items = new();
    Table _table;
    StackPanel _info;
    CleanItem _sel;

    public CleanerPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Cleaner");
    public override bool ClosedInAuto => true;

    public override FrameworkElement Build()
    {
        _items = Cleaner.Items();
        _sel = null;
        _table = new Table((L.T("Cache"), 0), (L.T("Size"), 86), (L.T("Files"), 66), (L.T("Oldest"), 92), (L.T("Newest"), 92), (L.T("Note"), 240));
        _table.SelectionChanged += () =>
        {
            _sel = _table.Selected as CleanItem;
            DrawInfo();
        };
        _info = new StackPanel();
        var bar = Ui.Row(Ui.Btn(L.T("Scan"), Scan));
        int limit = Cleaner.SpotifyLimit();
        var foot = Ui.Card(Ui.Col(_info, Ui.Note(L.F("Spotify's own cache limit: {0}", limit > 0 ? limit + " MB" : L.T("not set")))));
        foot.Margin = new Thickness(0, Ui.Gap, 0, Ui.Gap);
        var dock = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        DockPanel.SetDock(foot, Dock.Bottom);
        dock.Children.Add(bar);
        dock.Children.Add(foot);
        dock.Children.Add(_table.Root);
        Fill();
        DrawInfo();
        return dock;
    }

    static string Day(DateTime d) => d == DateTime.MaxValue || d == DateTime.MinValue ? "" : d.ToLocalTime().ToString("d", CultureInfo.CurrentCulture);

    void Fill()
    {
        _table.SetRows(_items.Select(i => ((object)i, new UIElement[]
        {
            Ui.Text(i.Name + (i.Running ? "  · " + L.T("running") : ""), brush: i.Running ? "Dim" : "Text"),
            Ui.Text(i.Scanned ? Ui.Size(i.Size) : "", Ui.FsNote, "Dim"),
            Ui.Text(i.Scanned ? i.Files.ToString(CultureInfo.CurrentCulture) : "", Ui.FsNote, "Dim"),
            Ui.Text(i.Scanned ? Day(i.Oldest) : "", Ui.FsNote, "Dim"),
            Ui.Text(i.Scanned ? Day(i.Newest) : "", Ui.FsNote, "Dim"),
            Ui.Text(i.Scanned ? i.ScanNote : L.T("not scanned"), Ui.FsNote, "Dim"),
        })));
        _sel = null;
    }

    void Scan() => S.BusyTask(L.T("Scanning…"), () =>
    {
        foreach (var i in _items) Cleaner.Scan(i);
        return _items.Sum(i => i.Size);
    }, total =>
    {
        Fill();
        DrawInfo();
        S.Did(L.F("Scan: {0} can be freed", Ui.Size(total)));
    });

    void DrawInfo()
    {
        _info.Children.Clear();
        if (_sel == null)
        {
            _info.Children.Add(Ui.Note(L.T("Press Scan, select a cache, then Clean. Nothing is deleted without a scan; caches of running programs are skipped.")));
            return;
        }
        var item = _sel;
        _info.Children.Add(Ui.Head(item.Name));
        foreach (var r in item.Roots) _info.Children.Add(Ui.Note(r));
        if (item.AskFirst) _info.Children.Add(Ui.Note(L.T("This cache is rebuilt on the next start of the game or program: the first run after cleaning is slower.")));
        var clean = Ui.ConfirmBtn(L.T("Clean"), () => S.BusyTask(L.T("Cleaning…"), () => Cleaner.Clean(item, consent: true), res =>
        {
            S.Did(L.F("{0}: {1} files, {2} freed", item.Name, res.Files, Ui.Size(res.Bytes)) + (res.Note.Length > 0 ? " · " + res.Note : ""));
            Fill();
            DrawInfo();
        }));
        clean.IsEnabled = item.Scanned && item.Files > 0 && !item.Running;
        _info.Children.Add(Ui.Row(clean));
    }
}
