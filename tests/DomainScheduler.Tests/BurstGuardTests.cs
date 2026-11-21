using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

public class BurstGuardTests
{
    sealed class FakeNative : IBurstNative
    {
        public readonly Dictionary<uint, (long Created, long Cpu)> Procs = new();
        public readonly Dictionary<IntPtr, bool> Jobs = new();     // handle -> capped
        public readonly List<uint> CappedPids = new();
        public ulong Avail = 16UL << 30, Total = 32UL << 30, Ws = 3UL << 30;
        public (ulong Min, bool Hard)? SetWs;
        public bool Refuse;
        int _next = 1;
        public bool CpuTime(uint pid, out long created, out long cpu)
        {
            created = cpu = 0;
            if (!Procs.TryGetValue(pid, out var p)) return false;
            created = p.Created; cpu = p.Cpu; return true;
        }
        public int LastWeight = -1;
        public bool Cap(uint pid, string jobName, uint cpuRate, int weight, out IntPtr job, out int error)
        {
            job = IntPtr.Zero; error = 5;
            if (Refuse) return false;
            LastWeight = weight; job = new IntPtr(_next++); Jobs[job] = true; CappedPids.Add(pid); error = 0; return true;
        }
        public void Lift(IntPtr job) { if (Jobs.ContainsKey(job)) Jobs[job] = false; }
        public bool LiftByName(string jobName) => true;
        public void Close(IntPtr job) { }
        public bool Memory(out ulong avail, out ulong total) { avail = Avail; total = Total; return true; }
        public bool WorkingSet(uint pid, out ulong ws, out ulong min, out ulong max) { ws = Ws; min = 200 << 10; max = 1380 << 10; return true; }
        public bool SetWorkingSetMin(uint pid, ulong min, ulong max, bool hard) { SetWs = (min, hard); return true; }
        /// <summary>Adds cores×seconds of CPU time to a process.</summary>
        public void Burn(uint pid, double cores, double seconds) { var p = Procs[pid]; Procs[pid] = (p.Created, p.Cpu + (long)(cores * seconds * 1e7)); }
    }

    static (BurstGuard G, FakeNative N, Func<DateTime> Clock, Action<double> Advance) Make()
    {
        var n = new FakeNative();
        DateTime t = new(2026, 11, 21, 12, 0, 0, DateTimeKind.Utc);
        var g = new BurstGuard(n, () => t) { On = true, Logical = 16, Log = (_, _) => { } };
        return (g, n, () => t, s => t = t.AddSeconds(s));
    }

    [Fact]
    public void PolicyNumbers()
    {
        Assert.Equal(1.0, BurstPolicy.Cores(20_000_000, 2), 3);
        Assert.Equal(312u, BurstPolicy.CpuRate(0.5, 16));                 // half a processor of 16 = 3.12% of the machine
        Assert.Equal(10000u, BurstPolicy.CpuRate(64, 16));
        Assert.True(BurstPolicy.MemoryPressure(1UL << 30, 16UL << 30));     // 1 GB free of 16
        Assert.False(BurstPolicy.MemoryPressure(8UL << 30, 16UL << 30));
        Assert.Equal(4UL << 30, BurstPolicy.GameMinimum(6UL << 30, 16UL << 30));   // at most 25% of RAM
        Assert.Equal(0UL, BurstPolicy.GameMinimum(100UL << 20, 16UL << 30));       // too small to bother
    }

    [Fact]
    public void OnlyAProgramHeavyForThreeSamplesIsCappedAndReleasedWithTheGame()
    {
        var (g, n, _, adv) = Make();
        n.Procs[10] = (1, 0); n.Procs[20] = (2, 0); n.Procs[30] = (3, 0); n.Procs[99] = (9, 0);
        var cands = new[] { ((uint)20, "heavy.exe"), ((uint)30, "light.exe") };
        g.Tick(99, 9, true, cands, 10);                   // first sample: baseline only
        for (int i = 0; i < 2; i++) { adv(2); n.Burn(20, 1.5, 2); n.Burn(30, 0.1, 2); g.Tick(99, 9, true, cands, 10); }
        Assert.Empty(n.CappedPids);                       // two heavy samples are not enough
        adv(2); n.Burn(20, 1.5, 2); n.Burn(30, 0.1, 2); g.Tick(99, 9, true, cands, 10);
        Assert.Equal(new uint[] { 20 }, n.CappedPids);
        Assert.Equal(1, g.CappedCount);
        Assert.Equal(BurstPolicy.SoftWeight, n.LastWeight);    // soft mode by default: weight, not a hard cap
        g.Release();
        Assert.Equal(0, g.CappedCount);
        Assert.All(n.Jobs.Values, capped => Assert.False(capped));
    }

    [Fact]
    public void NetworkBoostersAndTheForegroundAreNeverCapped()
    {
        var (g, n, _, adv) = Make();
        n.Procs[20] = (2, 0); n.Procs[21] = (3, 0); n.Procs[99] = (9, 0);
        var cands = new[] { ((uint)20, "ExitLag.exe"), ((uint)21, "busy.exe") };
        g.Tick(99, 9, true, cands, 21);
        for (int i = 0; i < 5; i++) { adv(2); n.Burn(20, 2, 2); n.Burn(21, 2, 2); g.Tick(99, 9, true, cands, 21); }
        Assert.Empty(n.CappedPids);
    }

    [Fact]
    public void ACappedProgramComingToTheFrontIsReleased()
    {
        var (g, n, _, adv) = Make();
        n.Procs[20] = (2, 0); n.Procs[99] = (9, 0);
        var cands = new[] { ((uint)20, "busy.exe") };
        g.Tick(99, 9, true, cands, 1);
        for (int i = 0; i < 3; i++) { adv(2); n.Burn(20, 1, 2); g.Tick(99, 9, true, cands, 1); }
        Assert.Equal(1, g.CappedCount);
        adv(2); g.Tick(99, 9, true, cands, 20);
        Assert.Equal(0, g.CappedCount);
    }

    [Fact]
    public void GameMemoryIsHeldOnlyUnderPressureAndOnlyWhenTheGameMayBeTouched()
    {
        var (g, n, _, adv) = Make();
        n.Procs[99] = (9, 0);
        g.Tick(99, 9, true, Array.Empty<(uint, string)>(), 1);
        Assert.Null(n.SetWs);                                    // plenty of free memory
        n.Avail = 1UL << 30;
        adv(2); g.Tick(99, 9, false, Array.Empty<(uint, string)>(), 1);
        Assert.Null(n.SetWs);                                    // the game may not be touched (anti-cheat)
        adv(2); g.Tick(99, 9, true, Array.Empty<(uint, string)>(), 1);
        Assert.Equal((3UL << 30, true), n.SetWs);
        g.Release();
        Assert.False(n.SetWs!.Value.Hard);                       // put back with the game
    }

    [Fact]
    public void OffDoesNothingAndARefusedCapIsNotRetried()
    {
        var (g, n, _, adv) = Make();
        n.Procs[20] = (2, 0); n.Procs[99] = (9, 0); n.Refuse = true;
        var cands = new[] { ((uint)20, "busy.exe") };
        g.Tick(99, 9, true, cands, 1);
        for (int i = 0; i < 8; i++) { adv(2); n.Burn(20, 1, 2); g.Tick(99, 9, true, cands, 1); }
        Assert.Equal(0, g.CappedCount);
        g.On = false; n.Refuse = false; n.Avail = 1;
        adv(2); g.Tick(99, 9, true, cands, 1);
        Assert.Null(n.SetWs);
    }

    [Fact]
    public void HardModeUsesTheCap()
    {
        var (g, n, _, adv) = Make();
        g.Soft = false;
        n.Procs[20] = (2, 0); n.Procs[99] = (9, 0);
        var cands = new[] { ((uint)20, "busy.exe") };
        g.Tick(99, 9, true, cands, 1);
        for (int i = 0; i < 3; i++) { adv(2); n.Burn(20, 1, 2); g.Tick(99, 9, true, cands, 1); }
        Assert.Equal(0, n.LastWeight);
    }
}
