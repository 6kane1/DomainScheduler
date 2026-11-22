using System;
using System.CodeDom.Compiler;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;

namespace DomainScheduler.App;

public partial class App : Application
{
	private const string MutexName = "Local\\DomainScheduler.Instance";

	private const string ShowEventName = "Local\\DomainScheduler.Show";

	private Mutex _mutex;

	private EventWaitHandle _showEvent;

	private AutoCommand _autoCmd;

	private readonly Hub _hub = new Hub();

	private Shell _window;

	private Tray _tray;

	private ErrorPopup _errors;

	private int _shutdown;

	/// <summary>Started by an update (not the rolled-back one): its clean exit tells the old version all is well.</summary>
	private bool _afterUpdate;

	/// <summary>
	/// Before anything reads its state (1.15.0 Test 1.3): the admin-only state folder is checked (and made safe),
	/// an older version's data in the user's own folder is taken over once (only what passes the checks), and the
	/// settings that moved to HKLM are copied over once. Everything is logged; nothing here stops the start.
	/// </summary>
	private static void PrepareState()
	{
		try
		{
			_ = Store.Dir;
		}
		catch (Exception ex)
		{
			AppLog.Add(ex.Message, AppLog.Sev.Error);
		}
		foreach (string note in StateDir.TakeNotes())
		{
			AppLog.Add(note, AppLog.Sev.Warn);
		}
		string account = "";
		try
		{
			account = System.Security.Principal.WindowsIdentity.GetCurrent().User?.Value ?? "";
		}
		catch (Exception)
		{
		}
		try
		{
			foreach (var (text, sev) in LegacyImport.Run(Store.LegacyDir, account))
			{
				AppLog.Add(text, sev);
			}
		}
		catch (Exception ex2)
		{
			AppLog.Add(L.F("Taking over the data of the older version: {0}", ex2.Message), AppLog.Sev.Error);
		}
		try
		{
			foreach (string line in RegistrySettingsBackend.MigrateToMachine(account.Length > 0 ? account : Environment.UserName))
			{
				AppLog.Add(line, AppLog.Sev.Warn);
			}
		}
		catch (Exception ex3)
		{
			AppLog.Add(L.F("Settings migration: {0}", ex3.Message), AppLog.Sev.Warn);
		}
	}

	protected override void OnStartup(StartupEventArgs e)
	{
		base.OnStartup(e);
		L.Load(Settings.Language());
		PrepareState();
		if (e.Args.Contains("--revert-devices"))
		{
			// Without the window and without the single-instance check: usable from Safe Mode or a script.
			string err;
			try
			{
				err = Devices.RevertAll();
			}
			catch (Exception ex)
			{
				err = ex.Message;
			}
			AppLog.Add((err == null) ? L.T("Device interrupt changes reverted (--revert-devices); restart Windows") : L.F("Devices: {0}", err), (err == null) ? AppLog.Sev.Info : AppLog.Sev.Error);
			AppLog.Flush();
			Shutdown((err == null) ? 0 : 1);
			return;
		}
		// --auto on|off|status (1.16.0-test1.3): for Domain Bench. Talks to the running Toolbox and exits; never starts one.
		// --auto-mask on|off (1.16.0-test1.4, test setting AutoNoMask): off = Auto keeps everything else but gives the game
		// every processor. Read by the running Toolbox at its next pass; switch it while Auto is off.
		int maskArg = Array.IndexOf(e.Args, "--auto-mask");
		if (maskArg >= 0)
		{
			string v = maskArg + 1 < e.Args.Length ? e.Args[maskArg + 1].ToLowerInvariant() : "";
			if (v is "on" or "off" or "auto") { Settings.Set("AutoMask", v); Settings.SetBool("AutoNoMask", false); AppLog.Add("Auto mask: " + v + " (--auto-mask)", AppLog.Sev.Info); AppLog.Flush(); }
			Shutdown(v is "on" or "off" or "auto" ? 0 : 4);
			return;
		}
		int autoArg = Array.IndexOf(e.Args, "--auto");
		if (autoArg >= 0)
		{
			Shutdown(AutoCommand.Send(autoArg + 1 < e.Args.Length ? e.Args[autoArg + 1] : "status"));
			return;
		}
		_mutex = new Mutex(initiallyOwned: true, "Local\\DomainScheduler.Instance", out var createdNew);
		// After an update the old version is still closing: wait for it to let go (up to 60 s).
		if (!createdNew && e.Args.Contains("--after-update"))
		{
			try
			{
				createdNew = _mutex.WaitOne(TimeSpan.FromSeconds(60));
			}
			catch (AbandonedMutexException)
			{
				createdNew = true;
			}
		}
		if (!createdNew)
		{
			try
			{
				EventWaitHandle.OpenExisting("Local\\DomainScheduler.Show").Set();
			}
			catch (Exception)
			{
			}
			Shutdown();
			return;
		}
		try
		{
			Process.GetCurrentProcess().PriorityClass = ProcessPriorityClass.BelowNormal;
		}
		catch (Exception)
		{
		}
		try
		{
			RegistrySettingsBackend.MigrateFromLegacy();
		}
		catch (Exception ex3)
		{
			AppLog.Add(L.F("Settings migration: {0}", ex3.Message), AppLog.Sev.Warn);
		}
		RenderOptions.ProcessRenderMode = RenderMode.SoftwareOnly;
		L.Load(Settings.Language());
		Theme.Apply(this);
		// From here on every error is shown in a window (there is no on-screen journal).
		// "A game runs": Auto has it active, or the last game it laid out still runs (after an error reverted
		// everything, the window must not open over that game either).
		_errors = new ErrorPopup(base.Dispatcher, () => _hub.Auto != null && (_hub.Auto.State == AutoState.Active || _hub.Auto.LastGameRunning), () => _window, t => _tray?.Notice(t));
		Engine engine = new Engine(new WinNative());
		engine.GateHook = (uint pid, string exe) => Guard.Gate(exe, Guard.Op.Process, isProtected: false, engine.ProcessesNow().Values.Select(((string Name, uint Parent) v) => v.Name));
		_hub.Engine = engine;
		if (e.Args.Any((string a) => (a == "--factory-reset" || a == "--uninstall") ? true : false))
		{
			foreach (string item in Maintenance.FactoryReset(engine))
			{
				AppLog.Add(item);
			}
			AppLog.Flush();
			Shutdown();
			return;
		}
		base.DispatcherUnhandledException += delegate(object _, DispatcherUnhandledExceptionEventArgs a)
		{
			AppLog.Add(L.F("Unhandled error: {0}", AppLog.Describe(a.Exception)), AppLog.Sev.Error);
			a.Handled = true;
			EmergencyRevert();
		};
		AppDomain.CurrentDomain.UnhandledException += delegate(object _, UnhandledExceptionEventArgs a)
		{
			AppLog.Add(L.F("Unhandled error: {0}", AppLog.Describe(a.ExceptionObject as Exception)), AppLog.Sev.Error);
			EmergencyRevert();
		};
		TaskScheduler.UnobservedTaskException += delegate(object? _, UnobservedTaskExceptionEventArgs a)
		{
			AppLog.Add(L.F("Unhandled error: {0}", AppLog.Describe(a.Exception)), AppLog.Sev.Error);
			a.SetObserved();
		};
		base.SessionEnding += delegate
		{
			ExitApp();
		};
		GameMask.IrqSource = Devices.PinnedIrqs;
		EtwIrq.StopOrphan();
		BlackBox.StopOrphan();
		ReadyProbe.StopOrphan();
		EtwIrq.StopOrphanSched();
		System.Threading.Tasks.Task.Run(SafetyAtStart);
		System.Threading.Tasks.Task.Run(() =>
		{
			try { string s = DefenderGameExclusion.RemoveAll(); if (s.Length > 0) AppLog.Add(s); } catch (Exception) { }
			try { string p = GamePower.Restore(); if (p.Length > 0) AppLog.Add(p); } catch (Exception) { }   // power values a test build left behind
		});
		_hub.Frames = new EtwFrames();
		_hub.Frames.Start();
		if (!_hub.Frames.Available)
		{
			AppLog.Add(L.F("Frame time is not measured: {0}", _hub.Frames.Error), AppLog.Sev.Warn);
		}
		_hub.Probe = new WinGameProbe();
		_hub.Auto = new AutoController(engine, _hub.Probe, _hub.Frames)
		{
			WatchList = () => _hub.Watch
		};
		// 1.16.0-test1.5: on 8 cores or fewer the game keeps every processor (AutoLogic.KeepsAllCores); setting AutoMask
		// auto|on|off, --auto-mask. The test1.4 setting AutoNoMask=true still means "off".
		var topoAtStart = TopoBuilder.Live();
		string MaskSetting() => Settings.GetBool("AutoNoMask") ? "off" : (Settings.Get("AutoMask") ?? "auto");
		_hub.Auto.MaskFilter = mp => AutoLogic.KeepsAllCores(topoAtStart, MaskSetting()) ? AutoLogic.AllCores(topoAtStart, mp) : mp;
		AppLog.Add(AutoLogic.KeepsAllCores(topoAtStart, MaskSetting())
			? L.F("Auto: the game keeps every processor ({0} cores; setting {1})", topoAtStart?.PhysicalCount ?? 0, MaskSetting())
			: L.F("Auto: the game mask is used ({0} cores; setting {1})", topoAtStart?.PhysicalCount ?? 0, MaskSetting()), AppLog.Sev.Info);
		_hub.Sys = new SysLayer(engine);
		_hub.SelfTest = ThreadIni.SelfTest();
		_hub.Auto.SelfTestProblems = _hub.SelfTest;
		foreach (string item2 in _hub.SelfTest)
		{
			AppLog.Add("SelfTest: " + item2, AppLog.Sev.Error);
		}
		SessionData sessionData = SessionFile.Read();
		if (sessionData != null)
		{
			if (!sessionData.CleanShutdown)
			{
				_hub.Auto.RecoverCrashedSession(sessionData);
			}
			SessionFile.Delete();
		}
		Autostart.EnsureOnFirstRun(Environment.ProcessPath ?? "");
		try
		{
			_hub.LoadPresets();
		}
		catch (Exception ex4)
		{
			AppLog.Add(L.F("Presets: {0}", ex4.Message), AppLog.Sev.Error);
		}
		_hub.Configure();
		_hub.Auto.SetTop5(true);   // 1.16.0 Test 1.9: dynamic Top-5 is the only thread mode (the role plan is gone)
		_hub.Auto.SelfLoadLine = _hub.SelfLine;
		try
		{
			_hub.GpuOcStartup();
		}
		catch (Exception ex5)
		{
			AppLog.Add(L.F("GPU overclock: {0}", ex5.Message), AppLog.Sev.Warn);
		}
		_hub.Opt = new Optimizer(base.Dispatcher, _hub);
		_window = new Shell(_hub);
		_hub.Opt.Evaluate += _window.Refresh;
		_autoCmd = new AutoCommand(() => _hub.Auto.On, () => base.Dispatcher.Invoke(() => { if (!_hub.Auto.On) _hub.Auto.Click(); }),
			() => base.Dispatcher.Invoke(() => { if (_hub.Auto.On) _hub.Auto.Click(); }));
		_hub.Auto.Changed += () => _autoCmd?.Mirror();
		_hub.Auto.Changed += delegate
		{
			base.Dispatcher.BeginInvoke(new Action(_window.Refresh));
			base.Dispatcher.BeginInvoke(new Action(() => UpdateService.Instance?.Poke()));   // redraws the cards only when a game started or ended
		};
		_hub.Opt.Start();
		_tray = new Tray(ShowWindow, ExitApp);
		new UpdateService(base.Dispatcher, () => _hub.Auto?.State == AutoState.Active, t => _tray?.Info(t));
		AfterUpdate(e.Args);
		_showEvent = new EventWaitHandle(initialState: false, EventResetMode.AutoReset, "Local\\DomainScheduler.Show");
		Thread thread = new Thread((ThreadStart)delegate
		{
			while (true)
			{
				EventWaitHandle ev = Volatile.Read(ref _showEvent);
				if (ev == null)
				{
					break;
				}
				try
				{
					if (!ev.WaitOne())
					{
						break;
					}
				}
				catch (ObjectDisposedException)
				{
					break;
				}
				if (Volatile.Read(ref _shutdown) != 0)
				{
					break;
				}
				base.Dispatcher.BeginInvoke(new Action(ShowWindow));
			}
		});
		thread.IsBackground = true;
		thread.Name = "show-listener";
		thread.Start();
		if (!e.Args.Contains("--autostart"))
		{
			ShowWindow();
		}
		AppLog.Add(L.F("Started, CPU: {0}", Native.CpuName()));
		// Unpacked libraries of other builds (an update leaves the old ones): removed once the window's are in use.
		System.Threading.Tasks.Task.Run(async () =>
		{
			await System.Threading.Tasks.Task.Delay(TimeSpan.FromMinutes(1));
			NativeHome.CleanOld();
		});
	}

	/// <summary>
	/// At every start, off the UI thread: the device boot guard (reverts the interrupt changes when Windows did not
	/// start fine after them), values older versions wrote and 1.12.0 no longer writes, and the removed protection
	/// switches put back from their backups. Then the device changes are confirmed after 2 minutes of uptime.
	/// </summary>
	private static void SafetyAtStart()
	{
		try
		{
			foreach (string line in BootGuard.OnStart(Devices.RevertAll))
			{
				AppLog.Add(line, AppLog.Sev.Warn);
			}
			GameMask.InvalidateIrq();
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Device boot guard: {0}", ex.Message), AppLog.Sev.Error);
		}
		try
		{
			// First start of 1.12.0: an old automatic layout is reverted and the safe one written again (boot guard on).
			if (!Settings.GetBool("SafeDeviceLayout112"))
			{
				Settings.SetBool("SafeDeviceLayout112", value: true);
				foreach (var (text, ok) in IrqAuto.RedoOldLayout())
				{
					AppLog.Add(text, ok ? AppLog.Sev.Warn : AppLog.Sev.Error);
				}
				GameMask.InvalidateIrq();
			}
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Device layout update: {0}", ex.Message), AppLog.Sev.Error);
		}
		try
		{
			bool guess = !Settings.GetBool("LegacyRawMouseChecked");
			foreach (string line in IrqAuto.CleanupLegacy(guess))
			{
				AppLog.Add(line);
			}
			Settings.SetBool("LegacyRawMouseChecked", value: true);
		}
		catch (Exception)
		{
		}
		try
		{
			foreach (string line in Tweaks.RevertRetired())
			{
				AppLog.Add(line, AppLog.Sev.Warn);
			}
		}
		catch (Exception)
		{
		}
		try
		{
			if (BootGuard.Load()?.Pending != true)
			{
				return;
			}
			TimeSpan wait = BootGuard.ConfirmUptime - TimeSpan.FromMilliseconds(Native.GetTickCount64()) + TimeSpan.FromSeconds(1);
			if (wait > TimeSpan.Zero)
			{
				Thread.Sleep(wait);
			}
			string confirmed = BootGuard.ConfirmIfDue(TimeSpan.FromMilliseconds(Native.GetTickCount64()));
			if (confirmed != null)
			{
				AppLog.Add(confirmed);
			}
		}
		catch (Exception)
		{
		}
	}

	private void ShowWindow()
	{
		_window.Show();
		if (_window.WindowState == WindowState.Minimized)
		{
			_window.WindowState = WindowState.Normal;
		}
		_window.Activate();
	}

	public void ExitApp()
	{
		if (CleanUp())
		{
			Shutdown();
		}
	}

	/// <summary>Everything the program changed is put back and its parts stopped (exit and update). False when already done.</summary>
	private bool CleanUp()
	{
		if (Interlocked.Exchange(ref _shutdown, 1) != 0)
		{
			return false;
		}
		try
		{
			bool finished = _hub.Opt?.RunNow(delegate
			{
				_hub.Auto.RevertEverything(L.T("Exit"));
			}) ?? true;
			if (!finished)
			{
				// The session file is then not marked clean: the next start puts back whatever is left.
				AppLog.Add(L.T("Revert at exit did not finish in time: what is left is put back at the next start"), AppLog.Sev.Warn);
			}
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Revert at exit: {0}", ex.Message), AppLog.Sev.Error);
		}
		// The show event goes now: a new version starting after this one gets the "show the window" signal itself.
		EventWaitHandle show = Interlocked.Exchange(ref _showEvent, null);
		if (show != null)
		{
			try
			{
				show.Set();
			}
			catch (Exception)
			{
			}
			show.Dispose();
		}
		// A version started by an update that closes cleanly (the user chose Exit) is not a failed update.
		if (_afterUpdate)
		{
			try
			{
				using EventWaitHandle ok = EventWaitHandle.OpenExisting(UpdateOkName);
				ok.Set();
			}
			catch (Exception)
			{
			}
		}
		_hub.Opt?.Dispose();
		_hub.Frames?.Dispose();
		_hub.Irq.Dispose();
		_hub.Box.Dispose();
		_hub.Waits.Dispose();
		try { string d = DefenderGameExclusion.RemoveAll(); if (d.Length > 0) AppLog.Add(d); } catch (Exception) { }
		try { string p = GamePower.Restore(); if (p.Length > 0) AppLog.Add(p); } catch (Exception) { }
		_tray?.Dispose();
		_window?.SaveGeometry();
		AppLog.Flush();
		return true;
	}

	private const string UpdateOkName = "Local\\DomainScheduler.UpdateOk";

	/// <summary>
	/// Installs a downloaded and checked Toolbox (1.15.0): everything is put back as on exit first; only then the
	/// program file is swapped (the running one becomes "DomainScheduler.exe.prev", the checked bytes are written as the
	/// program file) and the new version is started. The new one signals when it runs fine (after 20 s, or when it is
	/// closed cleanly before that); if it ends with an error before that, the previous file is put back and started
	/// again. Never called while a game is running (the card refuses it).
	/// </summary>
	public void InstallUpdate(byte[] content, string version, Action<string> failed)
	{
		string exe = Environment.ProcessPath ?? "";
		Handover(() => Updater.Swap(exe, content), () => Updater.SwapBack(exe), version, failed);
	}

	/// <summary>"Return to the previous version" on the About page: the ".prev" file is swapped in directly (nothing is copied anywhere).</summary>
	public void RollBack()
	{
		string exe = Environment.ProcessPath ?? "";
		if (!File.Exists(exe + ".prev"))
		{
			return;
		}
		Handover(() => Updater.SwapToPrevious(exe), () => Updater.UndoSwapToPrevious(exe), "previous", delegate(string err)
		{
			AppLog.Add(err, AppLog.Sev.Warn);
		});
	}

	private void Handover(Action swap, Action swapBack, string version, Action<string> failed)
	{
		string exe = Environment.ProcessPath ?? "";
		string from = AppVersion.Full;
		if (!CleanUp())
		{
			return;
		}
		try
		{
			swap();
		}
		catch (Exception ex)
		{
			// Everything was already put back and stopped: the same version starts again (no change on disk).
			AppLog.Add(L.F("Update: the program file could not be replaced: {0}", ex.Message), AppLog.Sev.Warn);
			failed(L.F("The program file could not be replaced: {0}. Nothing was changed.", ex.Message));
			RestartSame(exe);
			return;
		}
		AppLog.Add(L.F("Update: {0} → {1}, starting the new version", from, version));
		Settings.Set("UpdateFrom", from);
		AppLog.Flush();
		_window?.Hide();
		EventWaitHandle ok = new EventWaitHandle(initialState: false, EventResetMode.ManualReset, UpdateOkName);
		ok.Reset();      // an event left signalled by an earlier hand-over must not count for this one
		try
		{
			_mutex?.ReleaseMutex();
		}
		catch (Exception)
		{
		}
		Thread thread = new Thread((ThreadStart)delegate
		{
			bool good = false;
			Process p = null;
			try
			{
				p = Process.Start(new ProcessStartInfo(exe, "--after-update") { UseShellExecute = false });
				for (int i = 0; i < 180 && !good; i++)
				{
					good = ok.WaitOne(500);
					if (!good && p != null && p.HasExited)
					{
						// A clean exit (code 0) is the user closing it, or a second copy handing over: not a failed start.
						good = ok.WaitOne(0) || p.ExitCode == 0;
						break;
					}
				}
			}
			catch (Exception ex2)
			{
				AppLog.Add(L.F("Update: the new version did not start: {0}", ex2.Message), AppLog.Sev.Warn);
			}
			// Still running after 90 s without the signal: it is left alone (nothing in Toolbox ends a process) and
			// counts as started. Only a new version that ended with an error is replaced with the previous one.
			if (!good && p != null && !p.HasExited)
			{
				good = true;
				AppLog.Add(L.F("Update: {0} runs but did not confirm its start", version), AppLog.Sev.Warn);
			}
			if (!good)
			{
				try
				{
					swapBack();
					AppLog.Add(L.F("Update: {0} did not start, the previous version was put back", version), AppLog.Sev.Warn);
					AppLog.Flush();
					Process.Start(new ProcessStartInfo(exe, "--after-update --rolled-back") { UseShellExecute = false });
				}
				catch (Exception ex3)
				{
					AppLog.Add(L.F("Update: putting the previous version back failed: {0}", ex3.Message), AppLog.Sev.Error);
					AppLog.Flush();
				}
			}
			base.Dispatcher.BeginInvoke(new Action(Shutdown));
		});
		thread.IsBackground = false;
		thread.Name = "update";
		thread.Start();
	}

	/// <summary>After a failed swap (everything already stopped): the unchanged program starts again and this copy closes.</summary>
	private void RestartSame(string exe)
	{
		try
		{
			_mutex?.ReleaseMutex();
		}
		catch (Exception)
		{
		}
		try
		{
			AppLog.Flush();
			Process.Start(new ProcessStartInfo(exe, "--after-update --update-failed") { UseShellExecute = false });
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Update: {0}", ex.Message), AppLog.Sev.Error);
		}
		base.Dispatcher.BeginInvoke(new Action(Shutdown));
	}

	/// <summary>Started by an update: tell the old version all is well (after 20 s of running), say what happened.</summary>
	private void AfterUpdate(string[] args)
	{
		if (!args.Contains("--after-update"))
		{
			return;
		}
		if (args.Contains("--rolled-back"))
		{
			AppLog.Add(L.T("The update did not start: the previous version was put back"), AppLog.Sev.Warn);
			_tray?.Info(L.T("The update did not start, so the previous version was put back. Nothing else changed."));
			return;
		}
		if (args.Contains("--update-failed"))
		{
			_tray?.Info(L.T("The update could not be installed: the program file could not be replaced. Nothing was changed."));
			return;
		}
		_afterUpdate = true;
		DispatcherTimer t = new DispatcherTimer
		{
			Interval = TimeSpan.FromSeconds(20)
		};
		t.Tick += delegate
		{
			t.Stop();
			try
			{
				using EventWaitHandle ok = EventWaitHandle.OpenExisting(UpdateOkName);
				ok.Set();
			}
			catch (Exception)
			{
			}
			string from = Settings.Get("UpdateFrom");
			AppLog.Add(L.F("Updated: {0} → {1}", from ?? "?", AppVersion.Full));
			_tray?.Info(L.F("Toolbox was updated to {0}.", AppVersion.Display));
		};
		t.Start();
	}

	private void EmergencyRevert()
	{
		try
		{
			_hub.Auto?.RevertEverything(L.T("Error"));
		}
		catch (Exception)
		{
		}
		AppLog.Flush();
	}

	protected override void OnExit(ExitEventArgs e)
	{
		_mutex?.Dispose();
		base.OnExit(e);
	}
}
