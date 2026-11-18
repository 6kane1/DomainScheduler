using Xunit;

namespace DomainScheduler.Tests;

sealed class MemTasks : ITaskBackend
{
    public readonly Dictionary<string, string> Tasks = new();
    public bool Fail;
    public string Read(string name) => Tasks.GetValueOrDefault(name);
    public void Register(string name, string xml) { if (Fail) throw new InvalidOperationException("COM down"); Tasks[name] = xml; }
    public void Delete(string name) => Tasks.Remove(name);
}

[Collection("static-hooks")]
public class AddendumTests : IDisposable
{
    readonly MemTasks _com = new(), _sch = new();

    public AddendumTests()
    {
        Autostart.Primary = _com;
        Autostart.Fallback = _sch;
        Autostart.UserHook = () => "PC\\player";
        Settings.Backend = new MemorySettingsBackend();
        Topo.LiveHook = Synth.X7800X3D;
        Reservation.Reg = new FakeReserved();
        GameMask.IrqHook = () => new();
    }

    public void Dispose()
    {
        Autostart.Primary = new ComTaskBackend();
        Autostart.Fallback = new SchtasksBackend();
        Autostart.UserHook = null;
        Settings.Backend = new RegistrySettingsBackend();
        Topo.LiveHook = null;
        Reservation.Reg = new WinReservedRegistry();
        GameMask.IrqHook = null;
        ThreadIni.TextHook = null;
    }

    [Fact]
    public void AutostartTaskIsCreatedOnceAndReadBack()
    {
        Autostart.EnsureOnFirstRun(@"C:\Tools\DomainScheduler.exe");
        Assert.True(Autostart.TaskExists());
        Assert.Contains("HighestAvailable", _com.Tasks[Autostart.TaskName]);
        Assert.Contains("--autostart", _com.Tasks[Autostart.TaskName]);
        Assert.True(Autostart.TaskEnabled());
        // The user removed it: a second start does not recreate it; exit does not remove it.
        _com.Tasks.Clear();
        Autostart.EnsureOnFirstRun(@"C:\Tools\DomainScheduler.exe");
        Assert.False(Autostart.TaskExists());
        // COM failing: schtasks takes over.
        _com.Fail = true;
        Autostart.TaskCreate(@"C:\Tools\DomainScheduler.exe");
        Assert.True(_sch.Tasks.ContainsKey(Autostart.TaskName));
        Autostart.TaskDelete();
        Assert.False(Autostart.TaskExists());
        Assert.Contains("(off)", Autostart.ReportLine());
    }

    [Fact]
    public void BundledPresetsPassSelfTestAndBrokenOnesAreReported()
    {
        Assert.Empty(ThreadIni.SelfTest());
        var bundled = ThreadIni.LoadBundled();
        foreach (var name in new[] { "spotify", "discord", "steam", "steamwebhelper", "dwm", "searchapp", "winlogon", "audiodg", "sihost",
                     "taskhostw", "dllhost", "startmenuexperiencehost", "shellexperiencehost", "textinputhost", "conhost", "explorer", "ctfmon", "ayugram", "winws", "domainscheduler" })
            Assert.True(bundled.Presets.ContainsKey(name + ".exe"), name);
        Assert.Equal(20, bundled.Presets.Count);   // svchost was removed: it is a protected process and always failed
        Assert.All(bundled.Presets.Values, p => Assert.True(p.Builtin));
        Assert.Equal("off", bundled.Presets["spotify.exe"].Boost);
        Assert.Equal("", bundled.Presets["dwm.exe"].Boost);
        Assert.Equal(10000, bundled.Sleep);
        Settings.Backend = new MemorySettingsBackend();
        Assert.Equal("10000", Settings.Get("ReapplyMs"));

        var r = ThreadIni.Parse("[SETTINGS]\nsleep=3000\n[DISABLEBOOST]\nfoo\n[foo]\nmodule=a.dll,-1,tid=last,disableboost\ndescription=*Render*,2,tid=3\n");
        Assert.Equal(3000, r.Sleep);
        var foo = r.Presets["foo.exe"];
        Assert.Equal("off", foo.Boost);
        Assert.Equal(("module", "a.dll", "-1", "last", "off"), (foo.Threads[0].Match, foo.Threads[0].Pattern, foo.Threads[0].Prio, foo.Threads[0].Tid, foo.Threads[0].Boost));
        Assert.Equal(("desc", "*Render*", "2", "3", ""), (foo.Threads[1].Match, foo.Threads[1].Pattern, foo.Threads[1].Prio, foo.Threads[1].Tid, foo.Threads[1].Boost));
        Assert.Empty(ThreadIni.SelfTest(r));

        ThreadIni.TextHook = () => "[bad]\nmodule=x.dll,7\nmodule=y.dll,1,tid=soon\nwhat=1\n[empty]\n[csrss]\nmodule=csrss.exe,1\n";
        var bad = ThreadIni.SelfTest();
        Assert.Contains(bad, b => b.Contains("bad priority"));
        Assert.Contains(bad, b => b.Contains("bad tid"));
        Assert.Contains(bad, b => b.Contains("unknown key"));
        Assert.Contains(bad, b => b.Contains("empty.exe"));
        Assert.Contains(bad, b => b.Contains("csrss.exe") && b.Contains("denylisted"));
    }

    [Fact]
    public void SelfTestProblemsReachTheReportAndAutoStillWorks()
    {
        var f = FakeNative.Cs2();
        var e = new Engine(f);
        var c = new AutoController(e, new FakeProbe(), new NoFrames(), () => DateTime.UtcNow)
        { SaveSession = _ => { }, AutostartLine = () => "Autostart: task DomainScheduler (on)", SelfTestProblems = new() { "bad.exe/x: bad prio" } };
        c.Click();
        Assert.Equal(AutoState.Armed, c.State);
        string rep = c.Report();
        Assert.Contains("SelfTest: 1 problems", rep);
        Assert.Contains("Autostart: task DomainScheduler (on)", rep);
        Assert.Contains("Manual API: present, not active in Auto", rep);
        Assert.Contains("Frametime: not measured", rep);
        c.Shutdown();
    }

    [Fact]
    public void ManualApisChangeTheTargetAndAutoNeverCallsThem()
    {
        var f = FakeNative.Cs2();
        var e = new Engine(f);
        var log = new List<string>();
        Assert.True(e.ProcessClassManual(500, "above", log));
        Assert.Equal(32768u, f.Procs[500].Cls);
        Assert.False(e.ProcessClassManual(500, "realtime", log));
        Assert.True(e.ProcessBoost(500, true, log));
        Assert.True(f.Procs[500].BoostDisabled);
        Assert.Contains("priority 2", e.ThreadManual(500, 100, "highest", "4", "off"));
        Assert.Equal(2, f.Threads[100].Prio);
        Assert.Equal(4u, f.Threads[100].Ideal);
        Assert.True(e.Revert(500) > 0);
        Assert.Equal(32u, f.Procs[500].Cls);
        Assert.Equal(0, f.Threads[100].Prio);
        // The Auto path: zero manual calls.
        var f2 = FakeNative.Cs2();
        var e2 = new Engine(f2);
        var p = new FakeProbe();
        var now = DateTime.UtcNow;
        var c = new AutoController(e2, p, new NoFrames(), () => now) { SaveSession = _ => { }, AutostartLine = () => "" };
        c.Click();
        p.Windows.Add(500); p.Graphics.Add(500); p.Fg = 500; p.Cpu[500] = (77, 0);
        for (int i = 0; i < 20; i++) { now = now.AddSeconds(1); p.Cpu[500] = (77, p.Cpu[500].cpu + 8_000_000); c.Tick(); }
        c.Shutdown();
        Assert.Equal(0, e2.ManualCalls);
    }

    [Fact]
    public void AppLogLevels()
    {
        AppLog.Clear();
        var f = FakeNative.Cs2();
        var e = new Engine(f) { GateHook = (pid, exe) => "cs2.exe runs under FACEIT" };
        e.ApplyAuto(500, new Preset { Exe = "cs2.exe" }, 0, new ApplyOptions());
        var tail = AppLog.Tail(10);
        Assert.Contains(tail, x => x.Sev == AppLog.Sev.Warn && x.Msg.Contains("FACEIT"));    // AdviceOnce
        // report.Failures: a process that cannot be changed is skipped — a warning in the log file, never an error window
        Assert.DoesNotContain(tail, x => x.Sev == AppLog.Sev.Error);
        for (int i = 0; i < AppLog.Capacity + 10; i++) AppLog.Add("x" + i);
        Assert.Equal(AppLog.Capacity, AppLog.Tail(int.MaxValue).Count);
    }
}

[Collection("static-hooks")]
public class LangTests
{
    [Fact]
    public void RussianStatusesMatchTheSpec()
    {
        try
        {
            L.Load("ru");
            Assert.Equal("ru", L.Lang);
            Assert.Equal("Жду игру…", L.T("Waiting for the game…"));
            Assert.Equal("Игра: cs2.exe · ускорение включено", L.F("Game: {0} · boost on", "cs2.exe"));
            Assert.Equal("Античит: игра не изменена, фон приглушён", L.T("Anti-cheat: the game is not changed, the background is muted"));
            Assert.Equal("Ошибка. Всё возвращено как было. Откройте отчёт.", L.T("Error. Everything was put back. Open the Report."));
            Assert.Equal("Раздел Session Manager\\Kernel не открыть", L.T("The Session Manager\\Kernel key cannot be opened"));
            Assert.Equal("untranslated", L.T("untranslated"));
        }
        finally { L.Load("en"); }
    }
}
