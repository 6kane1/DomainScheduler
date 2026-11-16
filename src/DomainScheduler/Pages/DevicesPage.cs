using System.Globalization;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>Devices and their interrupts: read-only facts; the optional policy write only by hand, outside Auto.</summary>
sealed class DevicesPage : Page
{
    sealed class Row
    {
        public PciDevice Dev;
        public IrqConfig Cfg = new();
    }

    Table _table;
    StackPanel _info;
    List<KeyValuePair<string, ulong>> _pins = new();
    Row _sel;
    Topo _topo;

    public DevicesPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Devices");

    public override FrameworkElement Build()
    {
        _topo = TopoBuilder.Live();
        _sel = null;
        _table = new Table((L.T("Device"), 0), (L.T("MSI"), 46), (L.T("Pinned to"), 90), (L.T("Priority"), 70));
        _table.SelectionChanged += () =>
        {
            _sel = _table.Selected as Row;
            LoadFacts();
        };
        _info = new StackPanel();
        var bar = Ui.Row(Ui.Btn(L.T("Refresh"), Refresh));
        var grid = new Grid();
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(380) });
        var side = Ui.Card(Ui.Scroll(_info));
        side.Margin = new Thickness(Ui.Gap, 0, 0, Ui.Gap);
        Grid.SetColumn(side, 1);
        grid.Children.Add(_table.Root);
        grid.Children.Add(side);
        var dock = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        dock.Children.Add(bar);
        dock.Children.Add(grid);
        _info.Children.Add(Ui.Note(L.T("Select a device. Interrupt pins are only read here; the game mask steps around dedicated ones.")));
        return dock;
    }

    public override void Shown() => Refresh();

    static string PrioText(int? p) => p switch { 1 => L.T("low"), 2 => L.T("normal"), 3 => L.T("high"), _ => "" };

    void Refresh() => S.BusyTask(L.T("Reading devices…"), () =>
    {
        var rows = Devices.List().Select(d => new Row { Dev = d, Cfg = Devices.ReadIrq(d.Pdo) }).OrderBy(r => r.Dev.Name, StringComparer.OrdinalIgnoreCase).ToList();
        var pins = rows.Where(r => (r.Cfg.Assign ?? 0) != 0).Select(r => new KeyValuePair<string, ulong>(r.Dev.Name, r.Cfg.Assign.Value)).ToList();
        return (rows, pins);
    }, res =>
    {
        _pins = res.pins ?? new();
        _sel = null;
        _table.SetRows((res.rows ?? new()).Select(r => ((object)r, new UIElement[]
        {
            Ui.Text(r.Dev.Name),
            Ui.Text(r.Cfg.Msi ? "✓" : "", brush: "On"),
            Ui.Text((r.Cfg.Assign ?? 0) != 0 ? Masks.Ranges(r.Cfg.Assign.Value) : "", Ui.FsNote, "Dim"),
            Ui.Text(PrioText(r.Cfg.Priority), Ui.FsNote, "Dim"),
        })));
    });

    void LoadFacts()
    {
        if (_sel == null) return;
        var row = _sel;
        S.BusyTask(L.T("Reading the device…"), () => Devices.Facts(row.Dev, _pins), f =>
        {
            if (f != null && ReferenceEquals(row, _sel)) Draw(row, f);
        });
    }

    void Draw(Row row, DeviceFacts f)
    {
        _info.Children.Clear();
        _info.Children.Add(Ui.Head(row.Dev.Name));
        var facts = new FactGrid(1).Add(L.T("Driver"), f.DriverFile).Add(L.T("Location"), row.Dev.Location)
            .Add(L.T("Windows started"), f.Boot == DateTime.MinValue ? "" : f.Boot.ToLocalTime().ToString("g", CultureInfo.CurrentCulture))
            .Add(L.T("Restarted by the program"), f.Restarted == DateTime.MinValue ? L.T("never") : f.Restarted.ToLocalTime().ToString("g", CultureInfo.CurrentCulture));
        _info.Children.Add(facts);
        _info.Children.Add(Ui.Head(L.T("Interrupts")));
        if (f.Resources.Count == 0) _info.Children.Add(Ui.Note(L.T("No interrupt resources.")));
        foreach (var r in f.Resources)
            _info.Children.Add(Ui.Note(L.F("Vector {0} · affinity {1} · group {2} · {3} · {4}", r.Vector, Ui.Hex(r.Affinity), r.Group, r.Message ? "MSI" : L.T("line"), r.Kind)));
        if (_topo != null)
        {
            ulong mine = row.Cfg.Assign ?? 0, others = f.Others.Aggregate(0UL, (a, p) => a | p.Value);
            _info.Children.Add(Ui.LpMap(_topo, lp =>
            {
                ulong bit = 1UL << lp.LP;
                return ((mine & bit) != 0 ? Ui.B("Irq") : Ui.B("Panel2"), (others & bit) != 0 ? Ui.B("Dim") : null, null);
            }, null, 22));
            _info.Children.Add(Leg.Line(new Leg { C = Ui.B("Irq"), Text = L.T("this device") }, new Leg { C = Ui.B("Dim"), Look = "ring", Text = L.T("other devices") }));
        }
        if (S.AutoOn || _topo == null || !Flags.Open(Flags.DevicesWrite)) return;      // 22.12: a closed flag hides the control

        // The optional policy write (advanced, never automatic).
        _info.Children.Add(Ui.Head(L.T("Change the pin (advanced)")));
        if (!row.Cfg.Msi)
        {
            _info.Children.Add(Ui.Note(L.T("Skipped: the device does not use MSI, a pin would not work reliably.")));
            return;
        }
        var lps = new List<Opt> { new("", L.T("no pin")) };
        lps.AddRange(_topo.Sets.Where(s => s.Group == 0).OrderBy(s => s.LP).Select(s => new Opt(s.LP.ToString(CultureInfo.InvariantCulture), L.F("Processor {0}", s.LP))));
        var lp = Combo.New(150, lps.ToArray());
        var prio = Combo.New(150, new Opt("", L.T("Priority: keep")), new Opt("1", L.T("low")), new Opt("2", L.T("normal")), new Opt("3", L.T("high")));
        _info.Children.Add(Ui.Row(lp, prio));
        _info.Children.Add(Ui.Row(
            Ui.ConfirmBtn(L.T("Write and restart the device"), () =>
            {
                var want = new IrqConfig { Msi = row.Cfg.Msi, MsgLimit = row.Cfg.MsgLimit, Priority = row.Cfg.Priority };
                if (int.TryParse(Combo.Key(lp), out int n)) want.Assign = 1UL << n;
                if (int.TryParse(Combo.Key(prio), out int p)) want.Priority = p;
                S.BusyTask(L.T("Writing…"), () => Devices.Write(row.Dev, want), err =>
                {
                    GameMask.InvalidateIrq();
                    S.Did(err == null ? L.F("{0}: policy written, read back, device restarted", row.Dev.Name) : L.F("{0}: {1}", row.Dev.Name, err), err == null ? AppLog.Sev.Info : AppLog.Sev.Error);
                    Refresh();
                });
            }),
            Ui.ConfirmBtn(L.T("Restore all devices"), () => S.BusyTask(L.T("Writing…"), Devices.RevertAll, err =>
            {
                GameMask.InvalidateIrq();
                S.Did(err == null ? L.T("Device policies restored from the backup") : L.F("Devices: {0}", err), err == null ? AppLog.Sev.Info : AppLog.Sev.Error);
                Refresh();
            }))));
        _info.Children.Add(Ui.Note(L.T("The device restarts for a moment (a network card drops the connection). The original values are backed up first and put back if the read-back differs.")));
    }
}
