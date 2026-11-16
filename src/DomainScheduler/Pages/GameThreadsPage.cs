using System.Globalization;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>
/// The game's key threads, fully automatic: one switch for Auto, one for the mode, the rest is
/// a live view. There is nothing here that can be set wrong.
/// </summary>
sealed class GameThreadsPage : Page
{
    TextBlock _state, _hint, _moves;
    Button _toggle;
    CheckRow _mode;
    Table _table;
    StackPanel _log;
    bool _shownTop5;

    public GameThreadsPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("CS2 threads");

    public override FrameworkElement Build()
    {
        _state = Ui.Text("", Ui.FsBody * 1.25, bold: true, wrap: true);
        _hint = Ui.Note("");
        _toggle = new Button { Height = 40, Width = 220, FontSize = Ui.FsBody, FontWeight = FontWeights.SemiBold, HorizontalAlignment = HorizontalAlignment.Left, Margin = Ui.MRow };
        _toggle.Click += (_, _) =>
        {
            _toggle.IsEnabled = false;
            H.Paused = false;
            H.Opt.Queue(H.Auto.Click, reports: true);
        };
        _mode = new CheckRow(L.T("Dynamic Top-5 (experimental)"),
            L.T("Off: the threads are recognized by their role and placed once (the default). On: the five busiest threads are re-checked every second and moved between cores and priorities as the load changes."),
            H.Auto.Top5Mode);
        _mode.Changed += v =>
        {
            Settings.SetBool("Top5On", v);
            S.Did(v ? L.T("Dynamic Top-5: on") : L.T("Dynamic Top-5: off"));
            _mode.IsEnabled = false;
            H.Opt.Run(() => { H.Auto.SetTop5(v); return v; }, _ =>
            {
                _mode.IsEnabled = true;
                BuildTable();
            });
        };
        _moves = Ui.Note("");
        _log = new StackPanel();
        var top = Ui.Card(Ui.Col(_state, _hint, _toggle, _mode));
        var host = new ContentControl { Focusable = false, Height = 190 };
        _hostTable = host;
        BuildTable();
        var page = Ui.Col(top,
            Ui.Card(Ui.Col(Ui.Head(L.T("Key threads now")), host, _moves)),
            Ui.Card(Ui.Col(Ui.Head(L.T("Last moves")), _log)),
            Ui.Card(Ui.Col(Ui.Head(L.T("What happens here")),
                Ui.Note(L.T("Everything on this page is automatic. The program finds the game's threads, gives the leading ones a slightly higher priority and a preferred core each (never core 0), and puts every value back when the game closes, when you turn Auto off or when you exit.")),
                Ui.Note(L.T("A preferred core is a hint: Windows may still move a thread when its core is busy. Nothing is hard-pinned here.")),
                Ui.Note(L.T("Threads of anti-cheat modules are never touched. While an anti-cheat client (FACEIT) is running, the game itself is not changed at all.")),
                Ui.Note(L.T("To compare the two modes, play the same scene with each and look at the 1% low / p99 frame time; a few percent of difference is noise.")))));
        Tick();
        return Ui.Scroll(page);
    }

    ContentControl _hostTable;

    void BuildTable()
    {
        _shownTop5 = H.Auto.Top5Mode;
        _table = _shownTop5
            ? new Table((L.T("Slot"), 44), (L.T("Thread"), 0), (L.T("TID"), 64), (L.T("Load"), 60), (L.T("Core"), 50), (L.T("Priority"), 70), (L.T("Role"), 70))
            : new Table((L.T("Role"), 80), (L.T("Thread"), 0), (L.T("TID"), 64), (L.T("Core"), 50));
        _hostTable.Content = _table.Root;
    }

    static string RoleText(ThreadRole r) => r switch
    {
        ThreadRole.Main => L.T("main"),
        ThreadRole.Render => L.T("render"),
        ThreadRole.Audio => L.T("audio"),
        ThreadRole.Worker => L.T("worker"),
        _ => L.T("other"),
    };

    public override void Tick()
    {
        if (_state == null) return;
        var auto = H.Auto;
        _toggle.Content = auto.On ? L.T("Turn off") : L.T("Turn on");
        _toggle.IsEnabled = auto.ButtonEnabled;
        _toggle.Background = Ui.B(auto.On ? "On" : "Panel2");
        if (_mode.Checked != auto.Top5Mode && _mode.IsEnabled) _mode.Set(auto.Top5Mode);
        if (_shownTop5 != auto.Top5Mode) BuildTable();

        string blocked = auto.Blocked;
        _state.Text = auto.State switch
        {
            AutoState.Active => L.F("Working on {0}", auto.GameExe),
            AutoState.Armed => L.T("Waiting for the game"),
            AutoState.Reverting => L.T("Putting everything back"),
            _ => L.T("Off"),
        };
        _hint.Text = auto.State == AutoState.Active ? ""
            : auto.State == AutoState.Armed ? (blocked.Length > 0 ? blocked : L.T("Start the game and keep it in front for a few seconds."))
            : L.T("Press \"Turn on\" and start the game. Nothing else has to be set.");

        if (auto.State != AutoState.Active)
        {
            _table.SetRows(Array.Empty<(object, UIElement[])>());
            _moves.Text = "";
            _log.Children.Clear();
            return;
        }
        if (auto.Top5Mode)
        {
            var view = auto.Dynamic.View();
            _table.SetRows(view.Select(s => ((object)s.Tid, new UIElement[]
            {
                Ui.Text((s.Index + 1).ToString(CultureInfo.InvariantCulture), brush: "Dim"),
                Ui.Text(s.Name),
                Ui.Text(s.Tid.ToString(CultureInfo.InvariantCulture), Ui.FsNote, "Dim"),
                Ui.Text((s.Share * 100).ToString("0", CultureInfo.CurrentCulture) + "%", Ui.FsNote, "Dim"),
                Ui.Text(s.LP.ToString(CultureInfo.InvariantCulture)),
                Ui.Text(s.Prio.HasValue ? "+" + s.Prio.Value.ToString(CultureInfo.InvariantCulture) : L.T("own"), Ui.FsNote, "Dim"),
                Ui.Text(RoleText(s.Role), Ui.FsNote, "Dim"),
            })));
            _moves.Text = view.Count == 0 ? L.T("Measuring the threads…") : L.F("{0} threads watched, {1} moves since the game started.", auto.Dynamic.Threads, auto.Dynamic.Moves);
            _log.Children.Clear();
            foreach (var line in auto.Dynamic.Log()) _log.Children.Add(Ui.Text(line, Ui.FsNote, "Dim"));
            if (_log.Children.Count == 0) _log.Children.Add(Ui.Note(L.T("No moves yet.")));
        }
        else
        {
            var plan = auto.PlanView();
            _table.SetRows(plan.Select(p => ((object)p.Tid, new UIElement[]
            {
                Ui.Text(RoleText(p.Role), brush: "Dim"),
                Ui.Text(p.Name),
                Ui.Text(p.Tid.ToString(CultureInfo.InvariantCulture), Ui.FsNote, "Dim"),
                Ui.Text(p.Pinned ? p.LP.ToString(CultureInfo.InvariantCulture) : "—"),
            })));
            _moves.Text = plan.Count == 0 ? L.T("Measuring the threads…")
                : plan[0].Pinned ? L.F("Role plan: {0} threads placed, confidence {1:0.00}.", plan.Count, auto.Confidence)
                : L.F("Role plan: the threads are not recognized well enough (confidence {0:0.00}); only priorities and the mask are set. The dynamic mode does not need thread names.", auto.Confidence);
            _log.Children.Clear();
            _log.Children.Add(Ui.Note(L.T("The role plan places the threads once and keeps them.")));
        }
    }
}
