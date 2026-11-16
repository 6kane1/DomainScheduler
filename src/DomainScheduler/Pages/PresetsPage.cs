using System.Windows;
using System.Windows.Controls;
using Microsoft.Win32;

namespace DomainScheduler.App;

/// <summary>Presets by program: process settings and thread rules. Built-in ones are read-only; a copy can be edited.</summary>
sealed class PresetsPage : Page
{
    Dictionary<string, Preset> _work = new(StringComparer.OrdinalIgnoreCase);
    string _cur = "";
    Table _list;
    StackPanel _editor;
    Button _save;
    ComboBox _cls, _io, _mem, _eco, _boost;
    bool _pload;       // true while the editor is filled by the program: nothing becomes dirty

    public PresetsPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Presets");

    static Dictionary<string, Preset> CloneAll(Dictionary<string, Preset> src) =>
        src.ToDictionary(kv => kv.Key, kv => kv.Value.Clone(), StringComparer.OrdinalIgnoreCase);

    /// <summary>How many presets differ from what is saved.</summary>
    public int DirtyCount
    {
        get
        {
            int n = _work.Count(kv => !H.Presets.TryGetValue(kv.Key, out var saved) || !Store.Same(saved, kv.Value));
            return n + H.Presets.Keys.Count(k => !_work.ContainsKey(k));
        }
    }

    string Source(string exe)
    {
        if (!_work.TryGetValue(exe, out var p)) return "";
        if (p.Builtin) return L.T("built-in");
        return ThreadIni.LoadBundled().Presets.ContainsKey(exe) ? L.T("yours, overrides the built-in") : L.T("yours");
    }

    public override FrameworkElement Build()
    {
        _work = CloneAll(H.Presets);
        if (!_work.ContainsKey(_cur)) _cur = "";
        _list = new Table((L.T("Program"), 0), (L.T("Source"), 150), (L.T("On"), 36));
        _list.SelectionChanged += () =>
        {
            _cur = _list.Selected as string ?? "";
            FillEditor();
        };
        _editor = new StackPanel();
        _save = Ui.Btn("", Save);

        var watchOpts = H.Watch.Where(w => !_work.ContainsKey(w)).Select(w => new Opt(w, w)).ToList();
        if (watchOpts.Count == 0) watchOpts.Add(new Opt("", L.T("no watched program without a preset")));
        var newExe = Combo.New(230, watchOpts.ToArray());
        var bar1 = Ui.Row(_save, Ui.Btn(L.T("Resync"), Resync), newExe, Ui.Btn(L.T("New preset"), () =>
        {
            string exe = Combo.Key(newExe);
            if (exe.Length == 0 || _work.ContainsKey(exe)) return;
            _work[exe] = new Preset { Exe = exe, Class = "", Io = "", Mem = "", Eco = "", Boost = "" };
            _cur = exe;
            FillList();
            FillEditor();
        }), Ui.Btn(L.T("Import…"), Import), Ui.Btn(L.T("Export…"), Export));

        // Profiles: named copies of the whole set.
        var profiles = Store.ListProfiles();
        var profOpts = profiles.Select(p => new Opt(p, p)).ToList();
        if (profOpts.Count == 0) profOpts.Add(new Opt("", L.T("no profiles")));
        var prof = Combo.New(180, profOpts.ToArray());
        string name = "";
        var bar2 = Ui.Row(Ui.Text(L.T("Profiles:") + "  ", Ui.FsNote, "Dim"), prof,
            Ui.Btn(L.T("Load"), () =>
            {
                string p = Combo.Key(prof);
                if (p.Length == 0) return;
                Store.Load(out var loaded, out _, Store.ProfilePath(p));
                Store.MarkBuiltin(loaded, ThreadIni.LoadBundled().Presets.Values);
                _work = loaded;
                _cur = "";
                FillList();
                FillEditor();
                S.Did(L.F("Profile {0} loaded: press Save to use it", p));
            }, 70),
            Ui.Btn(L.T("Copy"), () =>
            {
                string p = Combo.Key(prof);
                if (p.Length > 0 && Store.CopyProfile(p, p + " 2")) { S.Did(L.F("Profile {0} copied", p)); S.ShowPage<PresetsPage>(); }
            }, 70),
            Ui.ConfirmBtn(L.T("Delete"), () =>
            {
                string p = Combo.Key(prof);
                if (p.Length > 0 && Store.DeleteProfile(p)) { S.Did(L.F("Profile {0} deleted", p)); S.ShowPage<PresetsPage>(); }
            }),
            Ui.Input("", 150, v => name = v),
            Ui.Btn(L.T("Save as profile"), () =>
            {
                if (string.IsNullOrWhiteSpace(name)) return;
                Store.SaveProfile(name.Trim(), _work, H.Background);
                S.Did(L.F("Profile {0} saved", Store.Sanitize(name.Trim())));
                S.ShowPage<PresetsPage>();
            }));

        var grid = new Grid();
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(330) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var right = Ui.Card(Ui.Scroll(_editor));
        right.Margin = new Thickness(Ui.Gap, 0, 0, Ui.Gap);
        Grid.SetColumn(right, 1);
        grid.Children.Add(_list.Root);
        grid.Children.Add(right);
        var dock = new DockPanel();
        DockPanel.SetDock(bar1, Dock.Top);
        DockPanel.SetDock(bar2, Dock.Top);
        dock.Children.Add(bar1);
        dock.Children.Add(bar2);
        dock.Children.Add(grid);
        FillList();
        FillEditor();
        return dock;
    }

    void Touch()
    {
        int n = DirtyCount;
        _save.Content = n > 0 ? L.F("Save ({0})", n) : L.T("Save");
        _save.IsEnabled = n > 0;
    }

    void FillList()
    {
        _list.SetRows(_work.Values.OrderBy(p => p.Exe, StringComparer.OrdinalIgnoreCase).Select(p => ((object)p.Exe, new UIElement[]
        {
            Ui.Text(p.Exe, brush: p.Enabled ? "Text" : "Dim"),
            Ui.Text(Source(p.Exe), Ui.FsNote, "Dim"),
            Ui.Text(p.Enabled ? "✓" : "", brush: "On"),
        })));
        if (_cur.Length > 0) _list.Select(_cur, raise: false);
        Touch();
    }

    static Opt[] Keep(params (string, string)[] items) => items.Select(i => new Opt(i.Item1, i.Item2)).ToArray();

    void FillEditor()
    {
        _pload = true;
        _editor.Children.Clear();
        if (!_work.TryGetValue(_cur, out var p))
        {
            _editor.Children.Add(Ui.Note(L.T("Select a preset. A preset of your own applies only while its program is on the watchlist.")));
            _pload = false;
            return;
        }
        bool ro = p.Builtin;
        _editor.Children.Add(Ui.Head(p.Exe));
        _editor.Children.Add(Ui.Pair(L.T("Source"), Source(p.Exe)));
        var on = new CheckRow(L.T("Enabled"), "", p.Enabled);
        on.Changed += v => { p.Enabled = v; FillList(); };
        _editor.Children.Add(on);
        if (ro)
            _editor.Children.Add(Ui.Row(Ui.Btn(L.T("Copy to edit"), () =>
            {
                var c = p.Clone();
                c.Builtin = false;
                _work[p.Exe] = c;
                FillList();
                FillEditor();
            }), Ui.Text(L.T("Built-in presets are read-only."), Ui.FsNote, "Dim")));
        else
            _editor.Children.Add(Ui.Row(Ui.ConfirmBtn(L.T("Delete the preset"), () =>
            {
                _work.Remove(p.Exe);
                if (ThreadIni.LoadBundled().Presets.TryGetValue(p.Exe, out var shipped)) _work[p.Exe] = shipped.Clone();   // the built-in one is back
                _cur = "";
                FillList();
                FillEditor();
            })));

        string keep = L.T("keep");
        _cls = Combo.New(150, Keep(("", L.T("Class: ") + keep), ("idle", L.T("Idle")), ("below", L.T("Below normal")), ("normal", L.T("Normal")), ("above", L.T("Above normal"))));
        _io = Combo.New(150, Keep(("", L.T("I/O: ") + keep), ("verylow", L.T("I/O very low")), ("low", L.T("I/O low")), ("normal", L.T("I/O normal"))));
        _mem = Combo.New(150, Keep(("", L.T("Memory: ") + keep), ("verylow", L.T("Memory very low")), ("low", L.T("Memory low")), ("medium", L.T("Memory medium")), ("normal", L.T("Memory normal"))));
        _eco = Combo.New(150, Keep(("", L.T("EcoQoS: ") + keep), ("off", L.T("EcoQoS off")), ("on", L.T("EcoQoS on")), ("default", L.T("EcoQoS by Windows"))));
        _boost = Combo.New(150, Keep(("", L.T("Boost: keep")), ("on", L.T("Boost on")), ("off", L.T("Boost off"))));
        Combo.Select(_cls, p.Class);
        Combo.Select(_io, p.Io);
        Combo.Select(_mem, p.Mem);
        Combo.Select(_eco, p.Eco);
        Combo.Select(_boost, p.Boost);
        Combo.OnChange(_cls, k => { if (!_pload) { p.Class = k; Touch(); } });
        Combo.OnChange(_io, k => { if (!_pload) { p.Io = k; Touch(); } });
        Combo.OnChange(_mem, k => { if (!_pload) { p.Mem = k; Touch(); } });
        Combo.OnChange(_eco, k => { if (!_pload) { p.Eco = k; Touch(); } });
        Combo.OnChange(_boost, k => { if (!_pload) { p.Boost = k; Touch(); } });
        var procA = Ui.Row(_cls, _io, _mem);
        var procB = Ui.Row(_eco, _boost);
        procA.IsEnabled = procB.IsEnabled = !ro;
        _editor.Children.Add(procA);
        _editor.Children.Add(procB);

        // Heat policy (22.6): what the hottest thread and the next ones get.
        Opt[] heat(string who) => Keep(("none", who + L.T("by the rules")), ("ideal", who + L.T("ideal processor")), ("pin", who + L.T("ideal processor + own CPU Set")));
        var hotFirst = Combo.New(300, heat(L.T("Hottest thread: ")));
        var hotOthers = Combo.New(300, heat(L.T("Next hot threads: ")));
        Combo.Select(hotFirst, p.HotFirst);
        Combo.Select(hotOthers, p.HotOthers);
        Combo.OnChange(hotFirst, k => { if (!_pload) { p.HotFirst = k; Touch(); } });
        Combo.OnChange(hotOthers, k => { if (!_pload) { p.HotOthers = k; Touch(); } });
        var heatRow = Ui.Row(hotFirst, hotOthers);
        heatRow.IsEnabled = !ro;
        _editor.Children.Add(Ui.Head(L.T("Heat policy")));
        _editor.Children.Add(Ui.Note(L.T("The threads of this program are ranked by CPU use. \"Own CPU Set\" is a hard limit to one processor: used only when the threads are recognized with confidence.")));
        _editor.Children.Add(heatRow);

        _editor.Children.Add(Ui.Head(L.T("Thread rules")));
        _editor.Children.Add(Ui.Note(L.T("Match by start module (name.dll) or by thread name; * and ? are wildcards. Filter: first, last or an index by thread ID.")));
        var rules = new StackPanel { IsEnabled = !ro };
        foreach (var rule in p.Threads.ToList())
        {
            var r = rule;
            var en = new CheckRow("", "", r.Enabled) { Margin = new Thickness(0, 5, 0, 0), Width = 28 };
            en.Changed += v => { r.Enabled = v; Touch(); };
            var match = Combo.New(104, Keep(("module", L.T("module")), ("desc", L.T("name")), ("tid", L.T("thread ID"))));
            Combo.Select(match, r.Match);
            Combo.OnChange(match, k => { if (!_pload) { r.Match = k; Touch(); } });
            var prio = Combo.New(86, Keep(("", "—"), ("-15", "-15"), ("-2", "-2"), ("-1", "-1"), ("0", "0"), ("1", "+1"), ("2", "+2")));
            if (!Combo.Select(prio, Engine.TPRIO.TryGetValue(r.Prio ?? "", out int named) ? named.ToString() : r.Prio ?? "")) Combo.Select(prio, "");
            Combo.OnChange(prio, k => { if (!_pload) { r.Prio = k; Touch(); } });
            var boost = Combo.New(104, Keep(("", L.T("boost —")), ("on", L.T("Boost on")), ("off", L.T("Boost off"))));
            Combo.Select(boost, r.Boost);
            Combo.OnChange(boost, k => { if (!_pload) { r.Boost = k; Touch(); } });
            var pattern = Ui.Input(r.Pattern, 170, v => { if (!_pload) { r.Pattern = v; Touch(); } });
            var ideal = Ui.Input(r.Ideal, 52, v => { if (!_pload) { r.Ideal = v.Trim(); Touch(); } });
            ideal.ToolTip = L.T("Ideal processor: empty, auto or a number");
            var tid = Ui.Input(r.Tid, 52, v => { if (!_pload) { r.Tid = v.Trim(); Touch(); } });
            tid.ToolTip = L.T("Filter: empty, first, last or an index");
            var del = Ui.Btn("✕", () =>
            {
                p.Threads.Remove(r);
                FillEditor();
                Touch();
            }, 30);
            rules.Children.Add(Ui.Row(en, match, pattern, prio, ideal, boost, tid, del));
        }
        rules.Children.Add(Ui.Row(Ui.Btn(L.T("Add a rule"), () =>
        {
            p.Threads.Add(new ThreadRule { Match = "module", Pattern = "", Prio = "0" });
            FillEditor();
            Touch();
        })));
        _editor.Children.Add(rules);
        _pload = false;
        Touch();
    }

    void Save()
    {
        if (DirtyCount == 0) return;
        var bad = new ThreadIni.Result();
        foreach (var p in _work.Values.Where(p => !p.Builtin)) bad.Presets[p.Exe] = p;
        foreach (var problem in ThreadIni.SelfTest(bad).Where(x => !x.Contains(L.T("no rules")))) AppLog.Add(problem, AppLog.Sev.Warn);
        int n = DirtyCount;
        H.SavePresets(CloneAll(_work));
        S.Did(L.F("Presets saved: {0} changed", n));
        FillList();
    }

    /// <summary>Reads the presets again from the disk; the page is clean afterwards.</summary>
    void Resync()
    {
        H.LoadPresets();
        H.Configure();
        _work = CloneAll(H.Presets);
        if (!_work.ContainsKey(_cur)) _cur = "";
        FillList();
        FillEditor();
        S.Did(L.F("Presets read from the disk: {0}", _work.Count));
    }

    void Import()
    {
        var dlg = new OpenFileDialog { Filter = "JSON (*.json)|*.json", CheckFileExists = true };
        if (dlg.ShowDialog(S) != true) return;
        try
        {
            var notes = Store.Load(out var loaded, out _, dlg.FileName);
            foreach (var n in notes) AppLog.Add(n, AppLog.Sev.Warn);
            int count = 0;
            foreach (var p in loaded.Values.Where(p => !p.Builtin))
            {
                _work[p.Exe] = p;
                count++;
            }
            FillList();
            FillEditor();
            S.Did(L.F("Imported {0} presets: press Save to use them", count));
        }
        catch (Exception e) { S.Did(L.F("Import: {0}", e.Message), AppLog.Sev.Error); }
    }

    void Export()
    {
        var dlg = new SaveFileDialog { Filter = "JSON (*.json)|*.json", FileName = "presets.json", OverwritePrompt = true };
        if (dlg.ShowDialog(S) != true) return;
        try
        {
            var own = _work.Where(kv => !kv.Value.Builtin).ToDictionary(kv => kv.Key, kv => kv.Value, StringComparer.OrdinalIgnoreCase);
            Store.Save(own, H.Background, dlg.FileName);
            S.Did(L.F("Exported {0} presets to {1}", own.Count, dlg.FileName));
        }
        catch (Exception e) { S.Did(L.F("Export: {0}", e.Message), AppLog.Sev.Error); }
    }
}
