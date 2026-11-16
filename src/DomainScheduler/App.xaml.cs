using System.Windows;
using System.Windows.Threading;

namespace DomainScheduler.App;

public partial class App : Application
{
    const string MutexName = @"Local\DomainScheduler.Instance", ShowEventName = @"Local\DomainScheduler.Show";

    Mutex _mutex;
    EventWaitHandle _showEvent;
    readonly Hub _hub = new();
    Shell _window;
    Tray _tray;
    int _shutdown;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        _mutex = new Mutex(true, MutexName, out bool first);
        if (!first)
        {
            try { EventWaitHandle.OpenExisting(ShowEventName).Set(); }
            catch (Exception) { }
            Shutdown();
            return;
        }
        try { RegistrySettingsBackend.MigrateFromLegacy(); }
        catch (Exception ex) { AppLog.Add(L.F("Settings migration: {0}", ex.Message), AppLog.Sev.Warn); }
        L.Load(Settings.Language());
        Theme.Apply(this);

        var engine = new Engine(new WinNative());
        engine.GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false, engine.ProcessesNow().Values.Select(v => v.Name));
        _hub.Engine = engine;

        if (e.Args.Any(a => a is "--factory-reset" or "--uninstall"))
        {
            foreach (var line in Maintenance.FactoryReset(engine)) AppLog.Add(line);
            AppLog.Flush();
            Shutdown();
            return;
        }

        DispatcherUnhandledException += (_, a) =>
        {
            AppLog.Add(L.F("Unhandled error: {0}", a.Exception.Message), AppLog.Sev.Error);
            a.Handled = true;
            EmergencyRevert();
        };
        AppDomain.CurrentDomain.UnhandledException += (_, a) =>
        {
            AppLog.Add(L.F("Unhandled error: {0}", (a.ExceptionObject as Exception)?.Message), AppLog.Sev.Error);
            EmergencyRevert();
        };
        TaskScheduler.UnobservedTaskException += (_, a) =>
        {
            AppLog.Add(L.F("Unhandled error: {0}", a.Exception.Message), AppLog.Sev.Error);
            a.SetObserved();
        };
        SessionEnding += (_, _) => ExitApp();

        GameMask.IrqSource = Devices.PinnedIrqs;
        _hub.Frames = new EtwFrames();
        _hub.Frames.Start();
        if (!_hub.Frames.Available) AppLog.Add(L.F("Frame time is not measured: {0}", _hub.Frames.Error), AppLog.Sev.Warn);
        _hub.Probe = new WinGameProbe();
        _hub.Auto = new AutoController(engine, _hub.Probe, _hub.Frames) { WatchList = () => _hub.Watch };
        _hub.Sys = new SysLayer(engine);

        // SelfTest of the built-in presets: problems go to the log and the report; the run is not blocked.
        _hub.SelfTest = ThreadIni.SelfTest();
        _hub.Auto.SelfTestProblems = _hub.SelfTest;
        foreach (var p in _hub.SelfTest) AppLog.Add("SelfTest: " + p, AppLog.Sev.Error);

        // A previous run that did not finish: put back what it changed.
        var session = SessionFile.Read();
        if (session != null)
        {
            if (!session.CleanShutdown) _hub.Auto.RecoverCrashedSession(session);
            SessionFile.Delete();
        }
        Autostart.EnsureOnFirstRun(Environment.ProcessPath ?? "");

        try { _hub.LoadPresets(); }
        catch (Exception ex) { AppLog.Add(L.F("Presets: {0}", ex.Message), AppLog.Sev.Error); }
        _hub.Configure();
        _hub.Auto.SetTop5(Settings.GetBool("Top5On"));

        _hub.Opt = new Optimizer(Dispatcher, _hub);
        _window = new Shell(_hub);
        _hub.Opt.Evaluate += _window.Refresh;
        _hub.Auto.Changed += () => Dispatcher.BeginInvoke(_window.Refresh);
        _hub.Opt.Start();
        _tray = new Tray(ShowWindow, ExitApp);

        _showEvent = new EventWaitHandle(false, EventResetMode.AutoReset, ShowEventName);
        var listen = new Thread(() =>
        {
            while (_showEvent.WaitOne())
                if (Volatile.Read(ref _shutdown) == 0) Dispatcher.BeginInvoke(ShowWindow);
        }) { IsBackground = true, Name = "show-listener" };
        listen.Start();

        if (!e.Args.Contains(Autostart.Arg)) ShowWindow();
        AppLog.Add(L.F("Started, CPU: {0}", Native.CpuName()));
    }

    void ShowWindow()
    {
        _window.Show();
        if (_window.WindowState == WindowState.Minimized) _window.WindowState = WindowState.Normal;
        _window.Activate();
    }

    /// <summary>Exit from the tray, Windows session end: everything back, then quit.</summary>
    public void ExitApp()
    {
        if (Interlocked.Exchange(ref _shutdown, 1) != 0) return;
        try { _hub.Opt?.RunNow(() => _hub.Auto.RevertEverything(L.T("Exit"))); }
        catch (Exception ex) { AppLog.Add(L.F("Revert at exit: {0}", ex.Message), AppLog.Sev.Error); }
        _hub.Opt?.Dispose();
        _hub.Frames?.Dispose();
        _hub.Irq.Dispose();
        _tray?.Dispose();
        _window?.SaveGeometry();
        AppLog.Flush();
        Shutdown();
    }

    void EmergencyRevert()
    {
        try { _hub.Auto?.RevertEverything(L.T("Error")); }
        catch (Exception) { }
        AppLog.Flush();
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _mutex?.Dispose();
        base.OnExit(e);
    }
}
