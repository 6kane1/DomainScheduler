using Xunit;

namespace DomainScheduler.Tests;

public sealed class FakeProbe : IGameProbe
{
    public uint SelfPid => 1;
    public uint Fg;
    public readonly HashSet<uint> Windows = new();
    public readonly HashSet<uint> Graphics = new();
    public readonly Dictionary<uint, (long created, long cpu)> Cpu = new();
    public readonly Dictionary<uint, uint> Session = new();
    public int GraphicsChecks;
    public uint ForegroundPid() => Fg;
    public HashSet<uint> WindowPids() => new(Windows);
    public bool HasGraphicsModule(uint pid) { GraphicsChecks++; return Graphics.Contains(pid); }
    public bool CpuTime(uint pid, out long created, out long cpu)
    {
        created = cpu = 0;
        if (!Cpu.TryGetValue(pid, out var v)) return false;
        (created, cpu) = v;
        return true;
    }
    public uint SessionOf(uint pid) => Session.GetValueOrDefault(pid, 1u);
}

public sealed class FakeFrames : IFrameSource
{
    public bool Available { get; set; } = true;
    public string Name => "fake";
    public string Error => "";
    public uint Watched;
    public double FrameMs = 4;
    public void Watch(uint pid) => Watched = pid;
    public List<double> Intervals(DateTime from, DateTime to) => Watched == 0 ? new() : Enumerable.Repeat(FrameMs, (int)((to - from).TotalMilliseconds / FrameMs)).ToList();
}

[Collection("static-hooks")]
public class AutoTests : IDisposable
{
    DateTime _now = new(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);
    readonly FakeNative _f = FakeNative.Cs2();
    readonly FakeProbe _p = new();
    readonly FakeFrames _fr = new();
    readonly Engine _e;
    readonly AutoController _c;
    readonly List<SessionData> _saved = new();

    public AutoTests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        GameMask.IrqHook = () => new();
        Reservation.Reg = new FakeReserved();
        Settings.Backend = new MemorySettingsBackend();
        Settings.ResetReads();
        _f.Procs[800] = new FakeNative.P { Name = "GCC.exe", Created = 5 };
        _f.Procs[801] = new FakeNative.P { Name = "svchost.exe", Created = 6 };
        _f.Procs[1] = new FakeNative.P { Name = "DomainScheduler.exe", Created = 1 };
        _p.Session[801] = 0;
        _e = new Engine(_f) { Now = () => _now, Sleep = Advance };
        _c = new AutoController(_e, _p, _fr, () => _now) { SaveSession = s => _saved.Add(s) };
    }

    public void Dispose()
    {
        Topo.LiveHook = null;
        GameMask.IrqHook = null;
        Reservation.Reg = new WinReservedRegistry();
        Settings.Backend = new RegistrySettingsBackend();
    }

    void Advance(int ms)
    {
        _now = _now.AddMilliseconds(ms);
        _f.Run(new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [102] = 300_000_000, [103] = 900_000_000, [104] = 800_000_000, [105] = 1_200_000_000 }, ms / 1000.0);
        foreach (var pid in _p.Cpu.Keys.ToList())
        {
            var (c, cpu) = _p.Cpu[pid];
            if (_f.Procs.ContainsKey(pid)) _p.Cpu[pid] = (c, cpu + (long)(ms * 10_000 * 0.8));   // 0.8 of a core
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

    void StartGame(uint pid = 500)
    {
        _p.Windows.Add(pid);
        _p.Graphics.Add(pid);
        _p.Cpu[pid] = (_f.Procs[pid].Created, 0);
        _p.Fg = pid;
    }

    [Fact]
    public void ClickWithoutGameArmsAndMutesTheBackground()
    {
        _c.Click();
        Assert.Equal(AutoState.Armed, _c.State);
        Assert.Equal("Waiting for the game…", _c.Status);
        Seconds(12);
        Assert.Equal(AutoState.Armed, _c.State);
        Assert.Equal(16384u, _f.Procs[800].Cls);
        Assert.Equal(32u, _f.Procs[801].Cls);   // another session
        Assert.Equal(32u, _f.Procs[1].Cls);     // this program
        Assert.NotEmpty(_saved);
        Assert.False(_saved[^1].CleanShutdown);
    }

    [Fact]
    public void GameIsLaidOutAfterHysteresis()
    {
        _c.Click();
        StartGame();
        Seconds(3);
        Assert.Equal(AutoState.Armed, _c.State);
        Assert.Equal("Detecting the game…", _c.Status);
        Seconds(4);
        Assert.Equal(AutoState.Active, _c.State);
        Assert.Equal("Game: cs2.exe · boost on", _c.Status);
        Assert.Equal(32768u, _f.Procs[500].Cls);
        Assert.Equal(1u, _f.Procs[500].EcoCtl);
        Assert.Equal(0u, _f.Procs[500].EcoState);
        // Warm-up gave rates to at least two threads before classification.
        Assert.True(_c.WarmRates >= 2);
        // Pins: at most cores−1, one per physical core, Main and Render apart in one domain.
        var ideals = _e.Ideals;
        var cand = Masks.IdealCandidates(Synth.X7800X3D(), true, 0xFFFF);
        Assert.True(ideals.Count <= cand.Count - 1);
        Assert.Equal(ideals.Count, ideals.Values.Distinct().Count());
        Assert.All(ideals.Values, lp => Assert.True(lp % 2 == 0 && lp != 0));
        uint main = _f.Threads[100].Ideal, render = _f.Threads[101].Ideal;
        Assert.NotEqual(main, render);
        Assert.NotEqual(main / 2, render / 2);
        Assert.Equal(999u, _f.Threads[106].Ideal);   // anti-cheat thread
        Assert.Equal(999u, _f.Threads[107].Ideal);   // cold aux thread
        // The synthesized rules are numeric and the LPs do not move on later ticks.
        var before = _f.Threads.ToDictionary(kv => kv.Key, kv => kv.Value.IdealSets);
        long sets = _e.SetCalls;
        Seconds(20);
        Assert.Equal(before, _f.Threads.ToDictionary(kv => kv.Key, kv => kv.Value.IdealSets));
    }

    [Fact]
    public void AltTabNeverReverts()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);
        _p.Fg = 800;
        Seconds(3);
        _p.Fg = 500;
        Seconds(1);
        Assert.Equal(AutoState.Active, _c.State);
        _p.Fg = 800;
        Seconds(20);
        Assert.Equal(AutoState.Active, _c.State);   // alt-tab no longer reverts: the changes stay while the game lives
    }

    [Fact]
    public void RestartedGameIsPickedUpWithoutAClick()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);
        _f.Procs.Remove(500);
        _p.Windows.Remove(500);
        _p.Cpu.Remove(500);
        Seconds(2);
        Assert.Equal(AutoState.Armed, _c.State);
        _f.Procs[500] = new FakeNative.P { Name = "cs2.exe", Created = 99, Mods = FakeNative.Cs2().Procs[500].Mods };
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);
        Assert.Equal(32768u, _f.Procs[500].Cls);
    }

    [Fact]
    public void ClosingGameThatLosesItsThreadsFirstKeepsAutoOn()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);
        // The game is closing: its threads are gone, the process object still exists for a moment.
        foreach (var tid in _f.Threads.Where(kv => kv.Value.Pid == 500).Select(kv => kv.Key).ToList()) _f.Threads.Remove(tid);
        Seconds(12);
        Assert.NotEqual(AutoState.Idle, _c.State);   // a normal end, not a watchdog revert that switches Auto off
        _f.Procs.Remove(500);
        _p.Windows.Remove(500);
        _p.Cpu.Remove(500);
        Seconds(2);
        _f.Procs[500] = new FakeNative.P { Name = "cs2.exe", Created = 99, Mods = FakeNative.Cs2().Procs[500].Mods };
        foreach (var kv in FakeNative.Cs2().Threads.Where(kv => kv.Value.Pid == 500)) _f.Threads[kv.Key] = kv.Value;
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);   // the next game is picked up without a click
    }

    [Fact]
    public void AntiCheatGameIsNeverOpened()
    {
        _f.Procs[900] = new FakeNative.P { Name = "faceit.exe", Created = 3 };
        _c.Click();
        StartGame();
        Seconds(15);
        Assert.Equal(AutoState.Armed, _c.State);
        Assert.Equal("Anti-cheat: the game is not changed, the background is muted", _c.Status);
        Assert.DoesNotContain(500u, _f.OpenedPids);
        Assert.DoesNotContain(900u, _f.OpenedPids);
        Assert.Equal(16384u, _f.Procs[800].Cls);
    }

    [Fact]
    public void LowConfidenceSetsNoIdealProcessor()
    {
        foreach (var t in _f.Threads.Values)
        {
            t.Desc = "";
            t.Start = 0;
        }
        _c.Click();
        StartGame();
        Seconds(10);
        Assert.True(_c.State == AutoState.Active, string.Join("\n", AppLog.Tail(50)));
        Assert.Equal(0, _f.Threads.Values.Sum(t => t.IdealSets));
        Assert.EndsWith("(basic)", _c.Status);
        Assert.Equal(32768u, _f.Procs[500].Cls);   // mask + priority + EcoQoS still apply
    }

    [Fact]
    public void UnsatisfiableMaskRevertsWithinOneTick()
    {
        Topo.LiveHook = Synth.X7950X3D;
        _c.Click();
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);
        Assert.Equal(0xFFFFUL, _f.Procs[500].Aff);   // the V-Cache die
        _c.MaskFilter = mp => { mp.Satisfied = false; mp.Notes.Add("injected"); return mp; };
        Seconds(5);   // one re-apply tick
        Assert.Equal(AutoState.Idle, _c.State);
        Assert.Equal(32u, _f.Procs[500].Cls);
        Assert.Equal(ulong.MaxValue, _f.Procs[500].Aff);
        Assert.StartsWith("Reverted:", _c.Status);
    }

    [Fact]
    public void ShutdownRevertsEverything()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        _c.Shutdown();
        Assert.Equal(AutoState.Idle, _c.State);
        Assert.Equal(32u, _f.Procs[500].Cls);
        Assert.Equal(0u, _f.Procs[500].EcoCtl);
        Assert.Equal(32u, _f.Procs[800].Cls);
        Assert.Equal(0, _f.Threads[100].Prio);
        Assert.True(_saved[^1].CleanShutdown);
    }

    [Fact]
    public void CrashedSessionIsRestoredAtTheNextStart()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        var crashed = _saved[^1];
        Assert.False(crashed.CleanShutdown);
        // A new process of the program: new engine and controller over the same Windows.
        var e2 = new Engine(_f);
        var c2 = new AutoController(e2, _p, _fr, () => _now) { SaveSession = _ => { } };
        Assert.True(c2.RecoverCrashedSession(crashed) > 0);
        Assert.Equal(32u, _f.Procs[800].Cls);
        Assert.Equal(32u, _f.Procs[500].Cls);
    }

    [Fact]
    public void AutoPathReadsNoSettingsAndCallsNoBannedLoaders()
    {
        Settings.ResetReads();
        int st = Store.LoadCalls, lp = Store.ListProfilesCalls, ar = Settings.LoadAffRulesCalls, res = Reservation.ApplyCalls, dev = Devices.WriteCalls;
        _c.Click();
        StartGame();
        Seconds(30);
        _c.Shutdown();
        Assert.DoesNotContain(Settings.Reads().Keys, k => !Settings.ProductKeys.Contains(k));
        Assert.Equal(st, Store.LoadCalls);
        Assert.Equal(lp, Store.ListProfilesCalls);
        Assert.Equal(ar, Settings.LoadAffRulesCalls);
        Assert.Equal(res, Reservation.ApplyCalls);
        Assert.Equal(dev, Devices.WriteCalls);
    }

    [Fact]
    public void WorseFrameTimeNeverRevertsAndNoFramesIsReported()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        Assert.Equal(AutoState.Active, _c.State);
        _fr.FrameMs = 8;   // p99 doubled: there is no frame-time watchdog, Auto keeps its changes
        Seconds(90);
        Assert.Equal(AutoState.Active, _c.State);
        Assert.Contains("boost on", _c.Status);
        _fr.Available = false;
        Assert.Contains("Frame time is not measured", _c.Report());
    }

    [Fact]
    public void ButtonDuringActiveRevertsToIdle()
    {
        _c.Click();
        StartGame();
        Seconds(7);
        _c.Click();
        Assert.Equal(AutoState.Idle, _c.State);
        Assert.Equal("Off", _c.Status);
        Assert.Equal(32u, _f.Procs[500].Cls);
    }
}

[Collection("static-hooks")]
public class AutoLogicTests
{
    [Fact]
    public void HardenKeepsAtLeastHalfAndMinLp()
    {
        var x = Synth.X7950X3D();
        Assert.Equal(x.FullMask(), AutoLogic.Harden(x, 0x7));            // < MinGameLp
        Assert.Equal(x.FullMask(), AutoLogic.Harden(x, 0xFF));           // < half
        Assert.Equal(0xFFFFUL, AutoLogic.Harden(x, 0xFFFF));             // the V-Cache die (exactly half)
        Assert.Equal(0xFFFFUL, AutoLogic.Harden(x, x.FullMask()));       // two domains → the best single domain
        var e = Synth.X7950X();
        Assert.Equal(0xFFFFUL, AutoLogic.Harden(e, e.FullMask()));
        var s = Synth.X7800X3D();
        Assert.Equal(s.FullMask(), AutoLogic.Harden(s, s.FullMask()));
        foreach (var t in new[] { x, e, s, Synth.I13900K(), Synth.I12400(), Synth.Six() })
        {
            Topo.LiveHook = () => t;
            try
            {
                GameMask.IrqHook = () => new();
                var mp = AutoLogic.BuildMask(t, 0);
                if (mp.Satisfied)
                {
                    Assert.True(TopoBuilder.PopCount(mp.Mask) >= 4);
                    Assert.True(TopoBuilder.PopCount(mp.Mask) * 2 >= TopoBuilder.PopCount(t.FullMask()));
                }
            }
            finally { Topo.LiveHook = null; GameMask.IrqHook = null; }
        }
    }

    [Fact]
    public void PickMaskByProcessor()
    {
        Assert.Equal("vcache", AutoLogic.PickMask(Synth.X7950X3D()));
        Assert.Equal("pcores", AutoLogic.PickMask(Synth.I13900K()));
        Assert.Equal("auto", AutoLogic.PickMask(Synth.X7800X3D()));
        Assert.Equal("auto", AutoLogic.PickMask(Synth.X7950X()));
    }

    [Fact]
    public void ClassifyRoles()
    {
        var s = new ThreadSnap
        {
            Rows =
            {
                new ThreadRow { Tid = 10, Desc = "MainThrd", Rate = 100 },
                new ThreadRow { Tid = 11, Desc = "D3D11RenderThread", Rate = 50 },
                new ThreadRow { Tid = 12, Desc = "AudioMixer", Rate = 5 },
                new ThreadRow { Tid = 13, Desc = "GlobPool/P0", Rate = 40 },
                new ThreadRow { Tid = 14, Desc = "idle", Rate = 1 },
                new ThreadRow { Tid = 15, Sym = "EasyAntiCheat_EOS.dll+0x10", Rate = 30 },
            },
        };
        Assert.Equal(ThreadRole.Main, AutoLogic.Classify(s.Rows[0], s));
        Assert.Equal(ThreadRole.Render, AutoLogic.Classify(s.Rows[1], s));
        Assert.Equal(ThreadRole.Audio, AutoLogic.Classify(s.Rows[2], s));
        Assert.Equal(ThreadRole.Worker, AutoLogic.Classify(s.Rows[3], s));
        Assert.Equal(ThreadRole.Aux, AutoLogic.Classify(s.Rows[4], s));
        Assert.Equal(ThreadRole.Skip, AutoLogic.Classify(s.Rows[5], s));
    }

    [Fact]
    public void PlanIsStickyAndRebuildsOnlyOnBigChanges()
    {
        var t = Synth.X7950X3D();
        var s = new ThreadSnap { Rows = Enumerable.Range(0, 10).Select(i => new ThreadRow { Tid = (uint)(100 + i), Desc = i == 0 ? "Main" : "Worker" + i, Sym = "game.exe+0x1", Rate = (ulong)(1000 - i * 50) }).ToList() };
        var p1 = AutoLogic.Plan(t, s, 0xFFFF);
        Assert.True(p1.Count <= 6);   // 8 cores in the mask, CPU0's core skipped, one left: 7 − 1
        var prev = p1.ToDictionary(c => c.Row.Tid, c => c.LP);
        var p2 = AutoLogic.Plan(t, s, 0xFFFF, prev);
        Assert.All(p2, c => Assert.Equal(prev[c.Row.Tid], c.LP));
        Assert.Equal(0.2, AutoLogic.CompositionChange(new uint[] { 1, 2, 3, 4, 5 }, new uint[] { 1, 2, 3, 4 }), 3);
        Assert.Equal(0.0, AutoLogic.CompositionChange(new uint[] { 1, 2 }, new uint[] { 2, 1 }));
    }
}
