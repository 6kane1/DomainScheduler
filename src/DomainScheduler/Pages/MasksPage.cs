using System.Globalization;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>The game mask: the split into game and non-game cores, the spec, the IRQ refinement, the reservation.</summary>
sealed class MasksPage : Page
{
    string _spec = "auto", _irq = GameMask.CoreMode;
    bool _noSmt, _noCore0, _dirty, _load;
    ulong _add, _drop;
    StackPanel _view;
    Button _apply;
    Topo _topo;

    public MasksPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Masks");

    void LoadStored()
    {
        _spec = Settings.Get("MaskName");
        _irq = Settings.Get("IrqMode");
        _noSmt = Settings.GetBool("MaskNoSmt");
        _noCore0 = Settings.GetBool("MaskNoCore0");
        var e = Hub.MaskEdits();
        bool stale = _topo == null || e.Sig != GameMask.CpuSig(_topo);
        _add = stale ? 0 : e.Add;
        _drop = stale ? 0 : e.Drop;
        _dirty = false;
    }

    string SpecText() => _spec + (_noSmt ? "+nosmt" : "") + (_noCore0 ? "+nocore0" : "");

    GameMaskInfo Info() => GameMask.Build(_topo, SpecText(), new GameMask.Edits { Add = _add, Drop = _drop, Sig = GameMask.CpuSig(_topo) }, _irq);

    public override FrameworkElement Build()
    {
        _topo = TopoBuilder.Live();
        _view = new StackPanel();
        if (_topo == null)
        {
            _view.Children.Add(Ui.Text(L.T("CPU: the topology could not be read"), brush: "Warn"));
            return _view;
        }
        LoadStored();
        Draw();
        return Ui.Scroll(_view);
    }

    void Changed()
    {
        if (_load) return;
        _dirty = true;
        Draw();
    }

    void Draw()
    {
        _load = true;
        _view.Children.Clear();
        var split = _topo.Split;
        var info = Info();

        // Controls.
        var specs = new List<Opt>
        {
            new("auto", L.T("auto — chosen for this processor")), new("all", L.T("all processors")), new("game", L.T("game cores")),
            new("nongame", L.T("non-game cores")), new("vcache", L.T("big-cache cores")), new("pcores", L.T("P-cores")), new("ecores", L.T("E-cores")),
        };
        // 22.7: ccdN is named by what it is: the big-cache domain and which half of the numbers it takes.
        for (int i = 0; i < _topo.LLCGroups.Count && _topo.LLCGroups.Count > 1; i++)
        {
            byte llc = _topo.LLCGroups[i];
            string label = L.F("L3 domain {0}", i) + " · " + Masks.Ranges(TopoBuilder.Mask(_topo.Sets.Where(s => s.LLC == llc)));
            if (_topo.BigCacheLlc == llc) label += " · " + L.T("big cache");
            specs.Add(new Opt("ccd" + i.ToString(CultureInfo.InvariantCulture), label));
        }
        var spec = Combo.New(260, specs.ToArray());
        if (!Combo.Select(spec, _spec)) Combo.Select(spec, "auto");
        Combo.OnChange(spec, k => { _spec = k; _add = _drop = 0; Changed(); });
        var irq = Combo.New(260, new Opt(GameMask.Off, L.T("IRQ: do not refine")), new Opt(GameMask.ThreadMode, L.T("IRQ: exclude the pinned processors")),
            new Opt(GameMask.CoreMode, L.T("IRQ: exclude the whole pinned cores")));
        Combo.Select(irq, _irq);
        Combo.OnChange(irq, k => { _irq = k; Changed(); });
        var noSmt = new CheckRow(L.T("One processor per core (nosmt)"), L.T("Leaves the second SMT thread of every core out of the mask."), _noSmt);
        noSmt.Changed += v => { _noSmt = v; Changed(); };
        var noCore0 = new CheckRow(L.T("Without the first core (nocore0)"), L.T("Windows and drivers load the first core most."), _noCore0);
        noCore0.Changed += v => { _noCore0 = v; Changed(); };
        _apply = Ui.Btn(L.T("Apply"), Apply);
        _apply.IsEnabled = _dirty;
        var reset = Ui.Btn(L.T("Reset"), () =>
        {
            LoadStored();
            Draw();
        });
        reset.IsEnabled = _dirty;
        _view.Children.Add(Ui.Card(Ui.Col(
            Ui.Head(L.T("Mask of watched processes")),
            Ui.Note(L.T("Used by presets of watched processes that have no rule of their own. Auto picks its mask itself.")),
            Ui.Row(spec, irq),
            noSmt, noCore0,
            Ui.Row(_apply, reset))));

        // Result.
        var res = Masks.Resolve(_topo, SpecText());
        var facts = new FactGrid(1)
            .Add(L.T("Split"), split.CanSplit ? split.Label + " — " + L.F("game {0}, non-game {1}", Masks.Ranges(split.Game), Masks.Ranges(split.NonGame)) : split.Why())
            .Add(L.T("Mask"), Ui.Hex(info.Final) + "  " + Masks.Ranges(info.Final) + (info.Adjusted ? "  · " + L.T("adjusted") : ""))
            .Add(L.T("Background"), Masks.Ranges(GameMask.Background(_topo, info)));
        var result = Ui.Col(Ui.Head(L.T("Result")), facts);
        foreach (var n in res.Notes) result.Children.Add(Ui.Text(Masks.NoteText(n), Ui.FsNote, "Warn", wrap: true));
        if (info.EditsStale) result.Children.Add(Ui.Text(L.T("The manual edits were made on another processor and are dropped."), Ui.FsNote, "Warn", wrap: true));
        foreach (var n in info.Notes) result.Children.Add(Ui.Note(n == GameMask.WideNote ? L.T("wide-irq: partial — a device pinned to several cores takes out only its own processors, not whole cores") : n));
        if (info.IrqIgnored) result.Children.Add(Ui.Note(L.T("IRQ refinement ignored: it would remove more than half of the mask.")));
        foreach (var d in info.IrqDevices) result.Children.Add(Ui.Note(L.F("{0}: processors {1}", d.Key, Masks.Ranges(d.Value))));
        foreach (var d in info.IrqWide) result.Children.Add(Ui.Note(L.F("{0}: processors {1} (several cores, not excluded)", d.Key, Masks.Ranges(d.Value))));

        // Visualization; the per-processor editor only where there is something to split.
        bool edit = split.CanSplit;
        result.Children.Add(Ui.LpMap(_topo, lp =>
        {
            ulong bit = 1UL << lp.LP;
            var fill = !split.CanSplit ? Ui.B("Panel2") : (split.Game & bit) != 0 ? Ui.GameC : Ui.NonGameC;
            var ring = (info.IrqDrop & bit) != 0 ? Ui.B("Irq") : (info.Final & bit) != 0 ? Ui.B("Text") : null;
            return (fill, ring, null);
        }, edit ? lp =>
        {
            ulong bit = 1UL << lp.LP;
            if ((info.Base & bit) != 0) _drop ^= bit;
            else _add ^= bit;
            Changed();
        } : null));
        var legs = new List<Leg>();
        if (split.CanSplit)
        {
            string half = split.Family == "x3d" ? " (" + (split.UpperHalf ? L.T("big cache, upper half of the numbers") : L.T("big cache, lower half of the numbers")) + ")" : "";
            legs.Add(new Leg { C = Ui.GameC, Text = L.T("game cores") + half });
            legs.Add(new Leg { C = Ui.NonGameC, Text = L.T("non-game cores") });
        }
        legs.Add(new Leg { C = Ui.B("Text"), Look = "ring", Text = L.T("in the mask") });
        legs.Add(new Leg { C = Ui.B("Irq"), Look = "ring", Text = L.T("excluded for IRQ") });
        result.Children.Add(Leg.Line(legs.ToArray()));
        result.Children.Add(Ui.Note(edit ? L.T("Click a processor to add it to the mask or take it out.") : split.Why()));
        _view.Children.Add(Ui.Card(result));

        _view.Children.Add(Ui.Card(Reservation_()));
        _load = false;
    }

    void Apply()
    {
        Settings.Set("MaskName", _spec);
        Settings.Set("IrqMode", _irq);
        Settings.SetBool("MaskNoSmt", _noSmt);
        Settings.SetBool("MaskNoCore0", _noCore0);
        Settings.Set("MaskAdd", _add.ToString("X", CultureInfo.InvariantCulture));
        Settings.Set("MaskDrop", _drop.ToString("X", CultureInfo.InvariantCulture));
        Settings.Set("MaskSig", GameMask.CpuSig(_topo));
        H.Configure();
        _dirty = false;
        var info = Info();
        S.Did(L.F("Mask saved: {0} → {1} ({2})", SpecText(), Ui.Hex(info.Final), Masks.Ranges(info.Final)));
        Draw();
    }

    /// <summary>Core reservation (ReservedCpuSets): only the whole non-game half, with a backup and a read-back.</summary>
    UIElement Reservation_()
    {
        var st = Reservation.Read();
        var split = _topo.Split;
        var box = Ui.Col(Ui.Head(L.T("Core reservation")),
            Ui.Note(L.T("Windows keeps reserved cores free of ordinary programs. Works after a reboot; only the non-game cores can be reserved.")));
        string state = st.Kind == "error" ? L.F("cannot be read ({0})", st.Error)
            : st.Mask == 0 ? L.T("not set")
            : L.F("processors {0}", Masks.Ranges(st.Mask)) + (st.Pending ? " · " + L.T("waits for a reboot") : "");
        box.Children.Add(Ui.Pair(L.T("Now"), state));
        var refusals = Reservation.Refusals(_topo, split.NonGame);
        if (refusals.Count > 0) box.Children.Add(Ui.Note(L.F("Cannot be reserved here: {0}", string.Join("; ", refusals))));
        var set = Ui.ConfirmBtn(L.T("Reserve the non-game cores"), () => S.BusyTask(L.T("Writing…"), () => Reservation.ApplyNonGame(_topo), err =>
        {
            S.Did(err == null ? L.T("Reservation written and read back: it works after a reboot") : L.F("Reservation: {0}", err), err == null ? AppLog.Sev.Info : AppLog.Sev.Error);
            Draw();
        }));
        set.IsEnabled = refusals.Count == 0 && !S.AutoOn;
        var undo = Ui.ConfirmBtn(L.T("Remove the reservation"), () => S.BusyTask(L.T("Writing…"), Reservation.Revert, err =>
        {
            S.Did(err == null ? L.T("Reservation removed: the original value is back") : L.F("Reservation: {0}", err), err == null ? AppLog.Sev.Info : AppLog.Sev.Warn);
            Draw();
        }));
        undo.IsEnabled = st.HasBackup && !S.AutoOn;
        box.Children.Add(Ui.Row(set, undo));
        return box;
    }
}
