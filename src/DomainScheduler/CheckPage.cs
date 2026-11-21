using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>
/// "Check" (1.16.0 Test 1): the measurement of Domain SchedTest inside Toolbox, for people who do not use a console.
/// One run = 30 s of the game: frames (FPS, 1% / 0.1% low, longest frame, hitches) and how long the game's threads waited
/// for a processor.
/// (1.16.0 Test 1.1: the automatic "3 with Auto, 3 without" comparison was removed at the owner's request.)
/// Nothing in the game is opened: kernel trace and DXGI frames only.
/// </summary>
internal sealed class CheckPage : Page
{
	public const int RunSeconds = 30, Warmup = 6, Countdown = 10;

	private StackPanel _state, _results;
	private Button _one, _stop;
	private CancellationTokenSource _cts;
	private readonly List<CheckResult> _done = new List<CheckResult>();
	private string _game = "";

	public override string Title => L.T("Check");

	public CheckPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		TextBlock title = new TextBlock
		{
			Text = L.T("Check"),
			FontSize = Ui.FsBody * 1.3,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong")
		};
		TextBlock intro = Ui.Text(L.T("Measures the game while you play: frames and how long the game's threads wait for a processor. Play the same scene the whole time (for example a deathmatch with bots). Nothing in the game is changed."), Ui.FsNote, "Dim", bold: false, wrap: true);
		intro.Margin = new Thickness(0.0, 2.0, 0.0, 12.0);
		_state = Ui.Col();
		_one = Ui.Btn(L.T("One run (30 s)"), delegate
		{
			Start();
		}, 150.0);
		Ui.Primary(_one);
		_stop = Ui.Btn(L.T("Stop"), delegate
		{
			_cts?.Cancel();
		}, 90.0);
		_stop.IsEnabled = false;
		StackPanel buttons = Ui.Row(_one, _stop);
		buttons.Margin = new Thickness(0.0, 10.0, 0.0, 0.0);
		TextBlock how = Ui.Note(L.F("Press the button, switch to the game within {0} s and play. The run lasts {1} s (plus {2} s while the trace starts); a short beep marks its start and end. To compare two variants, make the same number of runs of each.", Countdown, RunSeconds, Warmup));
		how.Margin = new Thickness(0.0, 10.0, 0.0, 0.0);
		_results = Ui.Col();
		_results.Margin = new Thickness(0.0, 14.0, 0.0, 0.0);
		DrawState(L.T("Ready."));
		return Ui.Scroll(Ui.Col(title, intro, Ui.Card(Ui.Col(_state, buttons, how), 12.0), _results));
	}

	public override void Shown()
	{
		if (_cts == null)
		{
			DrawState(L.T("Ready."));
		}
	}

	/// <summary>The game to measure: the one Auto handles, else the only running program from the watchlist.</summary>
	private (uint Pid, string Exe) FindGame()
	{
		uint pid = H.Auto.GamePid;
		if (pid != 0)
		{
			return (pid, H.Auto.GameExe);
		}
		HashSet<string> watch = new HashSet<string>(Settings.Watch(), StringComparer.OrdinalIgnoreCase);
		List<KeyValuePair<uint, (string Name, uint Parent)>> running = H.Engine.ProcessesNow().Where((KeyValuePair<uint, (string Name, uint Parent)> p) => watch.Contains(p.Value.Name)).ToList();
		return (running.Count == 1) ? (running[0].Key, running[0].Value.Name) : (0u, "");
	}

	private void DrawState(string text, string brush = "Text")
	{
		_state.Children.Clear();
		(uint Pid, string Exe) g = FindGame();
		_state.Children.Add(Ui.Text((g.Pid != 0) ? L.F("Game: {0} (PID {1})", g.Exe, g.Pid) : L.T("Game: none found. Start a game from the watchlist (or let Auto pick it up), then press a button."), 0.0, (g.Pid != 0) ? "Strong" : "Warn", bold: false, wrap: true));
		TextBlock t = Ui.Text(text, Ui.FsNote, brush, bold: false, wrap: true);
		t.Margin = new Thickness(0.0, 4.0, 0.0, 0.0);
		_state.Children.Add(t);
	}

	private void Ui_(Action a)
	{
		S.Dispatcher.BeginInvoke(a);
	}

	private void Start()
	{
		(uint Pid, string Exe) g = FindGame();
		if (g.Pid == 0)
		{
			DrawState(L.T("No game to measure."), "Warn");
			return;
		}
		_cts = new CancellationTokenSource();
		_one.IsEnabled = false;
		_stop.IsEnabled = true;
		_done.Clear();
		_game = g.Exe;
		_results.Children.Clear();
		CancellationToken ct = _cts.Token;
		Task.Run(delegate
		{
			string end;
			try
			{
				end = Single(g.Pid, ct);
			}
			catch (OperationCanceledException)
			{
				end = L.T("Stopped.");
			}
			catch (Exception ex)
			{
				end = L.F("The check stopped: {0}", ex.Message);
				AppLog.Add(end, AppLog.Sev.Warn);
			}
			Ui_(delegate
			{
				_cts = null;
				_one.IsEnabled = true;
				_stop.IsEnabled = false;
				DrawState(end);
				DrawResults();
			});
		});
	}

	private string Single(uint pid, CancellationToken ct)
	{
		CountdownToGame(ct);
		_done.Add(Measure(pid, H.Auto.GamePid == pid ? L.T("with Auto") : L.T("without Auto"), 1, ct));
		return SaveReport();
	}

	private void CountdownToGame(CancellationToken ct)
	{
		for (int s = Countdown; s > 0; s--)
		{
			Say(L.F("Switch to the game: the check starts in {0} s", s));
			Sleep(1, ct);
		}
	}

	private CheckResult Measure(uint pid, string group, int run, CancellationToken ct)
	{
		using CheckTrace trace = new CheckTrace(pid);
		if (!trace.Start())
		{
			throw new InvalidOperationException(trace.Error);
		}
		Say(L.F("{0}, run {1}: the trace starts…", group, run));
		Sleep(Warmup, ct);
		Beep(880);
		trace.Begin();
		for (int s = 1; s <= RunSeconds; s++)
		{
			Sleep(1, ct);
			Say(L.F("{0}, run {1}: {2} / {3} s", group, run, s, RunSeconds));
		}
		trace.End();
		Beep(1175);
		(long[] Waits, long[] Presents, long First, long Last) snap = trace.Snapshot();
		CheckResult r = CheckStats.From(group, run, snap.Waits, snap.Presents, snap.First, snap.Last, CheckTrace.TicksPerSecond, trace.Lost);
		AppLog.Add(L.F("Check {0} #{1}: {2:0} FPS, 1% low {3:0}, longest frame {4:0.00} ms, waits over 500 µs {5}", group, run, r.Fps, r.Low1, r.MaxMs, r.Over500));
		return r;
	}

	private static void Sleep(int seconds, CancellationToken ct)
	{
		if (ct.WaitHandle.WaitOne(seconds * 1000))
		{
			ct.ThrowIfCancellationRequested();
		}
	}

	private static void Beep(int hz)
	{
		try
		{
			Console.Beep(hz, 150);
		}
		catch (Exception)
		{
		}
	}

	private bool ProcessAlive(uint pid)
	{
		return H.Engine.ProcessesNow().ContainsKey(pid);
	}

	private void Say(string text)
	{
		Ui_(delegate
		{
			DrawState(text, "Accent");
		});
	}

	private string SaveReport()
	{
		if (_done.Count == 0)
		{
			return L.T("Nothing was measured.");
		}
		string header = L.F("Domain OS Toolbox {0} — check, {1}", AppVersion.Full, DateTime.Now.ToString("yyyy-MM-dd HH:mm", CultureInfo.InvariantCulture));
		string text = CheckStats.Report(_done, _game, header);
		try
		{
			string file = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory), "DomainOS-Check-" + DateTime.Now.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture) + ".txt");
			File.WriteAllText(file, text, new System.Text.UTF8Encoding(encoderShouldEmitUTF8Identifier: true));
			AppLog.Add(L.F("Check saved: {0}", file));
			return L.F("Done. The report is on the desktop: {0}", Path.GetFileName(file));
		}
		catch (Exception ex)
		{
			return L.F("Done, but the report could not be saved: {0}", ex.Message);
		}
	}

	private void DrawResults()
	{
		_results.Children.Clear();
		if (_done.Count == 0)
		{
			return;
		}
		foreach (CheckResult r in _done)
		{
			TextBlock head = Ui.Text(L.F("{0}, run {1}", r.Group, r.Run), 0.0, "Strong", bold: true);
			TextBlock frames = Ui.Text((r.Frames > 0) ? L.F("{0:0} FPS · 1% low {1:0} · 0.1% low {2:0} · longest frame {3:0.00} ms · hitches {4}", r.Fps, r.Low1, r.Low01, r.MaxMs, r.Hitches) : L.T("No frames seen (the game does not use DirectX 10/11/12, or it was not in front)."), Ui.FsNote, "Text", bold: false, wrap: true);
			TextBlock waits = Ui.Text(L.F("Game threads waited for a processor: p99 {0:0.0} µs · p99.9 {1:0.0} µs · longest {2:0} µs · over 500 µs: {3}", r.WaitP99Us, r.WaitP999Us, r.WaitMaxUs, r.Over500), Ui.FsNote, "Text", bold: false, wrap: true);
			_results.Children.Add(Ui.Card(Ui.Col(head, frames, waits), 10.0));
		}
	}
}
