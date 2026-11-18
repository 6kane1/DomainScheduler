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

	private readonly Hub _hub = new Hub();

	private Shell _window;

	private Tray _tray;

	private ErrorPopup _errors;

	private int _shutdown;

	protected override void OnStartup(StartupEventArgs e)
	{
		base.OnStartup(e);
		if (e.Args.Contains("--revert-devices"))
		{
			// Without the window and without the single-instance check: usable from Safe Mode or a script.
			L.Load(Settings.Language());
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
		_errors = new ErrorPopup(base.Dispatcher, () => _hub.Auto?.State == AutoState.Active, () => _window, t => _tray?.Notice(t));
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
		_hub.Auto.SetTop5(Settings.GetBool("Top5On"));
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
		_hub.Auto.Changed += delegate
		{
			base.Dispatcher.BeginInvoke(new Action(_window.Refresh));
			base.Dispatcher.BeginInvoke(new Action(() => UpdateService.Instance?.Poke()));
		};
		_hub.Opt.Start();
		_tray = new Tray(ShowWindow, ExitApp);
		new UpdateService(base.Dispatcher, () => _hub.Auto?.State == AutoState.Active, t => _tray?.Info(t));
		AfterUpdate(e.Args);
		_showEvent = new EventWaitHandle(initialState: false, EventResetMode.AutoReset, "Local\\DomainScheduler.Show");
		Thread thread = new Thread((ThreadStart)delegate
		{
			while (_showEvent.WaitOne())
			{
				if (Volatile.Read(ref _shutdown) == 0)
				{
					base.Dispatcher.BeginInvoke(new Action(ShowWindow));
				}
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
			_hub.Opt?.RunNow(delegate
			{
				_hub.Auto.RevertEverything(L.T("Exit"));
			});
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Revert at exit: {0}", ex.Message), AppLog.Sev.Error);
		}
		_hub.Opt?.Dispose();
		_hub.Frames?.Dispose();
		_hub.Irq.Dispose();
		try { string d = DefenderGameExclusion.RemoveAll(); if (d.Length > 0) AppLog.Add(d); } catch (Exception) { }
		try { string p = GamePower.Restore(); if (p.Length > 0) AppLog.Add(p); } catch (Exception) { }
		_tray?.Dispose();
		_window?.SaveGeometry();
		AppLog.Flush();
		return true;
	}

	private const string UpdateOkName = "Local\\DomainScheduler.UpdateOk";

	/// <summary>
	/// Installs a downloaded and checked Toolbox (1.15.0): the program file is swapped (the running one becomes
	/// "DomainScheduler.exe.prev"), everything is put back as on exit, and the new version is started. The new one
	/// signals when it runs fine (after 20 s); if it closes before that, the previous file is put back and started again. Never called while a game is running (the card refuses it).
	/// </summary>
	public void InstallUpdate(string file, string version, Action<string> failed)
	{
		string exe = Environment.ProcessPath ?? "";
		try
		{
			Updater.Swap(exe, file);
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Update: the program file could not be replaced: {0}", ex.Message), AppLog.Sev.Warn);
			failed(L.F("The program file could not be replaced: {0}. Nothing was changed.", ex.Message));
			return;
		}
		string from = AppVersion.Full;
		AppLog.Add(L.F("Update: {0} → {1}, starting the new version", from, version));
		Settings.Set("UpdateFrom", from);
		if (!CleanUp())
		{
			return;
		}
		_window?.Hide();
		EventWaitHandle ok = new EventWaitHandle(initialState: false, EventResetMode.ManualReset, UpdateOkName);
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
						break;
					}
				}
			}
			catch (Exception ex2)
			{
				AppLog.Add(L.F("Update: the new version did not start: {0}", ex2.Message), AppLog.Sev.Warn);
			}
			// Still running after 90 s without the signal: it is left alone (nothing in Toolbox ends a process) and
			// counts as started. Only a new version that closed by itself is replaced with the previous one.
			if (!good && p != null && !p.HasExited)
			{
				good = true;
				AppLog.Add(L.F("Update: {0} runs but did not confirm its start", version), AppLog.Sev.Warn);
			}
			if (!good)
			{
				try
				{
					Updater.SwapBack(exe);
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

	/// <summary>"Return to the previous version" on the About page: the same swap, backwards.</summary>
	public void RollBack()
	{
		string exe = Environment.ProcessPath ?? "";
		string prev = exe + ".prev";
		if (!File.Exists(prev))
		{
			return;
		}
		string tmp = Path.Combine(Updater.Dir, "DomainScheduler-previous.exe");
		try
		{
			Directory.CreateDirectory(Updater.Dir);
			File.Copy(prev, tmp, overwrite: true);
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Rollback: {0}", ex.Message), AppLog.Sev.Warn);
			return;
		}
		InstallUpdate(tmp, "previous", delegate (string err) { AppLog.Add(err, AppLog.Sev.Warn); });
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
