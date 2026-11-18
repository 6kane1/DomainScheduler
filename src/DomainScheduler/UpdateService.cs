using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;

namespace DomainScheduler.App;

/// <summary>
/// Update checks (1.15.0): one minute after start and then once a day, only while the setting "CheckUpdates" is on
/// (default on). Nothing is installed by itself: the home page and the About page show a card, the tray says it once
/// per version, and the person presses "Update". While a game is running (Auto active) nothing is shown or installed.
/// </summary>
internal sealed class UpdateService
{
	public static UpdateService Instance;

	private readonly Dispatcher _ui;
	private readonly Func<bool> _inGame;
	private readonly Action<string> _notice;
	private readonly DispatcherTimer _timer;

	public UpdateCheck Last { get; private set; }
	public DateTime LastAt { get; private set; }
	public bool Checking { get; private set; }
	public string Busy { get; private set; }          // "downloading 42 %" etc., null when idle
	public string Error { get; private set; }
	public string PlaybookFile { get; private set; }  // the checked .apbx after a download

	public event Action Changed;

	public static bool Enabled => Settings.GetBool("CheckUpdates", fallback: true);

	public UpdateService(Dispatcher ui, Func<bool> inGame, Action<string> notice)
	{
		_ui = ui; _inGame = inGame; _notice = notice;
		Instance = this;
		_timer = new DispatcherTimer(TimeSpan.FromMinutes(1), DispatcherPriority.Background, delegate { Tick(); }, ui);
		_timer.Start();
	}

	public bool InGame => _inGame();

	private void Tick()
	{
		_timer.Interval = TimeSpan.FromMinutes(10);
		if (!Enabled || Checking) return;
		if (DateTime.Now - LastAt >= TimeSpan.FromHours(24)) CheckNow();
		else NotifyIfDue();
	}

	public void CheckNow()
	{
		if (Checking) return;
		Checking = true; Error = null; Fire();
		Task.Run(async () =>
		{
			var r = await Updater.CheckAsync();
			_ui.BeginInvoke(() =>
			{
				Checking = false; LastAt = DateTime.Now; Last = r;
				if (r.Error != null) { Error = r.Error; AppLog.Add(L.F("Update check: {0}", r.Error), AppLog.Sev.Info); }
				Fire();
				NotifyIfDue();
			});
		});
	}

	/// <summary>The tray says it once per version, and never during a game.</summary>
	private void NotifyIfDue()
	{
		var t = Last?.Toolbox;
		if (t == null || InGame || Settings.Get("UpdateNotified") == t.Version) return;
		Settings.Set("UpdateNotified", t.Version);
		_notice(L.F("Toolbox {0} is available. Open the program to see what is new.", AppVersion.Describe(t.Version)));
	}

	public void InstallToolbox()
	{
		var t = Last?.Toolbox;
		if (t == null || Busy != null) return;
		if (InGame) { Error = L.T("Close the game first: Toolbox does not update while a game is running."); Fire(); return; }
		Busy = L.T("Downloading…"); Error = null; Fire();
		var progress = new Progress<double>(p => { Busy = L.F("Downloading {0} %", (int)(p * 100)); Fire(); });
		Task.Run(async () =>
		{
			try
			{
				string file = await Updater.DownloadAsync(t, "DomainScheduler-" + t.Version + ".exe", progress);
				_ui.BeginInvoke(() =>
				{
					if (InGame) { Busy = null; Error = L.T("Close the game first: Toolbox does not update while a game is running."); Fire(); return; }
					Busy = L.T("Installing…"); Fire();
					((App)Application.Current).InstallUpdate(file, t.Version, err => { Busy = null; Error = err; Fire(); });
				});
			}
			catch (Exception e)
			{
				_ui.BeginInvoke(() => { Busy = null; Error = e.Message; AppLog.Add(L.F("Update: {0}", e.Message), AppLog.Sev.Warn); Fire(); });
			}
		});
	}

	/// <summary>The playbook is downloaded and checked, then shown in Explorer: AME Wizard installs it.</summary>
	public void DownloadPlaybook()
	{
		var p = Last?.Playbook;
		if (p == null || Busy != null) return;
		Busy = L.T("Downloading…"); Error = null; Fire();
		var progress = new Progress<double>(x => { Busy = L.F("Downloading {0} %", (int)(x * 100)); Fire(); });
		Task.Run(async () =>
		{
			try
			{
				string file = await Updater.DownloadAsync(p, "Domain-OS-" + p.Version + ".apbx", progress);
				_ui.BeginInvoke(() => { Busy = null; PlaybookFile = file; Fire(); ShowFile(file); });
			}
			catch (Exception e)
			{
				_ui.BeginInvoke(() => { Busy = null; Error = e.Message; Fire(); });
			}
		});
	}

	public static void ShowFile(string file)
	{
		try { Process.Start(new ProcessStartInfo("explorer.exe", "/select,\"" + file + "\"") { UseShellExecute = true }); }
		catch (Exception) { }
	}

	/// <summary>A game started or ended: the cards hide or show the Update button.</summary>
	public void Poke() => Fire();

	private void Fire() => Changed?.Invoke();
}

/// <summary>The "update available" card: on the home page (only when there is something) and on the About page (always).</summary>
internal sealed class UpdateCard
{
	public readonly Border Root;
	private readonly StackPanel _body;
	private readonly bool _always;

	public UpdateCard(bool always)
	{
		_always = always;
		_body = Ui.Col();
		Root = Ui.Card(_body);
		Root.Margin = new Thickness(0, 0, 0, 8);
		if (UpdateService.Instance != null) UpdateService.Instance.Changed += () => Root.Dispatcher.BeginInvoke(Refresh);
		Refresh();
	}

	private static string Notes(UpdateFile f)
	{
		string n = L.Lang == "ru" && f.NotesRu.Length > 0 ? f.NotesRu : f.NotesEn;
		return n.Length > 600 ? n[..600] + "…" : n;
	}

	public void Refresh()
	{
		var s = UpdateService.Instance;
		_body.Children.Clear();
		var t = s?.Last?.Toolbox; var p = s?.Last?.Playbook;
		bool show = _always || t != null || p != null;
		Root.Visibility = show && !(s?.InGame == true && !_always) ? Visibility.Visible : Visibility.Collapsed;
		if (!show) return;
		_body.Children.Add(Ui.Head(L.T("Updates")));
		string chan = AppVersion.IsTest ? L.T("Test: every new test build and every stable release") : L.T("Stable: only checked releases");
		_body.Children.Add(Ui.Text(L.F("This is {0} · channel {1}", AppVersion.Display, chan), Ui.FsNote, "Dim", wrap: true));
		if (t != null)
		{
			_body.Children.Add(Ui.Text(L.F("Toolbox {0} is available", AppVersion.Describe(t.Version)), Ui.FsBody, "Strong", bold: true, wrap: true));
			string n = Notes(t); if (n.Length > 0) _body.Children.Add(Ui.Text(n, Ui.FsNote, "Text", wrap: true));
			var b = Ui.Primary(Ui.Btn(L.T("Update"), s.InstallToolbox));
			b.IsEnabled = s.Busy == null && !s.InGame;
			_body.Children.Add(Ui.Row(b));
			if (s.InGame) _body.Children.Add(Ui.Note(L.T("Close the game first: Toolbox does not update while a game is running.")));
			else _body.Children.Add(Ui.Note(L.T("Toolbox puts back what it changed, closes and starts the new version. If the new one does not start, the previous version comes back by itself.")));
		}
		if (p != null)
		{
			_body.Children.Add(Ui.Text(L.F("Domain OS playbook {0} is available", p.Version), Ui.FsBody, "Strong", bold: true, wrap: true));
			string n = Notes(p); if (n.Length > 0) _body.Children.Add(Ui.Text(n, Ui.FsNote, "Text", wrap: true));
			if (s.PlaybookFile != null)
			{
				_body.Children.Add(Ui.Row(Ui.Btn(L.T("Show the file"), () => UpdateService.ShowFile(s.PlaybookFile))));
				_body.Children.Add(Ui.Note(L.T("Downloaded and checked. Open it with AME Wizard and press Install, the same way as the first time.")));
			}
			else
			{
				_body.Children.Add(Ui.Row(Ui.Btn(L.T("Download"), s.DownloadPlaybook)));
				_body.Children.Add(Ui.Note(L.T("The playbook is not installed by Toolbox: the file is downloaded and checked, AME Wizard installs it.")));
			}
		}
		if (s == null) return;
		if (s.Busy != null) _body.Children.Add(Ui.Text(s.Busy, Ui.FsNote, "Accent", wrap: true));
		if (s.Error != null) _body.Children.Add(Ui.Text(s.Error, Ui.FsNote, "Warn", wrap: true));
		if (_always)
		{
			string when = s.Checking ? L.T("Checking…")
				: s.LastAt == default ? L.T("Not checked yet")
				: (t == null && p == null && s.Error == null ? L.F("Up to date · checked {0:t}", s.LastAt) : L.F("Checked {0:t}", s.LastAt));
			_body.Children.Add(Ui.Text(when, Ui.FsNote, "Dim", wrap: true));
			var row = Ui.Row(Ui.Btn(L.T("Check now"), s.CheckNow));
			if (File.Exists((Environment.ProcessPath ?? "") + ".prev"))
				row.Children.Add(Ui.ConfirmBtn(L.T("Return to the previous version"), () => ((App)Application.Current).RollBack()));
			_body.Children.Add(row);
		}
	}
}
