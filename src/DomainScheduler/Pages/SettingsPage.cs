using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>Switches, autostart, language, watchlist, factory reset.</summary>
sealed class SettingsPage : Page
{
    public SettingsPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("Settings");

    public override FrameworkElement Build()
    {
        var page = new StackPanel();

        var sys = new CheckRow(L.T("Built-in system presets"), L.T("Thread priorities for Windows and common background programs (the bundled list)."), Settings.GetBool("SysPresets"));
        sys.Changed += v =>
        {
            Settings.SetBool("SysPresets", v);
            H.Configure();
            S.Did(v ? L.T("Built-in presets on") : L.T("Built-in presets off: what they changed goes back on Revert or exit"));
        };
        var auto = new CheckRow(L.T("Start with Windows"), L.T("A Task Scheduler task at logon; the program starts in the tray."), false);
        auto.IsEnabled = false;
        Task.Run(() =>
        {
            bool exists = false;
            try { exists = Autostart.TaskExists(); } catch (Exception) { }
            S.Dispatcher.BeginInvoke(() =>
            {
                auto.Set(exists);
                auto.IsEnabled = true;
            });
        });
        auto.Changed += v => S.BusyTask(L.T("Writing…"), () =>
        {
            if (v) Autostart.TaskCreate(Environment.ProcessPath ?? "");
            else Autostart.TaskDelete();
            Settings.SetBool("Autostart", v);
            return Autostart.TaskExists();
        }, exists =>
        {
            auto.Set(exists);
            S.Did(exists ? L.T("Autostart task created") : L.T("Autostart task removed"), exists == v ? AppLog.Sev.Info : AppLog.Sev.Error);
        });

        var interval = Combo.New(150, new Opt("1000", L.F("{0} s", 1)), new Opt("2000", L.F("{0} s", 2)), new Opt("5000", L.F("{0} s", 5)), new Opt("10000", L.F("{0} s", 10)), new Opt("30000", L.F("{0} s", 30)));
        if (!Combo.Select(interval, Settings.Get("ReapplyMs"))) Combo.Select(interval, "10000");
        Combo.OnChange(interval, k =>
        {
            Settings.Set("ReapplyMs", k);
            H.Configure();
            S.Did(L.F("Presets are re-applied every {0} s", H.ReapplyTicks));
        });

        var lang = Combo.New(150, L.Available.Select(c => new Opt(c, L.NameOf(c))).ToArray());
        Combo.Select(lang, L.Lang);
        Combo.OnChange(lang, k =>
        {
            if (k == L.Lang) return;
            Settings.Set("Language", k);
            L.Load(k);          // the window rebuilds its texts and font
            S.Did(L.F("Language: {0}", L.NameOf(k)));
        });

        page.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("General")), sys, auto,
            Ui.Row(Ui.Text(L.T("Re-apply presets every") + "  "), interval),
            Ui.Row(Ui.Text(L.T("Language") + "  "), lang))));

        page.Children.Add(Ui.Card(Functions()));
        page.Children.Add(Ui.Card(FlagCard()));

        var watch = Ui.Col(Ui.Head(L.T("Watchlist")), Ui.Note(L.T("Only these programs are changed (plus the built-in presets and Auto's background). Add programs on the Processes page.")));
        foreach (var exe in H.Watch.OrderBy(x => x, StringComparer.OrdinalIgnoreCase).ToList())
        {
            string e = exe;
            watch.Children.Add(Ui.Row(Ui.Btn("✕", () =>
            {
                H.SetWatched(e, false);
                S.ShowPage<SettingsPage>();
            }, 30), Ui.Text(e)));
        }
        var known = Tables.KnownGames.Where(g => !H.Watch.Contains(g.Key)).OrderBy(g => g.Value, StringComparer.OrdinalIgnoreCase).Select(g => new Opt(g.Key, g.Value + " — " + g.Key)).ToList();
        if (known.Count > 0)
        {
            var pick = Combo.New(320, known.ToArray());
            watch.Children.Add(Ui.Row(pick, Ui.Btn(L.T("Add the known game"), () =>
            {
                H.SetWatched(Combo.Key(pick), true);
                S.ShowPage<SettingsPage>();
            })));
        }
        if (H.Watch.Count == 0) watch.Children.Add(Ui.Note(L.T("The watchlist is empty: Auto will not pick a game.")));
        page.Children.Add(Ui.Card(watch));

        page.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("Factory reset")),
            Ui.Note(L.T("Puts everything back, removes the settings, presets, the autostart task and the data folder, then closes the program.")),
            Ui.Row(Ui.ConfirmBtn(L.T("Factory reset"), () =>
            {
                H.Opt.RunNow(() => H.Auto.RevertEverything(L.T("Factory reset")));
                foreach (var line in Maintenance.FactoryReset(H.Engine)) AppLog.Add(line);
                ((App)Application.Current).ExitApp();
            })))));
        return Ui.Scroll(page);
    }

    /// <summary>One switch of a roadmap function: saved, handed to the layers, logged.</summary>
    CheckRow Toggle(string key, string label, string hint, Action<bool> after = null)
    {
        var row = new CheckRow(label, hint, Settings.GetBool(key));
        row.Changed += v =>
        {
            Settings.SetBool(key, v);
            H.Configure();
            S.Did(v ? L.F("{0}: on", label) : L.F("{0}: off", label));
            after?.Invoke(v);
        };
        return row;
    }

    /// <summary>Section 22: every function has its own switch; a function behind a closed flag is not shown.</summary>
    UIElement Functions()
    {
        var box = Ui.Col(Ui.Head(L.T("Functions")));
        box.Children.Add(Toggle("BgOn", L.T("Lower background programs while a watched game runs"),
            L.T("Below-normal priority, lowest I/O, low memory priority, EcoQoS and the background cores. Auto always does this. Turning it off puts everything back."),
            v => { if (!v) H.Opt.Queue(() => H.Auto.StandaloneBackground(false, 0), reports: true); }));
        box.Children.Add(Toggle("Warmup", L.T("Measure threads before a manual apply (warm-up)"),
            L.T("Two measurements 0.7 s apart, so the rules and the heat policy see which threads really work. The first apply in Auto always warms up.")));
        box.Children.Add(Toggle("ThreadSetsOn", L.T("Limit pinned threads to their processor (thread CPU Sets)"),
            L.T("A hard limit on top of the ideal processor, only for threads Auto recognized with confidence. If Windows refuses, the ideal processor stays.")));
        if (Flags.Open(Flags.TimerRes))
            box.Children.Add(Toggle("TimerResOn", L.T("Fine system timer while a game is active (Auto)"),
                L.T("Requested when Auto finds the game, released on revert or when the game ends.")));
        if (Flags.Open(Flags.EtwIrq))
            box.Children.Add(Toggle("EtwIrq", L.T("Live DPC/ISR in the mask refinement"),
                L.T("Processors busy with interrupts for more than 2% of the last 10 s are stepped around, like pinned devices.")));
        if (Flags.Open(Flags.SoftClose)) box.Children.Add(SoftCloseBox());
        return box;
    }

    UIElement SoftCloseBox()
    {
        var box = new StackPanel();
        var on = new CheckRow(L.T("Ask listed programs to close when a game starts (soft close)"),
            L.T("Only programs from your list below; they get the ordinary \"close window\" request and are never forced. Closing cannot be undone."), Settings.GetBool("SoftCloseOn"));
        on.Changed += v =>
        {
            // Every switch-on is confirmed in a dialog (22.3).
            if (v && MessageBox.Show(S, L.T("Programs from your list will be asked to close every time a game starts. Unsaved work in them may be lost if they close without asking. Turn soft close on?"),
                    "DomainScheduler", MessageBoxButton.YesNo, MessageBoxImage.Warning) != MessageBoxResult.Yes)
            {
                on.Set(false);
                return;
            }
            Settings.SetBool("SoftCloseOn", v);
            H.Configure();
            S.Did(v ? L.T("Soft close: on") : L.T("Soft close: off"));
        };
        box.Children.Add(on);
        var list = Settings.WatchList(Settings.Get("ClosableList"));
        void Save(IEnumerable<string> items)
        {
            Settings.Set("ClosableList", string.Join(", ", items));
            H.Configure();
            S.ShowPage<SettingsPage>();
        }
        foreach (var exe in list.OrderBy(x => x, StringComparer.OrdinalIgnoreCase))
        {
            string e = exe;
            box.Children.Add(Ui.Row(Ui.Btn("✕", () => Save(list.Where(x => !x.Equals(e, StringComparison.OrdinalIgnoreCase))), 30), Ui.Text(e)));
        }
        var running = H.Engine.ProcessesNow().Values.Select(p => p.Name).Distinct(StringComparer.OrdinalIgnoreCase)
            .Where(n => n.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) && !Guard.IsDenied(n) && !AutoController.IsSystem(n) && !H.Watch.Contains(n) && !list.Contains(n, StringComparer.OrdinalIgnoreCase))
            .OrderBy(n => n, StringComparer.OrdinalIgnoreCase).Select(n => new Opt(n, n)).ToArray();
        if (running.Length > 0)
        {
            var pick = Combo.New(260, running);
            box.Children.Add(Ui.Row(pick, Ui.Btn(L.T("Add to the close list"), () => Save(list.Append(Combo.Key(pick))))));
        }
        return box;
    }

    /// <summary>Feature flags (22.12): extended actions stay closed until their flag is opened here.</summary>
    UIElement FlagCard()
    {
        var box = Ui.Col(Ui.Head(L.T("Feature flags")),
            Ui.Note(L.T("Extended actions are closed by default. While a flag is closed, its controls are hidden and the engine refuses the action.")));
        (string Key, string Label)[] flags =
        {
            (Flags.DevicesWrite, L.T("Device interrupt policy write (Devices page)")),
            (Flags.SoftClose, L.T("Soft close of listed programs")),
            (Flags.EtwIrq, L.T("Live DPC/ISR tracing")),
            (Flags.TimerRes, L.T("System timer resolution")),
        };
        foreach (var (key, label) in flags)
        {
            var row = new CheckRow(label, key, Flags.Open(key));
            row.Changed += v =>
            {
                Flags.Set(key, v);
                H.Configure();
                S.ShowPage<SettingsPage>();
            };
            box.Children.Add(row);
        }
        return box;
    }
}
