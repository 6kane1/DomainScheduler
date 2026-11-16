using System.Globalization;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>Threads of one process: rate, names, start modules; manual priority, ideal processor and boost.</summary>
sealed class ThreadsPage : Page
{
    ComboBox _proc, _prio, _ideal, _boost, _cls;
    Table _table;
    StackPanel _panel;
    ThreadSnap _snap;
    ThreadRow _sel;
    uint _pid;
    string _exe = "";
    bool _load;      // programmatic loads do not count as user input

    public ThreadsPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Threads");

    public override FrameworkElement Build()
    {
        _snap = null;
        _sel = null;
        _load = true;
        var picked = S.Get<ProcessesPage>().Picked;
        var opts = new List<Opt>();
        var procs = H.Engine.ProcessesNow();
        if (picked.Pid != 0 && procs.ContainsKey(picked.Pid)) opts.Add(new Opt(picked.Pid.ToString(CultureInfo.InvariantCulture), picked.Exe + " (" + picked.Pid + ")"));
        foreach (var (pid, p) in procs.Where(p => H.Watch.Contains(p.Value.Name)).OrderBy(p => p.Value.Name, StringComparer.OrdinalIgnoreCase))
            if (pid != picked.Pid) opts.Add(new Opt(pid.ToString(CultureInfo.InvariantCulture), p.Name + " (" + pid + ")"));
        if (opts.Count == 0) opts.Add(new Opt("0", L.T("No watched process is running")));
        _proc = Combo.New(300, opts.ToArray());
        Combo.OnChange(_proc, _ => { if (!_load) Refresh(); });
        _table = new Table((L.T("TID"), 64), (L.T("Name"), 190), (L.T("Start module"), 0), (L.T("Rate"), 76), (L.T("Priority"), 66), (L.T("Ideal"), 54));
        _table.SelectionChanged += () =>
        {
            _sel = _table.Selected as ThreadRow;
            BuildPanel();
        };
        _panel = new StackPanel();
        var bar = Ui.Row(_proc, Ui.Btn(L.T("Refresh"), Refresh));
        var bottom = Ui.Card(_panel);
        bottom.Margin = new Thickness(0, Ui.Gap, 0, Ui.Gap);
        var dock = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        DockPanel.SetDock(bottom, Dock.Bottom);
        dock.Children.Add(bar);
        dock.Children.Add(bottom);
        dock.Children.Add(_table.Root);
        _load = false;
        BuildPanel();
        return dock;
    }

    public override void Shown() => Refresh();

    void Refresh()
    {
        uint.TryParse(Combo.Key(_proc), NumberStyles.None, CultureInfo.InvariantCulture, out _pid);
        if (_pid == 0)
        {
            _table.SetRows(Array.Empty<(object, UIElement[])>());
            return;
        }
        uint pid = _pid;
        S.Busy(L.T("Measuring threads…"), () =>
        {
            string exe = H.Engine.NameOf(pid);
            if (Guard.IsDenied(exe) || H.Engine.ReadProcess(pid, exe).Denied) return (exe, (ThreadSnap)null);
            H.Engine.Snapshot(pid);
            Thread.Sleep(700);
            return (exe, H.Engine.Snapshot(pid));
        }, res =>
        {
            _exe = res.exe ?? "";
            _snap = res.Item2;
            _sel = null;
            var ideals = H.Engine.Ideals;
            var rows = _snap?.Rows ?? new List<ThreadRow>();
            _table.SetRows(rows.Select(r =>
            {
                bool denied = Engine.IsDeniedModule(r);
                string brush = denied ? "Faint" : "Text";
                return ((object)r, new UIElement[]
                {
                    Ui.Text(r.Tid.ToString(CultureInfo.InvariantCulture), Ui.FsNote, "Dim"),
                    Ui.Text(r.Desc, brush: brush),
                    Ui.Text(denied ? r.Sym + "  · " + L.T("denylist") : r.Sym, Ui.FsNote, denied ? "Faint" : "Dim"),
                    Ui.Text((r.Rate / 1e6).ToString("0.0", CultureInfo.CurrentCulture), Ui.FsNote, "Dim"),
                    Ui.Text(r.Prio.ToString(CultureInfo.InvariantCulture), Ui.FsNote, "Dim"),
                    Ui.Text(ideals.TryGetValue(pid + ":" + r.Tid, out uint lp) ? lp.ToString(CultureInfo.InvariantCulture) : "", Ui.FsNote, "Dim"),
                });
            }));
            BuildPanel();
        });
    }

    void BuildPanel()
    {
        _panel.Children.Clear();
        if (_pid == 0 || _exe.Length == 0)
        {
            _panel.Children.Add(Ui.Note(L.T("Pick a process and press Refresh. Rate is million CPU cycles per second.")));
            return;
        }
        if (_snap == null)
        {
            _panel.Children.Add(Ui.Note(L.F("{0} is on the denylist: its threads are not read.", _exe)));
            return;
        }
        string why = S.AutoOn ? L.T("Auto is on: threads are placed by Auto.") : H.Eligibility(_exe);
        var t = TopoBuilder.Live();
        _load = true;
        _cls = Combo.New(150, new Opt("idle", L.T("Idle")), new Opt("below", L.T("Below normal")), new Opt("normal", L.T("Normal")), new Opt("above", L.T("Above normal")), new Opt("high", L.T("High")));
        Combo.Select(_cls, "normal");
        var procRow = Ui.Row(Ui.Text(_exe + ":  ", bold: true), _cls,
            Ui.Btn(L.T("Set class"), () => Process(log => H.Engine.ProcessClassManual((int)_pid, Combo.Key(_cls), log))),
            Ui.Btn(L.T("Boost off"), () => Process(log => H.Engine.ProcessBoost((int)_pid, true, log))),
            Ui.Btn(L.T("Boost on"), () => Process(log => H.Engine.ProcessBoost((int)_pid, false, log))));
        procRow.IsEnabled = why == null;
        _panel.Children.Add(procRow);

        if (_sel == null) _panel.Children.Add(Ui.Note(L.T("Select a thread to change it.")));
        else
        {
            bool denied = Engine.IsDeniedModule(_sel);
            _prio = Combo.New(130, new Opt("", L.T("Priority: keep")), new Opt("-15", "-15"), new Opt("-2", "-2"), new Opt("-1", "-1"), new Opt("0", "0"), new Opt("1", "+1"), new Opt("2", "+2"));
            var lps = new List<Opt> { new("", L.T("Ideal: keep")) };
            if (t != null) lps.AddRange(t.Sets.Where(s => s.Group == 0).OrderBy(s => s.LP).Select(s => new Opt(s.LP.ToString(CultureInfo.InvariantCulture), L.F("Processor {0}", s.LP))));
            _ideal = Combo.New(150, lps.ToArray());
            _boost = Combo.New(130, new Opt("", L.T("Boost: keep")), new Opt("on", L.T("Boost on")), new Opt("off", L.T("Boost off")));
            uint tid = _sel.Tid;
            var row = Ui.Row(Ui.Text(L.F("Thread {0}:  ", tid), bold: true), _prio, _ideal, _boost, Ui.Btn(L.T("Apply"), () =>
            {
                string prio = Combo.Key(_prio), ideal = Combo.Key(_ideal), boost = Combo.Key(_boost);
                uint pid = _pid;
                H.Opt.Run(() => H.Engine.ThreadManual((int)pid, tid, prio, ideal, boost), msg =>
                {
                    S.Did(msg ?? "");
                    Refresh();
                });
            }));
            row.IsEnabled = why == null && !denied;
            _panel.Children.Add(row);
            if (denied) _panel.Children.Add(Ui.Note(L.T("This thread belongs to a denylisted module: shown, never changed.")));
        }
        if (why != null) _panel.Children.Add(Ui.Note(why));
        _load = false;
    }

    void Process(Func<List<string>, bool> act)
    {
        H.Opt.Run(() =>
        {
            var log = new List<string>();
            act(log);
            return log;
        }, log =>
        {
            foreach (var l in log ?? new()) S.Did(l);
        });
    }
}
