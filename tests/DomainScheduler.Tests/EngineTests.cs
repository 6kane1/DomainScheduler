using Xunit;

namespace DomainScheduler.Tests;

[Collection("static-hooks")]
public class EngineTests : IDisposable
{
    public EngineTests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        GameMask.IrqHook = () => new();
        Reservation.Reg = new FakeReserved();
    }

    public void Dispose()
    {
        Topo.LiveHook = null;
        GameMask.IrqHook = null;
        Reservation.Reg = new WinReservedRegistry();
    }

    static Preset Synth2(params (uint tid, string ideal, string prio, string boost)[] rules)
    {
        var p = new Preset { Exe = "cs2.exe", Class = "above", Io = "normal", Mem = "normal", Eco = "off" };
        foreach (var r in rules)
            p.Threads.Add(new ThreadRule { Match = "tid", Pattern = r.tid.ToString(), Ideal = r.ideal, Prio = r.prio, Boost = r.boost });
        return p;
    }

    static ApplyOptions Opts(bool warmup = false) => new() { DoMask = true, DoThreads = true, Warmup = warmup };

    (Engine e, FakeNative f, DateTime[] now) Make()
    {
        var f = FakeNative.Cs2();
        var now = new[] { new DateTime(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc) };
        var e = new Engine(f) { Now = () => now[0], Sleep = ms => { now[0] = now[0].AddMilliseconds(ms); f.Run(new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [103] = 1_000_000_000 }, ms / 1000.0); } };
        return (e, f, now);
    }

    [Fact]
    public void EcoQosOffInWindowsIsNotAFailureAndIsNotRetried()
    {
        var (e, f, now) = Make();
        f.EcoError = 1;
        var preset = Synth2((100, "2", "above", "on"));
        var r = e.ApplyAuto(500, preset, 0xFFFC, Opts(true));
        Assert.Empty(r.Failures);
        long sets = e.SetCalls;
        now[0] = now[0].AddSeconds(5);
        e.ApplyAuto(500, preset, 0xFFFC, Opts());
        Assert.Equal(sets, e.SetCalls);
    }

    [Fact]
    public void SecondApplyWritesNothing()
    {
        var (e, f, now) = Make();
        var preset = Synth2((100, "2", "above", "on"), (101, "4", "above", "off"), (103, "6", "", "off"));
        var r = e.ApplyAuto(500, preset, 0xFFFC, Opts(true));
        Assert.Empty(r.Failures);
        Assert.Equal(32768u, f.Procs[500].Cls);
        Assert.Equal(1u, f.Procs[500].EcoCtl);
        Assert.Equal(0xFFFCUL, f.Procs[500].Aff);
        Assert.NotEmpty(f.Procs[500].Sets);
        Assert.Equal(2u, f.Threads[100].Ideal);
        Assert.Equal(4u, f.Threads[101].Ideal);
        Assert.Equal(1, f.Threads[100].Prio);
        Assert.True(f.Threads[101].BoostDisabled);
        long sets = e.SetCalls;
        now[0] = now[0].AddSeconds(5);
        e.ApplyAuto(500, preset, 0xFFFC, Opts());
        Assert.Equal(sets, e.SetCalls);
        Assert.Equal(1, f.Threads[100].IdealSets);
        // The game changes a value itself: only that is written again.
        f.Procs[500].Cls = 32;
        e.ApplyAuto(500, preset, 0xFFFC, Opts());
        Assert.Equal(sets + 1, e.SetCalls);
    }

    [Fact]
    public void AutoIdealIsNeverReassigned()
    {
        var (e, f, _) = Make();
        var preset = new Preset { Exe = "cs2.exe", Class = "", Io = "", Mem = "", Eco = "", Threads = { new ThreadRule { Match = "module", Pattern = "tier0.dll", Ideal = "auto" } } };
        e.ApplyAuto(500, preset, 0xFFFF, Opts());
        var lps = new uint[] { 101, 102, 103, 104, 107 }.Select(t => f.Threads[t].Ideal).ToList();
        Assert.Equal(lps.Count, lps.Distinct().Count());
        Assert.All(lps, lp => Assert.True(lp % 2 == 0));   // first hardware thread of each core
        e.ApplyAuto(500, preset, 0xFFFF, Opts());
        Assert.All(new uint[] { 101, 102, 103, 104, 107 }, t => Assert.Equal(1, f.Threads[t].IdealSets));
    }

    [Fact]
    public void TidMatchHasNoFalseHits()
    {
        var rows = new List<ThreadRow> { new() { Tid = 100, Desc = "101", Sym = "101.dll+0x1" }, new() { Tid = 101, Desc = "x" } };
        var hits = Engine.RuleHits(new ThreadRule { Match = "tid", Pattern = "101" }, rows);
        Assert.Single(hits);
        Assert.Equal(101u, hits[0].Tid);
        Assert.Empty(Engine.RuleHits(new ThreadRule { Match = "tid", Pattern = "10*" }, rows));
        Assert.Single(Engine.RuleHits(new ThreadRule { Match = "module", Pattern = "1*" }, rows));   // "101.dll" without extension
    }

    [Fact]
    public void AntiCheatThreadsAreSkipped()
    {
        var (e, f, _) = Make();
        e.ApplyAuto(500, Synth2((106, "6", "highest", "")), 0xFFFF, Opts());
        Assert.Equal(0, f.Threads[106].Prio);
        Assert.Equal(999u, f.Threads[106].Ideal);
        Assert.Contains(e.Advice, a => a.Contains("EasyAntiCheat"));
    }

    [Fact]
    public void GateNeverOpensTheProcess()
    {
        var (e, f, _) = Make();
        e.GateHook = (pid, exe) => exe == "cs2.exe" ? "cs2.exe runs under FACEIT" : null;
        var r = e.ApplyAuto(500, Synth2((100, "2", "above", "")), 0xFFFF, Opts());
        Assert.NotEmpty(r.Failures);
        Assert.DoesNotContain(500u, f.OpenedPids);
        Assert.Equal(0, f.Writes);
    }

    [Fact]
    public void RevertIsIdempotentAndSkipsReusedIds()
    {
        var (e, f, _) = Make();
        f.Procs[500].Aff = 0xFFFF;
        e.ApplyAuto(500, Synth2((100, "2", "above", "on"), (101, "4", "above", "off")), 0xFFFC, Opts(true));
        f.Threads[101].Born = 9999;   // thread 101 ended, its ID reused
        Assert.True(e.Revert(500) > 0);
        Assert.Equal(32u, f.Procs[500].Cls);
        Assert.Equal(0xFFFFUL, f.Procs[500].Aff);
        Assert.Empty(f.Procs[500].Sets);
        Assert.Equal(0u, f.Procs[500].EcoCtl);
        Assert.Equal(0, f.Threads[100].Prio);
        Assert.True(f.Threads[101].BoostDisabled);   // not the same thread: left alone
        long w = f.Writes;
        Assert.Equal(0, e.Revert(500));
        Assert.Equal(0, e.RevertAll());
        Assert.Equal(w, f.Writes);
    }

    [Fact]
    public void BackgroundOnlyLowersAndReverts()
    {
        var (e, f, _) = Make();
        f.Procs[800] = new FakeNative.P { Name = "Discord.exe", Created = 5 };
        f.Procs[801] = new FakeNative.P { Name = "idle.exe", Created = 6, Cls = 64, Io = 0, Mem = 1, EcoCtl = 1, EcoState = 1 };
        f.Procs[802] = new FakeNative.P { Name = "FACEITService.exe", Created = 7 };
        int saved = 0;
        e.BeforeMutate = () => saved++;
        Assert.Equal(2, e.ApplyBackground(new uint[] { 800, 801, 802 }, 0x3, true));   // 801 is already low but still gets the background mask
        Assert.True(saved > 0);
        var d = f.Procs[800];
        Assert.Equal(16384u, d.Cls);
        Assert.Equal(0u, d.Io);
        Assert.Equal(2u, d.Mem);
        Assert.Equal(1u, d.EcoState);
        Assert.Equal(0x3UL, d.Aff);
        Assert.Equal(64u, f.Procs[801].Cls);   // never raised
        Assert.Equal(0x3UL, f.Procs[801].Aff);
        Assert.Equal(32u, f.Procs[802].Cls);   // anti-cheat: never touched
        Assert.Equal(0, e.ApplyBackground(new uint[] { 800, 801 }, 0x3, true));
        e.RevertBackground();
        Assert.Equal(32u, d.Cls);
        Assert.Equal(2u, d.Io);
        Assert.Equal(5u, d.Mem);
        Assert.Equal(ulong.MaxValue, d.Aff);
    }

    [Fact]
    public void PruneLeavesNothingAfterTheGameEnds()
    {
        var (e, f, now) = Make();
        var preset = Synth2((100, "2", "above", "on"), (101, "4", "above", "off"));
        for (int i = 0; i < 120; i++)
        {
            now[0] = now[0].AddSeconds(5);
            e.ApplyAuto(500, preset, 0xFFFF, Opts());
            if (i % 12 == 0) e.Prune();
        }
        Assert.True(e.Sizes().Meta > 0);
        f.Procs.Remove(500);
        e.Prune();
        Assert.Equal((0, 0, 0, 0), e.Sizes());
    }

    [Fact]
    public void CycleFallbackIsReported()
    {
        var (e, f, _) = Make();
        f.Threads[103].NoCycles = true;
        var s = e.Snapshot(500);
        Assert.Contains(103u, s.CycleFallbackTids);
        Assert.Equal("cs2.exe+0x100", s.Rows.First(r => r.Tid == 100).Sym);
    }

    [Fact]
    public void SessionRestoresOnlyMatchingProcesses()
    {
        var (e, f, _) = Make();
        f.Procs[800] = new FakeNative.P { Name = "Discord.exe", Created = 5 };
        e.ApplyAuto(500, Synth2((100, "2", "above", "")), 0xFFFC, Opts());
        e.ApplyBackground(new uint[] { 800 }, 0, false);
        var s = e.Export();
        Assert.Equal(2, s.Procs.Count);
        Assert.Single(s.Threads);
        // Crash: a new engine restores from the session; PID 800 now belongs to another process.
        f.Procs[800].Created = 999;
        var e2 = new Engine(f);
        Assert.True(e2.RestoreSession(s) > 0);
        Assert.Equal(32u, f.Procs[500].Cls);
        Assert.Equal(0, f.Threads[100].Prio);
        Assert.Equal(16384u, f.Procs[800].Cls);   // someone else's PID: skipped
    }
}
