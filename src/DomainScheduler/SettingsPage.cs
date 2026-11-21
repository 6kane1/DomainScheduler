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
		// 1.16.0 Test 1.9: updates and the tester key have their own page; the simple layout reaches it from here.
		if (S.Simple)
		{
			Button toUpd = Ui.Btn(L.T("Updates and testing"), () => S.ShowPage<UpdatePage>(), 220.0);
			toUpd.HorizontalAlignment = HorizontalAlignment.Left;
			toUpd.Margin = new Thickness(0.0, 0.0, 0.0, 8.0);
			stackPanel.Children.Add(toUpd);
		}
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
		CheckRow charts = new CheckRow(chartsLabel, L.T("Drawn only while the window is open and a game runs. Off: the numbers stay, without the line."), HomeView.LiveCharts);
		charts.Changed += delegate(bool v)
		{
			Settings.SetBool("HomeCharts", v);
			HomeView.LiveCharts = v;
			S.Did(v ? L.F("{0}: on", chartsLabel) : L.F("{0}: off", chartsLabel));
		};
		stackPanel.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("General")), checkRow, auto, charts, Ui.Row(Ui.Text(L.T("Re-apply presets every") + "  "), comboBox), Ui.Row(Ui.Text(L.T("Language") + "  "), comboBox2))));
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
		stackPanel.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("Factory reset")), Ui.Note(L.T("Puts everything back, deletes the settings, presets, autostart and data, then closes Toolbox.")), Ui.Row(Ui.ConfirmBtn(L.T("Factory reset"), delegate
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
		stackPanel.Children.Add(Ui.Note(L.T("Auto-tune: while you play (about 8 minutes) Toolbox tries the game with and without the interrupt cores and keeps the rule only if it was better every round.")));
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
		stackPanel.Children.Add(Toggle("BgOn", L.T("Lower background programs while a watched game runs"), L.T("Lower priority and power-saving mode for background programs. Audio, voice, streaming, VR, launchers and Explorer are never touched. Off puts everything back."), delegate(bool v)
		{
			if (!v)
			{
				H.Opt.Queue(delegate
				{
					H.Auto.StandaloneBackground(on: false, 0uL);
				}, skipIfBusy: false, reports: true);
			}
		}));
		stackPanel.Children.Add(Toggle("TimerGuard", L.T("Ignore timer requests of background programs while a game runs"), L.T("Background programs (RGB, monitoring, launchers) ask for a fast timer that wakes every core up to 2000 times a second. While the game runs, their requests are ignored; the game keeps its own. Off puts everything back."), delegate(bool v)
		{
			if (!v)
			{
				H.Opt.Queue(delegate
				{
					H.Engine.RevertBackground();
				}, skipIfBusy: false, reports: true);
			}
		}, fallback: true));
		// 1.16.0: the burst guard (on, soft mode by default).
		stackPanel.Children.Add(Toggle("BurstGuard", L.T("Burst guard: background programs that load the processor give way to the game"), L.T("Only a program that keeps using a third of a processor or more; it is limited to half a processor until the game ends or you switch to it. When free memory is low, the game keeps its memory. On a clean PC it does nothing."), null, fallback: true));
		stackPanel.Children.Add(Toggle("BurstGuardSoft", L.T("Burst guard: soft mode"), L.T("On: a busy background program only gives way to the game and is never stopped. Off: a hard limit of half a processor, which can pause that program for a moment."), null, fallback: true));
		stackPanel.Children.Add(Toggle("LiveIrq", L.T("Watch driver latency while a game runs (live DPC/ISR)"), L.T("While the game runs, Windows measures how long each driver works. Drivers that took 1 ms or more go to the log. Nothing is changed."), null, fallback: true));
		stackPanel.Children.Add(Toggle("LiveIrqMask", L.T("Keep the game off cores busy with interrupts"), L.T("Needs the line above. A core busy with interrupts more than 4% of the time (for 20 s) is taken away from the game until it calms down under 2%."), null, fallback: true));
		return stackPanel;
	}

	private UIElement FlagCard()
	{
		StackPanel stackPanel = Ui.Col(Ui.Head(L.T("Feature flags")), Ui.Note(L.T("Extra actions, closed by default. While closed, their buttons are hidden.")));
		(string, string)[] array = new(string, string)[2]
		{
			("devices-write", L.T("Device interrupt policy write (Devices page)")),
			("gpu-oc", L.T("GPU overclocking (NVIDIA: clock offsets and voltage boost; AMD Radeon: clocks, power limit and undervolt)"))
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
