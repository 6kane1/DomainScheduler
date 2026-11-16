using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>The one-button page: state, the toggle, five quick actions and what is changed now. No settings.</summary>
sealed class QuickPage : Page
{
    TextBlock _hero, _done, _layer;
    Button _toggle;

    public QuickPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Quick");

    public override FrameworkElement Build()
    {
        _hero = Ui.Text("", Ui.FsHero, bold: true);
        _hero.Margin = new Thickness(0, 6, 0, 10);
        _toggle = new Button { Height = 52, Width = 260, FontSize = Ui.FsBody * 1.25, FontWeight = FontWeights.SemiBold, HorizontalAlignment = HorizontalAlignment.Left };
        _toggle.Click += (_, _) =>
        {
            _toggle.IsEnabled = false;
            H.Paused = false;
            H.Opt.Queue(H.Auto.Click, reports: true);
        };
        _done = Ui.Text("", wrap: true);
        _done.Margin = Ui.MTop;
        _layer = Ui.Note("");
        var quick = Ui.Row(
            Ui.Btn(L.T("Boost"), Boost),
            Ui.Btn(L.T("Reapply"), Reapply),
            Ui.Btn(L.T("Resync"), Resync),
            Ui.Btn(L.T("Revert"), Revert),
            Ui.Btn(L.T("Report"), Report));
        quick.Margin = Ui.MTop;
        var page = Ui.Col(_hero, _toggle, quick, _done, _layer);
        Tick();
        return Ui.Scroll(page);
    }

    public override void Tick()
    {
        if (_hero == null) return;
        _hero.Text = H.Auto.State switch
        {
            AutoState.Armed => L.T("Waiting"),
            AutoState.Active => L.T("Active"),
            AutoState.Reverting => L.T("Reverting"),
            AutoState.Faulted => L.T("Error"),
            _ => L.T("Off"),
        };
        _toggle.Content = H.Auto.On ? L.T("Turn off") : L.T("Turn on");
        _toggle.IsEnabled = H.Auto.ButtonEnabled;
        _toggle.Background = Ui.B(H.Auto.On ? "On" : "Panel2");
        _done.Text = H.Auto.Summary();
        _layer.Text = H.Paused ? L.T("Presets are paused after Revert: press Reapply to resume.")
            : L.F("Presets: {0} processes changed, re-applied every {1} s.", H.Engine.ChangedPids().Count, H.Auto.On ? AutoPolicy.ReapplyMs / 1000 : H.ReapplyTicks);
    }

    /// <summary>The game: the one Auto holds, else the first running process of the watchlist.</summary>
    (uint Pid, string Exe) Game()
    {
        if (H.Auto.GamePid != 0) return (H.Auto.GamePid, H.Auto.GameExe);
        foreach (var (pid, p) in H.Engine.ProcessesNow())
            if (H.Watch.Contains(p.Name)) return (pid, p.Name);
        return (0, "");
    }

    void Boost() => H.Opt.Run(() =>
    {
        var log = new List<string>();
        var (pid, exe) = Game();
        if (pid == 0) { log.Add(L.T("Boost: no running process of the watchlist")); return log; }
        string why = H.Eligibility(exe);
        if (why != null) { log.Add(why); return log; }
        bool off = !H.Engine.ReadProcess(pid, exe).BoostOff;
        if (H.Engine.ProcessBoost((int)pid, off, log)) log.Add(off ? L.F("{0}: dynamic priority boost off", exe) : L.F("{0}: dynamic priority boost on", exe));
        return log;
    }, log => { foreach (var l in log ?? new()) S.Did(l); });

    void Reapply()
    {
        H.Paused = false;
        H.Opt.Run(() =>
        {
            H.Sys.Invalidate();
            return H.Sys.Pass(H.Auto.GamePid);
        }, n => S.Did(L.F("Reapplied: {0} processes", n)));
    }

    void Resync() => H.Opt.Run(() =>
    {
        TopoBuilder.Invalidate();
        GameMask.InvalidateIrq();
        Tables.Reload();
        H.LoadPresets();
        H.Configure();
        return H.Presets.Count;
    }, n => S.Did(L.F("Resynced: {0} presets, watchlist {1}", n, string.Join(", ", H.Watch))));

    void Revert()
    {
        H.Paused = true;
        H.Opt.Run(() => H.Auto.RevertEverything(L.T("Button")), _ => S.Did(L.T("Everything was put back")));
    }

    void Report() => H.Opt.Run(H.Auto.Report, text =>
    {
        try
        {
            Clipboard.SetText(text ?? "");
            S.Did(L.T("Report copied to the clipboard"));
        }
        catch (Exception e) { S.Did(L.F("Clipboard: {0}", e.Message), AppLog.Sev.Warn); }
    });
}
