using Xunit;

namespace DomainScheduler.Tests;

/// <summary>Section 22.13: the acceptance of the roadmap functions.</summary>
[Collection("static-hooks")]
public class RoadmapTests : IDisposable
{
    DateTime _now = new(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);
    readonly FakeNative _f = FakeNative.Cs2();
    readonly FakeProbe _p = new();
    readonly Engine _e;
    readonly AutoController _c;
    readonly HashSet<string> _watch = new(StringComparer.OrdinalIgnoreCase) { "cs2.exe" };
    readonly List<SessionData> _saved = new();
    readonly MemorySettingsBackend _settings = new();

    public RoadmapTests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        GameMask.IrqHook = () => new();
        GameMask.NoisyHook = null;
        Reservation.Reg = new FakeReserved();
        Settings.Backend = _settings;
        AppLog.Clear();
        _f.Procs[800] = new FakeNative.P { Name = "GCC.exe", Created = 5 };
        _f.Procs[801] = new FakeNative.P { Name = "notepad.exe", Created = 6 };
        _f.Procs[802] = new FakeNative.P { Name = "lsass.exe", Created = 7 };
        _f.Procs[803] = new FakeNative.P { Name = "dwm.exe", Created = 8 };
        _f.Procs[1] = new FakeNative.P { Name = "DomainScheduler.exe", Created = 1 };
        _e = new Engine(_f) { Now = () => _now, Sleep = Advance, GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false) };
        _c = new AutoController(_e, _p, new NoFrames(), () => _now) { SaveSession = s => _saved.Add(s), AutostartLine = () => "", WatchList = () => _watch };
    }

    public void Dispose()
    {
        Topo.LiveHook = null;
        GameMask.IrqHook = null;
        GameMask.NoisyHook = null;
        Reservation.Reg = new WinReservedRegistry();
        Settings.Backend = new RegistrySettingsBackend();
        Tables.EmbeddedHook = null;
        Tables.OverridePathHook = null;
        Tables.Reload();
    }

    void Advance(int ms)
    {
        _now = _now.AddMilliseconds(ms);
        _f.Run(new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [102] = 300_000_000, [103] = 900_000_000, [104] = 800_000_000, [105] = 1_200_000_000 }, ms / 1000.0);
        foreach (var pid in _p.Cpu.Keys.ToList())
        {
            var (c, cpu) = _p.Cpu[pid];
            if (_f.Procs.ContainsKey(pid)) _p.Cpu[pid] = (c, cpu + (long)(ms * 10_000 * 0.8));
        }
    }

    void Seconds(int n)
    {
        for (int i = 0; i < n; i++)
        {
            Advance(1000);
            _c.Tick();
        }
    }

    void GameToActive()
    {
        _p.Windows.Add(500);
        _p.Graphics.Add(500);
        _p.Cpu[500] = (_f.Procs[500].Created, 0);
        _p.Fg = 500;
        _c.Click();
        Seconds(12);
        Assert.Equal(AutoState.Active, _c.State);
    }

    static bool Logged(AppLog.Sev sev, string part) => AppLog.Tail(500).Any(e => e.Sev == sev && e.Msg.Contains(part));

    // ---- 22.1 ----------------------------------------------------------------------------------

    [Fact]
    public void BgOnOffRestoresEveryBackgroundBaseline()
    {
        Assert.Equal(0, _c.StandaloneBackground(true, 0) - _e.BackgroundCount);
        Assert.Equal(16384u, _f.Procs[800].Cls);               // lowered while the watched game runs
        Assert.Equal(16384u, _f.Procs[801].Cls);
        Assert.Equal(1u, _f.Procs[800].Io);
        Assert.Equal(32u, _f.Procs[500].Cls);                  // the game is not background
        Assert.Equal(32u, _f.Procs[802].Cls);                  // denylist
        Assert.Equal(32u, _f.Procs[803].Cls);                  // system
        Assert.DoesNotContain(802u, _f.OpenedPids);
        Assert.True(_e.BackgroundCount >= 2);

        _c.StandaloneBackground(false, 0);                     // the switch goes off: BgRelease
        Assert.Equal(32u, _f.Procs[800].Cls);
        Assert.Equal(32u, _f.Procs[801].Cls);
        Assert.Equal(2u, _f.Procs[800].Io);
        Assert.Equal(5u, _f.Procs[800].Mem);
        Assert.Equal(0u, _f.Procs[800].EcoState);
        Assert.Equal(0, _e.BackgroundCount);
        Assert.True(Logged(AppLog.Sev.Info, "BgRelease"));

        _c.StandaloneBackground(true, 0);
        Assert.Equal(16384u, _f.Procs[800].Cls);
        _f.Procs.Remove(500);                                  // the game ended: released too
        foreach (var t in _f.Threads.Where(t => t.Value.Pid == 500).Select(t => t.Key).ToList()) _f.Threads.Remove(t);
        _c.StandaloneBackground(true, 0);
        Assert.Equal(32u, _f.Procs[800].Cls);
    }

    // ---- 22.3 ----------------------------------------------------------------------------------

    [Fact]
    public void SoftCloseOnlyAsksListedProgramsAndNeverForces()
    {
        var asked = new List<uint>();
        var soft = new SoftClose { Now = () => _now, CloseWindow = pid => { asked.Add(pid); return true; }, On = true };
        soft.List = new(StringComparer.OrdinalIgnoreCase) { "notepad.exe", "lsass.exe", "dwm.exe", "cs2.exe", "DomainScheduler.exe" };
        var procs = _f.Processes();

        Assert.Equal(0, soft.Begin(procs, new[] { 500u }, _watch, 1));      // the flag is closed: refused with a warning
        Assert.Empty(asked);
        Assert.True(Logged(AppLog.Sev.Warn, "softclose"));

        Flags.Set(Flags.SoftClose, true);
        Assert.Equal(1, soft.Begin(procs, new[] { 500u }, _watch, 1));
        Assert.Equal(new[] { 801u }, asked);                                 // not the denylist, the system, the game or itself
        Assert.True(Logged(AppLog.Sev.Info, "notepad.exe (PID 801) was asked to close"));

        _now = _now.AddSeconds(3);
        soft.Tick(_ => true);
        Assert.False(Logged(AppLog.Sev.Warn, "still running"));
        _now = _now.AddSeconds(3);
        soft.Tick(_ => true);                                                // 5 s passed and it lives: left alone
        Assert.True(Logged(AppLog.Sev.Warn, "still running"));
        Assert.Single(asked);
        Assert.True(_f.Procs.ContainsKey(801));

        // Nothing in the sources can kill a process.
        string root = AppContext.BaseDirectory;
        while (root != null && !File.Exists(Path.Combine(root, "DomainScheduler.sln"))) root = Path.GetDirectoryName(root);
        foreach (var file in Directory.GetFiles(Path.Combine(root, "src"), "*.cs", SearchOption.AllDirectories).Where(x => !x.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)))
        {
            string code = string.Join("\n", File.ReadAllLines(file).Where(l => !l.TrimStart().StartsWith("//")));
            Assert.False(code.Contains("TerminateProcess(") || code.Contains(".Kill("), file);
        }
    }

    // ---- 22.4 ----------------------------------------------------------------------------------

    [Fact]
    public void ThreadCpuSetsOfThePinHeatPolicyHaveABaselineARevertAndDegradeToIdealOnly()
    {
        _f.Threads[100].Sets = new uint[] { 7 };                // what the thread had before
        _f.Threads[101].RefuseSets = true;                      // Windows refuses this one
        _c.Heat = _ => ("pin", "pin");                   // the game's own preset asks for hard limits
        GameToActive();
        var pins = _e.ThreadPins;
        Assert.Contains("500:100", pins.Keys);
        var t = Synth.X7800X3D();
        uint lp = pins["500:100"];
        Assert.Equal(new[] { t.Sets.First(s => s.LP == lp).Id }, _f.Threads[100].Sets);
        Assert.Equal(lp, _f.Threads[100].Ideal);
        Assert.Equal(0, _f.Threads[106].SetsWrites);            // the denylisted module's thread is never touched

        // The refused thread keeps its ideal processor; one warning; no failure, the pins stay.
        Assert.DoesNotContain("500:101", pins.Keys);
        Assert.NotEqual(999u, _f.Threads[101].Ideal);
        Assert.True(Logged(AppLog.Sev.Warn, "thread CPU Sets were refused"));
        Assert.False(Logged(AppLog.Sev.Error, "CPU Set"));
        Assert.Equal(AutoState.Active, _c.State);
        Assert.Contains(_saved.Last().Threads, x => x.Tid == 100 && x.SetsSet && x.Sets.SequenceEqual(new uint[] { 7 }));

        long before = _e.SetCalls;
        Seconds(10);
        Assert.Equal(before, _e.SetCalls);                      // a repeated apply writes nothing, the refused one is not retried

        _c.Click();                                             // revert
        Assert.Equal(new uint[] { 7 }, _f.Threads[100].Sets);
        Assert.Empty(_f.Threads[103].Sets);
        Assert.Empty(_e.ThreadPins);
    }

    [Fact]
    public void WithoutThePinHeatPolicyNoThreadCpuSetIsWritten()
    {
        GameToActive();
        Assert.All(_f.Threads.Values, t => Assert.Equal(0, t.SetsWrites));
        Assert.Empty(_e.ThreadPins);
    }

    // ---- 22.5 ----------------------------------------------------------------------------------

    [Fact]
    public void TimerResolutionIsRequestedOnActiveReleasedOnRevertAndAfterACrash()
    {
        _c.TimerRes = true;
        Assert.False(_e.TimerRequest());                        // the flag is closed: refused
        Assert.Equal(0, _f.TimerSets);
        Assert.True(Logged(AppLog.Sev.Warn, "timerres"));

        Flags.Set(Flags.TimerRes, true);
        GameToActive();
        Assert.True(_f.TimerHeld);
        Assert.Equal(5000u, _f.TimerCurrent);                   // the finest one, whatever the API calls it
        Assert.Equal(1, _f.TimerSets);
        Assert.Contains(_saved, s => s.TimerRequested && !s.CleanShutdown);
        Seconds(10);
        Assert.Equal(1, _f.TimerSets);

        _c.Click();
        Assert.False(_f.TimerHeld);
        Assert.Equal(1, _f.TimerReleases);
        Assert.False(_saved.Last().TimerRequested);
        Assert.True(_saved.Last().CleanShutdown);

        // A crashed run left the flag: the next start releases the timer before anything else.
        var f2 = FakeNative.Cs2();
        var e2 = new Engine(f2);
        e2.RestoreSession(new SessionData { CleanShutdown = false, TimerRequested = true });
        Assert.Equal(1, f2.TimerReleases);
        e2.RestoreSession(new SessionData { CleanShutdown = false });
        Assert.Equal(1, f2.TimerReleases);
    }

    [Fact]
    public void TimerIsReleasedWhenTheGameEnds()
    {
        Flags.Set(Flags.TimerRes, true);
        _c.TimerRes = true;
        GameToActive();
        Assert.True(_f.TimerHeld);
        _p.Cpu.Remove(500);
        _p.Windows.Remove(500);
        _p.Fg = 0;
        Seconds(2);
        Assert.Equal(AutoState.Armed, _c.State);
        Assert.False(_f.TimerHeld);
    }

    // ---- 22.6 ----------------------------------------------------------------------------------

    [Fact]
    public void HeatNoneNoneIsTheReferenceAndPinOverridesTheFirstSlot()
    {
        // Auto: none/none against no heat source at all — the same writes.
        var fa = FakeNative.Cs2();
        var fb = FakeNative.Cs2();
        var plan = new List<Classified>
        {
            new() { Row = new ThreadRow { Tid = 100, Born = 1000 }, Role = ThreadRole.Main, LP = 2 },
            new() { Row = new ThreadRow { Tid = 101, Born = 1010 }, Role = ThreadRole.Render, LP = 4 },
            new() { Row = new ThreadRow { Tid = 103, Born = 1030 }, Role = ThreadRole.Worker, LP = 6 },
        };
        var reference = AutoLogic.BuildAutoPreset("cs2.exe", plan, true);
        var none = AutoLogic.BuildAutoPreset("cs2.exe", plan, true, "none", "none");
        Assert.True(Store.Same(reference, none));
        new Engine(fa).ApplyAuto(500, reference, 0xFFFF, new ApplyOptions { DoMask = true, DoThreads = true });
        new Engine(fb).ApplyAuto(500, none, 0xFFFF, new ApplyOptions { DoMask = true, DoThreads = true });
        Assert.Equal(fa.Writes, fb.Writes);
        Assert.All(fb.Threads.Values, t => Assert.Equal(0, t.SetsWrites));

        // pin / ideal: the first slot gets the CPU Set too, the others the ideal processor only.
        var hot = AutoLogic.BuildAutoPreset("cs2.exe", plan, true, "pin", "ideal");
        Assert.Equal(new[] { true, false, false }, hot.Threads.Select(t => t.Pin));
        // Without pins (low confidence) nothing is hard.
        Assert.All(AutoLogic.BuildAutoPreset("cs2.exe", plan, false, "pin", "pin").Threads, t => Assert.False(t.Pin));

        // A preset of the user's: the hottest matched thread is pinned, the next ones get an ideal processor.
        var f = FakeNative.Cs2();
        DateTime now = _now;
        var e = new Engine(f) { Now = () => now };
        e.Sleep = ms =>
        {
            now = now.AddMilliseconds(ms);
            f.Run(new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [103] = 900_000_000, [106] = 9_000_000_000 }, ms / 1000.0);
        };
        var preset = new Preset { Exe = "cs2.exe", Class = "", Io = "", Mem = "", Eco = "", HotFirst = "pin", HotOthers = "ideal" };
        e.Apply(500, preset, new ApplyOptions { DoThreads = true, Warmup = true });
        Assert.Single(f.Threads[100].Sets);                     // the hottest eligible thread
        Assert.NotEqual(999u, f.Threads[101].Ideal);
        Assert.Empty(f.Threads[101].Sets);
        Assert.Equal(999u, f.Threads[106].Ideal);               // the denylisted module's thread is the hottest of all and is skipped
        Assert.Equal(0, f.Threads[106].SetsWrites);
        Assert.Equal(999u, f.Threads[107].Ideal);               // idle: not ranked

        var same = FakeNative.Cs2();
        new Engine(same).Apply(500, new Preset { Exe = "cs2.exe", Class = "", Io = "", Mem = "", Eco = "" }, new ApplyOptions { DoThreads = true });
        Assert.Equal(0, same.Writes);                           // none/none and no rules: nothing at all
    }

    // ---- 22.7 ----------------------------------------------------------------------------------

    [Fact]
    public void X3dCandidatesStartWithTheBigCacheDomainWhicheverHalfItIs()
    {
        var x = Synth.X7950X3D();
        Assert.Equal("x3d", x.Split.Family);
        var cand = Masks.IdealCandidates(x, false, 0);
        byte big = x.BigCacheLlc.Value;
        int firstOther = cand.FindIndex(c => c.LLC != big);
        Assert.True(firstOther > 0);
        Assert.All(cand.Take(firstOther), c => Assert.Equal(big, c.LLC));
        Assert.All(cand.Skip(firstOther), c => Assert.NotEqual(big, c.LLC));
    }

    // ---- 22.8 ----------------------------------------------------------------------------------

    [Fact]
    public void WideIrqNeverCutsWholeCoresAndTheHalfGuardCountsEverything()
    {
        var x = Synth.X7950X3D();
        var notes = new List<string>();
        // Dedicated pin on LP 4 (core mode: LPs 4-5), wide pin on LPs 8 and 10 (two cores): only those two.
        ulong m = GameMask.Adjust(x, 0xFFFF, 0, 0, 0x10, 0x500, GameMask.CoreMode, out ulong drop, out bool ignored, notes);
        Assert.Equal(0xFFFFUL & ~0x30UL & ~0x500UL, m);
        Assert.Equal(0x530UL, drop);
        Assert.False(ignored);
        Assert.Contains("wide-irq: partial", notes);
        // Without a wide pin: no note, the old behaviour.
        notes.Clear();
        Assert.Equal(0xFFCFUL, GameMask.Adjust(x, 0xFFFF, 0, 0, 0x10, 0, GameMask.CoreMode, out _, out _, notes));
        Assert.Empty(notes);
        Assert.Equal(0xFFCFUL, GameMask.Adjust(x, 0xFFFF, 0, 0, 0x10, GameMask.CoreMode, out _, out _));
        // Together they would take more than half: everything is ignored, the mask is unchanged.
        Assert.Equal(0xFFFFUL, GameMask.Adjust(x, 0xFFFF, 0, 0, 0x10, 0xFF00, GameMask.CoreMode, out drop, out ignored, notes));
        Assert.True(ignored);
        Assert.Equal(0UL, drop);
    }

    // ---- 22.9 ----------------------------------------------------------------------------------

    [Fact]
    public void LiveDpcIsrChangesTheMaskOnlyWhenItIsOn()
    {
        var x = Synth.X7950X3D();
        GameMask.IrqHook = () => new() { new("GPU", 0x30) };
        var registryOnly = GameMask.ForRule(x, 0xFFFF, GameMask.CoreMode);

        var irq = new EtwIrq { Now = () => _now };
        irq.MarkAvailableForTests();
        for (int s = 0; s < 10; s++)
        {
            irq.Add(8, 30);                                     // 3% of processor 8
            irq.Add(12, 10);                                    // 1% of processor 12: quiet
            _now = _now.AddSeconds(1);
        }
        Assert.Equal(0x100UL, irq.Noisy());

        GameMask.NoisyHook = null;                              // EtwIrq = false
        var off = GameMask.ForRule(x, 0xFFFF, GameMask.CoreMode);
        Assert.Equal(registryOnly.Final, off.Final);
        Assert.Equal(registryOnly.Irq, off.Irq);

        GameMask.NoisyHook = irq.Noisy;                         // EtwIrq = true
        var on = GameMask.ForRule(x, 0xFFFF, GameMask.CoreMode);
        Assert.Equal(registryOnly.Final & ~0x300UL, on.Final);  // the whole core of processor 8 is stepped around
        _now = _now.AddSeconds(30);                             // the noise is over
        Assert.Equal(registryOnly.Final, GameMask.ForRule(x, 0xFFFF, GameMask.CoreMode).Final);
    }

    [Fact]
    public void BusyInterruptCoresUseHysteresisAndHold()
    {
        var x = Synth.X7800X3D();
        var irq = new EtwIrq { Now = () => _now };
        irq.MarkAvailableForTests();
        ulong? got = null;
        for (int s = 0; s < 40; s++)
        {
            irq.Add(12, 100);                                   // 10% of processor 12 (core 6)
            irq.Add(14, 100);                                   // 10% of processor 14 (core 7)
            irq.Add(2, 10);                                     // 1% of processor 2: quiet
            got = irq.BusyCoresIfReady(x);
            if (s < 25) Assert.Null(got);                       // no verdict before a full window and the hold
            _now = _now.AddSeconds(1);
        }
        Assert.Equal(0xF000UL, got);                            // both cores, both SMT threads
        for (int s = 0; s < 40; s++)                            // 3%: under "enter", above "leave": stays busy
        {
            irq.Add(12, 30); irq.Add(14, 30);
            Assert.Equal(0xF000UL, irq.BusyCoresIfReady(x));
            _now = _now.AddSeconds(1);
        }
        for (int s = 0; s < 60; s++)                            // quiet: still kept for 60 s (Test 6), no flip-flop
        {
            irq.Add(12, 5);
            Assert.Equal(0xF000UL, irq.BusyCoresIfReady(x));
            _now = _now.AddSeconds(1);
        }
        for (int s = 0; s < 15; s++) { irq.Add(12, 5); irq.BusyCoresIfReady(x); _now = _now.AddSeconds(1); }
        Assert.Equal(0UL, irq.BusyCoresIfReady(x));             // quiet for longer than window + 60 s
    }

    [Fact]
    public void GraphicsDriverCoresAreAvoidedFromTheThirdSecond()
    {
        var x = Synth.X7800X3D();
        var irq = new EtwIrq { Now = () => _now };
        irq.MarkAvailableForTests();
        irq.SetDrivers(new() { (0x1000, "nvlddmkm.sys"), (0x9000000, "storport.sys") });
        ulong? got = null;
        for (int s = 0; s < 6; s++)
        {
            irq.Add(4, 6, 0x1500);                              // the graphics driver: 0.6% of processor 4 (core 2)
            irq.Add(9, 6, 0x9000100);                           // another driver, same share: not a graphics core
            got = irq.BusyCoresIfReady(x);
            if (s < EtwIrq.GpuSeconds) Assert.Null(got);        // nothing known before 3 s of trace
            _now = _now.AddSeconds(1);
        }
        Assert.Equal(0x30UL, got);                              // the whole core 2 (processors 4 and 5), long before the busy-core rule
        for (int s = 0; s < 5; s++) { irq.Add(4, 0.5, 0x1500); got = irq.BusyCoresIfReady(x); _now = _now.AddSeconds(1); }
        Assert.Equal(0x30UL, irq.GpuCores(x));                  // quiet for 5 s: still kept (Test 6: 60 s, no flip-flop)
        for (int s = 0; s < 60; s++) { irq.Add(4, 0.5, 0x1500); got = irq.BusyCoresIfReady(x); _now = _now.AddSeconds(1); }
        Assert.Equal(0UL, irq.GpuCores(x));                     // quiet (0.05%) for over 60 s: the core is free again
    }

    [Fact]
    public void LiveMaskStepsAroundWholeBusyCoresButNeverMoreThanHalf()
    {
        var x = Synth.X7800X3D();
        var live = GameMask.ForLive(x, 0xFFFF, 0xF000);
        Assert.Equal(0x0FFFUL, live.Final);
        Assert.Equal(0xF000UL, live.IrqDrop);
        var tooMuch = GameMask.ForLive(x, 0xFFFF, 0xFFF0);
        Assert.True(tooMuch.IrqIgnored);
        Assert.Equal(0xFFFFUL, tooMuch.Final);
    }

    [Fact]
    public void AutoUsesLiveCoresWhenReadyAndThePinsBefore()
    {
        var x = Synth.X7800X3D();
        GameMask.IrqHook = () => new() { new("GPU", 0x5000), new("Mouse", 0x4) };
        try
        {
            GameMask.BusyHook = null;
            var pins = AutoLogic.BuildMask(x, 0);
            GameMask.BusyHook = _ => null;                      // live on, not ready yet: same as the pins
            Assert.Equal(pins.Mask, AutoLogic.BuildMask(x, 0).Mask);
            GameMask.BusyHook = _ => 0xF000UL;                  // ready: whole cores 6 and 7 out, the pins no longer used
            Assert.Equal(0x0FFFUL, AutoLogic.BuildMask(x, 0).Mask);
        }
        finally { GameMask.BusyHook = null; }
    }

    // ---- 22.11 ---------------------------------------------------------------------------------

    [Fact]
    public void TablesMergeTheUserFileAndSurviveABrokenOne()
    {
        string dir = Path.Combine(Path.GetTempPath(), "ds-tab-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        string path = Path.Combine(dir, "tables.json");
        Tables.OverridePathHook = () => path;
        try
        {
            Tables.Reload();
            Assert.Equal(16, Tables.X3d["7950X3D"]);
            Assert.True(Tables.KnownGames.ContainsKey("cs2.exe"));
            Assert.False(Tables.Current.OverrideUsed);
            int games = Tables.KnownGames.Count;

            File.WriteAllText(path, """{"knownGames":[{"exe":"mygame","label":"My game"},{"exe":"cs2.exe","label":"CS"},{"exe":"lsass.exe","label":"x"},{"exe":""}],"x3d":[{"sku":"9955X3D","cores":16},{"sku":"7950X3D","cores":32},{"sku":"junk","cores":16},{"sku":"1234X3D","cores":7}]}""");
            Tables.Reload();
            Assert.True(Tables.Current.OverrideUsed);
            Assert.Equal("My game", Tables.KnownGames["mygame.exe"]);
            Assert.Equal("CS", Tables.KnownGames["cs2.exe"]);                 // the user file wins
            Assert.False(Tables.KnownGames.ContainsKey("lsass.exe"));          // a denylisted program is never a known game
            Assert.Equal(games + 1, Tables.KnownGames.Count);
            Assert.Equal(16, Tables.X3d["9955X3D"]);
            Assert.Equal(32, Tables.X3d["7950X3D"]);
            Assert.False(Tables.X3d.ContainsKey("JUNK"));
            Assert.False(Tables.X3d.ContainsKey("1234X3D"));
            Assert.True(Logged(AppLog.Sev.Warn, "entries rejected"));

            AppLog.Clear();
            File.WriteAllText(path, "{ this is not json");
            Tables.Reload();
            Assert.False(Tables.Current.OverrideUsed);
            Assert.Equal(16, Tables.X3d["7950X3D"]);                           // embedded
            Assert.Equal(games, Tables.KnownGames.Count);
            Assert.True(Logged(AppLog.Sev.Error, "tables.json override is broken"));
        }
        finally { Directory.Delete(dir, true); }
    }

    // ---- 22.12 ---------------------------------------------------------------------------------

    [Fact]
    public void ClosedFlagsRefuseTheirActionsAndOnlyCoreDevicesAndGpuAreOpenByDefault()
    {
        Assert.Equal(new[] { "core", "devices-write", "softclose", "etwirq", "timerres", "gpu-oc" }, Flags.FlagKeys);
        // 1.16.0: core, devices-write and gpu-oc open by default (owner's defaults); the rest closed.
        foreach (var k in new[] { Flags.Core, Flags.DevicesWrite, Flags.GpuOc }) Assert.True(Flags.Open(k));
        foreach (var k in new[] { Flags.SoftClose, Flags.EtwIrq, Flags.TimerRes }) Assert.False(Flags.Open(k));
        Assert.False(Flags.Open("no-such-flag"));
        Flags.Set(Flags.DevicesWrite, false);      // the refusal below needs the flag closed by the user

        int calls = Devices.WriteCalls;
        string err = Devices.Write(new PciDevice { Name = "NIC", Pdo = @"PCI\VEN_1" }, new IrqConfig { Assign = 4 });
        Assert.Contains("devices-write", err);
        Assert.Equal(calls + 1, Devices.WriteCalls);
        Assert.True(Logged(AppLog.Sev.Warn, "devices-write"));
        Assert.False(_e.TimerRequest());
        Assert.Equal(0, _f.TimerSets);

        Flags.Set(Flags.TimerRes, true);
        Assert.True(Flags.Open(Flags.TimerRes));
        Assert.True(_e.TimerRequest());
        Assert.True(_e.TimerRelease());
        Flags.Set(Flags.TimerRes, false);
        Assert.False(_e.TimerRequest());
    }

    // ---- 22.13, last line ----------------------------------------------------------------------

    [Fact]
    public void NoRoadmapFunctionTouchesTheDenylistOrSetsRealtime()
    {
        Flags.Set(Flags.TimerRes, true);
        Flags.Set(Flags.SoftClose, true);
        _c.TimerRes = true;
        _c.Heat = _ => ("pin", "pin");
        _c.Soft = new SoftClose { On = true, CloseWindow = _ => true, List = new(StringComparer.OrdinalIgnoreCase) { "lsass.exe", "notepad.exe" } };
        _c.StandaloneBackground(true, 0);
        GameToActive();
        Seconds(10);
        Assert.DoesNotContain(802u, _f.OpenedPids);                           // lsass: never opened by anything
        Assert.All(_f.Procs.Values, p => Assert.NotEqual(256u, p.Cls));       // nobody is Realtime
        Assert.All(_f.Procs.Values, p => Assert.NotEqual(128u, p.Cls));
        Assert.Equal(0, _f.Threads[106].SetsWrites);                          // the denylisted module's thread
        Assert.Equal(0, _f.Threads[106].Prio);
        Assert.Equal(1, _c.Soft.Asked);
        _c.Click();
        Assert.Equal(32u, _f.Procs[500].Cls);
        Assert.All(_f.Threads.Values.Where(t => t.Pid == 500), t => Assert.Empty(t.Sets));
        Assert.False(_f.TimerHeld);
    }
}
