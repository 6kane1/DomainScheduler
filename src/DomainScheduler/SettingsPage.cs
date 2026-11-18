using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal sealed class SettingsPage : Page
{
	public override string Title => L.T("Settings");

	public override string Section => "Settings";

	public SettingsPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		StackPanel stackPanel = new StackPanel();
		CheckRow checkRow = new CheckRow(L.T("Built-in system presets"), L.T("Thread priorities for Windows and common background programs (the bundled list)."), Settings.GetBool("SysPresets"));
		checkRow.Changed += delegate(bool v)
		{
			Settings.SetBool("SysPresets", v);
			H.Configure();
			S.Did(v ? L.T("Built-in presets on") : L.T("Built-in presets off: what they changed goes back on Revert or exit"));
		};
		CheckRow auto = new CheckRow(L.T("Start with Windows"), L.T("A Task Scheduler task at logon; the program starts in the tray."), value: false);
		auto.IsEnabled = false;
		Task.Run(delegate
		{
			bool exists2 = false;
			try
			{
				exists2 = Autostart.TaskExists();
			}
			catch (Exception)
			{
			}
			S.Dispatcher.BeginInvoke((Action)delegate
			{
				auto.Set(exists2);
				auto.IsEnabled = true;
			});
		});
		auto.Changed += delegate(bool v)
		{
			S.BusyTask(L.T("Writing…"), delegate
			{
				if (v)
				{
					Autostart.TaskCreate(Environment.ProcessPath ?? "");
				}
				else
				{
					Autostart.TaskDelete();
				}
				Settings.SetBool("Autostart", v);
				return Autostart.TaskExists();
			}, delegate(bool exists)
			{
				auto.Set(exists);
				S.Did(exists ? L.T("Autostart task created") : L.T("Autostart task removed"), (exists != v) ? AppLog.Sev.Error : AppLog.Sev.Info);
			});
		};
		ComboBox comboBox = Combo.New(150.0, new Opt("1000", L.F("{0} s", 1)), new Opt("2000", L.F("{0} s", 2)), new Opt("5000", L.F("{0} s", 5)), new Opt("10000", L.F("{0} s", 10)), new Opt("30000", L.F("{0} s", 30)));
		if (!Combo.Select(comboBox, Settings.Get("ReapplyMs")))
		{
			Combo.Select(comboBox, "10000");
		}
		Combo.OnChange(comboBox, delegate(string k)
		{
			Settings.Set("ReapplyMs", k);
			H.Configure();
			S.Did(L.F("Presets are re-applied every {0} s", H.ReapplyTicks));
		});
		ComboBox comboBox2 = Combo.New(150.0, L.Available.Select((string c) => new Opt(c, L.NameOf(c))).ToArray());
		Combo.Select(comboBox2, L.Lang);
		Combo.OnChange(comboBox2, delegate(string k)
		{
			if (!(k == L.Lang))
			{
				Settings.Set("Language", k);
				L.Load(k);
				S.Did(L.F("Language: {0}", L.NameOf(k)));
			}
		});
		// 1.14.0: the home screen's frame-time line is drawn 4 times a second while a game runs; weak PCs can switch it off.
		string chartsLabel = L.T("Live charts on the home screen");
		CheckRow charts = new CheckRow(chartsLabel, L.T("On by default. The frame-time line is redrawn 4 times a second, only while the window is open and a game runs. Off: the numbers stay, the line is not drawn."), HomeView.LiveCharts);
		charts.Changed += delegate(bool v)
		{
			Settings.SetBool("HomeCharts", v);
			HomeView.LiveCharts = v;
			S.Did(v ? L.F("{0}: on", chartsLabel) : L.F("{0}: off", chartsLabel));
		};
		CheckRow upd = Toggle("CheckUpdates", L.T("Check for updates"), L.T("Once a day Toolbox reads a small file from the Domain OS site (nothing about this PC is sent). A new version is shown on the home page; nothing installs until you press Update."), null, fallback: true);
		stackPanel.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("General")), checkRow, auto, charts, upd, Ui.Row(Ui.Text(L.T("Re-apply presets every") + "  "), comboBox), Ui.Row(Ui.Text(L.T("Language") + "  "), comboBox2))));
		stackPanel.Children.Add(Ui.Card(Functions()));
		stackPanel.Children.Add(Ui.Card(FlagCard()));
		StackPanel stackPanel2 = Ui.Col(Ui.Head(L.T("Watchlist")), Ui.Note(L.T("Only these programs are changed (plus the built-in presets and Auto's background). Add programs on the Processes page.")));
		foreach (string item in H.Watch.OrderBy<string, string>((string x) => x, StringComparer.OrdinalIgnoreCase).ToList())
		{
			string e = item;
			stackPanel2.Children.Add(Ui.Row(Ui.Btn("✕", delegate
			{
				H.SetWatched(e, on: false);
				S.ShowPage<SettingsPage>();
			}, 30.0), Ui.Text(e)));
		}
		List<Opt> list = (from g in Tables.KnownGames.Where((KeyValuePair<string, string> g) => !H.Watch.Contains(g.Key)).OrderBy<KeyValuePair<string, string>, string>((KeyValuePair<string, string> g) => g.Value, StringComparer.OrdinalIgnoreCase)
			select new Opt(g.Key, g.Value + " — " + g.Key)).ToList();
		if (list.Count > 0)
		{
			ComboBox pick = Combo.New(320.0, list.ToArray());
			stackPanel2.Children.Add(Ui.Row(pick, Ui.Btn(L.T("Add the known game"), delegate
			{
				H.SetWatched(Combo.Key(pick), on: true);
				S.ShowPage<SettingsPage>();
			})));
		}
		if (H.Watch.Count == 0)
		{
			stackPanel2.Children.Add(Ui.Note(L.T("The watchlist is empty: Auto will not pick a game.")));
		}
		stackPanel.Children.Add(Ui.Card(stackPanel2));
		stackPanel.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("Factory reset")), Ui.Note(L.T("Puts everything back, removes the settings, presets, the autostart task and the data folder, then closes the program.")), Ui.Row(Ui.ConfirmBtn(L.T("Factory reset"), delegate
		{
			H.Opt.RunNow(delegate
			{
				H.Auto.RevertEverything(L.T("Factory reset"));
			});
			foreach (string item2 in Maintenance.FactoryReset(H.Engine))
			{
				AppLog.Add(item2);
			}
			((App)Application.Current).ExitApp();
		})))));
		return Ui.Scroll(stackPanel);
	}

	private CheckRow Toggle(string key, string label, string hint, Action<bool> after = null, bool fallback = false)
	{
		CheckRow checkRow = new CheckRow(label, hint, Settings.GetBool(key, fallback));
		checkRow.Changed += delegate(bool v)
		{
			Settings.SetBool(key, v);
			H.Configure();
			S.Did(v ? L.F("{0}: on", label) : L.F("{0}: off", label));
			after?.Invoke(v);
		};
		return checkRow;
	}

	private UIElement Functions()
	{
		StackPanel stackPanel = Ui.Col(Ui.Head(L.T("Functions")));
		stackPanel.Children.Add(Ui.Note(L.T("Auto-tune: with Auto on and the game running, the program tries the thread modes and the interrupt-core rule for about 8 minutes while you play, measures the frame time of each, and keeps a combination only if it was better in every round. If the game ends or Auto is turned off, the previous settings stay.")));
		stackPanel.Children.Add(Ui.Row(Ui.Btn(L.T("Start auto-tune"), delegate
		{
			H.Opt.Queue(delegate
			{
				string msg = H.TunerStart();
				S.Dispatcher.BeginInvoke(() => S.Did(msg));
			}, skipIfBusy: false, reports: true);
		}, 170), Ui.Btn(L.T("Stop auto-tune"), delegate
		{
			H.Opt.Queue(delegate
			{
				H.TunerStop(L.T("stopped by the user"));
			}, skipIfBusy: false, reports: true);
		}, 170)));
		stackPanel.Children.Add(Toggle("BgOn", L.T("Lower background programs while a watched game runs"), L.T("Below-normal priority, low I/O, low memory priority, EcoQoS and the background cores. Audio, voice, streaming and VR programs, launchers and Explorer are never changed. Auto always does this. Turning it off puts everything back."), delegate(bool v)
		{
			if (!v)
			{
				H.Opt.Queue(delegate
				{
					H.Auto.StandaloneBackground(on: false, 0uL);
				}, skipIfBusy: false, reports: true);
			}
		}));
		stackPanel.Children.Add(Toggle("TimerGuard", L.T("Ignore timer requests of background programs while a game runs"), L.T("On by default. Programs and services in the background (RGB, monitoring, launchers) often ask Windows for a 0.5–1 ms timer, which makes every core wake up to 2000 times a second. Their requests are ignored while the game runs; the game's own request stays. System, audio, voice, streaming programs and browsers are never changed. Turning it off puts everything back."), delegate(bool v)
		{
			if (!v)
			{
				H.Opt.Queue(delegate
				{
					H.Engine.RevertBackground();
				}, skipIfBusy: false, reports: true);
			}
		}, fallback: true));
		stackPanel.Children.Add(Toggle("LiveIrq", L.T("Watch driver latency while a game runs (live DPC/ISR)"), L.T("On by default. While a game is handled, Windows' kernel trace measures how long every driver's DPCs and interrupts take. Drivers that took 1 ms or longer are written to the activity log once a minute. Nothing is changed; the trace stops when the game ends."), null, fallback: true));
		stackPanel.Children.Add(Toggle("LiveIrqMask", L.T("Keep the game off cores busy with interrupts"), L.T("On by default, needs the line above. A core that spends more than 4% of its time on interrupts (for 20 s in a row) is left out of the game's cores, both of its threads; it comes back when it stays under 2%. Before the first measurement the pinned devices from the registry are used, as before."), null, fallback: true));
		return stackPanel;
	}

	private UIElement FlagCard()
	{
		StackPanel stackPanel = Ui.Col(Ui.Head(L.T("Feature flags")), Ui.Note(L.T("Extended actions are closed by default. While a flag is closed, its controls are hidden and the engine refuses the action.")));
		(string, string)[] array = new(string, string)[2]
		{
			("devices-write", L.T("Device interrupt policy write (Devices page)")),
			("gpu-oc", L.T("GPU overclocking (clock offsets and voltage boost, NVIDIA)"))
		};
		for (int i = 0; i < array.Length; i++)
		{
			(string, string) tuple = array[i];
			string key = tuple.Item1;
			CheckRow checkRow = new CheckRow(tuple.Item2, key, Flags.Open(key));
			checkRow.Changed += delegate(bool v)
			{
				Flags.Set(key, v);
				H.Configure();
				S.ShowPage<SettingsPage>();
			};
			stackPanel.Children.Add(checkRow);
		}
		return stackPanel;
	}
}
