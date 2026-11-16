using Xunit;

namespace DomainScheduler.Tests;

/// <summary>The dynamic Top-5 mode: the slot logic alone, then together with Auto on a fake Windows.</summary>
[Collection("static-hooks")]
public class Top5Tests : IDisposable
{
    DateTime _now = new(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);
    readonly FakeNative _f = FakeNative.Cs2();
    readonly FakeProbe _p = new();
    readonly Engine _e;
    readonly AutoController _c;
    readonly Dictionary<uint, ulong> _rates = new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [102] = 300_000_000, [103] = 900_000_000, [104] = 800_000_000, [105] = 1_200_000_000, [106] = 9_000_000_000 };

    public Top5Tests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        GameMask.IrqHook = () => new();
        GameMask.NoisyHook = null;
        Reservation.Reg = new FakeReserved();
        Settings.Backend = new MemorySettingsBackend();
        AppLog.Clear();
        _f.Procs[1] = new FakeNative.P { Name = "DomainScheduler.exe", Created = 1 };
        _e = new Engine(_f) { Now = () => _now, Sleep = Advance, GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false) };
        _c = new AutoController(_e, _p, new NoFrames(), () => _now) { SaveSession = _ => { }, AutostartLine = () => "" };
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
        _f.Run(_rates, ms / 1000.0);
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

    static Top5Plan.Cand C(uint tid, double share, ThreadRole role = ThreadRole.Worker) => new() { Tid = tid, Share = share, Role = role };

    // ---- the slot logic ----------------------------------------------------------------------------

    [Fact]
    public void SlotsFillByScoreTwoMovesPerTickAndRolesGetTheirPriority()
    {
        var plan = new Top5Plan();
        plan.SetCores(new uint[] { 2, 4, 6, 8, 10, 12, 14 });
        var cands = new[] { C(1, 0.40, ThreadRole.Main), C(2, 0.20, ThreadRole.Render), C(3, 0.12), C(4, 0.10), C(5, 0.02, ThreadRole.Audio), C(6, 0.09), C(7, 0.01) };
        DateTime t = _now;
        Assert.Equal(2, plan.Step(t, cands, null));                       // never more than two moves in a tick
        Assert.Equal(new uint[] { 1, 2 }, plan.Desired().Select(s => s.Tid));
        plan.Step(t = t.AddSeconds(1), cands, null);
        plan.Step(t = t.AddSeconds(1), cands, null);
        var d = plan.Desired();
        Assert.Equal(new uint[] { 1, 2, 3, 4, 5 }, d.Select(s => s.Tid));  // audio: 2% of the load, but its role earns the slot
        Assert.Equal(new uint[] { 2, 4, 6, 8, 10 }, d.Select(s => s.LP));  // one processor each, never processor 0
        Assert.Equal(new int?[] { 1, 1, null, null, 2 }, d.Select(s => s.Prio));
        Assert.DoesNotContain(d, s => s.Tid == 7);                         // too quiet for a slot
        int moves = plan.Moves;
        for (int i = 0; i < 20; i++) plan.Step(t = t.AddSeconds(1), cands, null);
        Assert.Equal(moves, plan.Moves);                                   // a steady load moves nothing
    }

    [Fact]
    public void AChallengerNeedsALeadForTwoTicksAndThreadsAreNotMovedTwiceInARow()
    {
        var plan = new Top5Plan();
        plan.SetCores(new uint[] { 2, 4, 6, 8, 10 });
        var steady = new[] { C(1, 0.40), C(2, 0.20), C(3, 0.12), C(4, 0.10), C(5, 0.08), C(6, 0.07) };
        DateTime t = _now;
        for (int i = 0; i < 6; i++) plan.Step(t = t.AddSeconds(1), steady, null);
        Assert.Equal(new uint[] { 1, 2, 3, 4, 5 }, plan.Desired().Select(s => s.Tid));

        // Thread 6 is only a little better than the weakest: not enough.
        var close = new[] { C(1, 0.40), C(2, 0.20), C(3, 0.12), C(4, 0.10), C(5, 0.08), C(6, 0.085) };
        for (int i = 0; i < 10; i++) plan.Step(t = t.AddSeconds(1), close, null);
        Assert.Contains(plan.Desired(), s => s.Tid == 5);

        // One tick of lead does not move anything; the second one does, into the weakest slot.
        var hot = new[] { C(1, 0.40), C(2, 0.20), C(3, 0.12), C(4, 0.10), C(5, 0.08), C(6, 0.60) };
        int before = plan.Moves;
        plan.Step(t = t.AddSeconds(1), hot, null);
        Assert.Equal(before, plan.Moves);
        Assert.Contains(plan.Desired(), s => s.Tid == 5);
        plan.Step(t = t.AddSeconds(1), hot, null);
        Assert.DoesNotContain(plan.Desired(), s => s.Tid == 5);
        var six = plan.Desired().First(s => s.Tid == 6);
        Assert.Equal(4, six.Index);
        Assert.Equal(10u, six.LP);

        // It climbs one slot at a time, never twice within the dwell time, and ends on top.
        var seen = new List<int> { six.Index };
        DateTime lastMove = t;
        for (int i = 0; i < 30; i++)
        {
            plan.Step(t = t.AddMilliseconds(500), hot, null);
            int idx = plan.Desired().First(s => s.Tid == 6).Index;
            if (idx != seen[^1])
            {
                Assert.Equal(seen[^1] - 1, idx);
                Assert.True((t - lastMove).TotalMilliseconds >= 2000);
                lastMove = t;
                seen.Add(idx);
            }
        }
        Assert.Equal(0, seen[^1]);
        Assert.Equal(1, plan.Desired()[0].Prio);
    }

    [Fact]
    public void ASlotLeavesAProcessorThatOtherWorkKeepsBusy()
    {
        var plan = new Top5Plan();
        plan.SetCores(new uint[] { 2, 4, 6, 8, 10, 12, 14 });
        var cands = new[] { C(1, 0.40), C(2, 0.20), C(3, 0.12), C(4, 0.10), C(5, 0.08) };
        DateTime t = _now;
        var load = new Dictionary<uint, double>();
        double Foreign(uint lp) => load.GetValueOrDefault(lp);
        for (int i = 0; i < 6; i++) plan.Step(t = t.AddSeconds(1), cands, Foreign);
        load[4] = 0.5;                 // somebody else works on processor 4
        load[12] = 0.3;                // one spare is busy too, the other is quiet
        plan.Step(t = t.AddSeconds(1), cands, Foreign);
        plan.Step(t = t.AddSeconds(1), cands, Foreign);
        Assert.Equal(4u, plan.Desired()[1].LP);                            // two seconds are not enough
        plan.Step(t = t.AddSeconds(1), cands, Foreign);
        Assert.Equal(14u, plan.Desired()[1].LP);
        Assert.Contains(plan.Events, e => e.Contains("busy with other work"));
        load[14] = 0.5;                // now 14 is busy; 4 is still busy, 12 too: nowhere better to go
        for (int i = 0; i < 6; i++) plan.Step(t = t.AddSeconds(1), cands, Foreign);
        Assert.Equal(14u, plan.Desired()[1].LP);
    }

    [Fact]
    public void QuietAndEndedThreadsFreeTheirSlot()
    {
        var plan = new Top5Plan();
        plan.SetCores(new uint[] { 2, 4, 6 });
        Assert.Equal(3, plan.SlotCount);                                   // fewer processors: fewer slots
        DateTime t = _now;
        var cands = new[] { C(1, 0.5), C(2, 0.3), C(3, 0.2) };
        for (int i = 0; i < 4; i++) plan.Step(t = t.AddSeconds(1), cands, null);
        Assert.Equal(3, plan.Desired().Count);
        for (int i = 0; i < 20; i++) plan.Step(t = t.AddSeconds(1), new[] { C(1, 0.99), C(2, 0.01), C(3, 0.0) }, null);
        Assert.Equal(new uint[] { 1 }, plan.Desired().Select(s => s.Tid));  // nobody is kept in a slot for nothing
        plan.Step(t = t.AddSeconds(1), Array.Empty<Top5Plan.Cand>(), null);
        Assert.Empty(plan.Desired());
    }

    // ---- with Auto on a fake Windows ---------------------------------------------------------------

    [Fact]
    public void DynamicModePlacesFollowsTheLoadAndPutsEverythingBack()
    {
        _c.SetTop5(true);
        GameToActive();
        Seconds(6);
        var view = _c.Dynamic.View();
        Assert.Equal(5, view.Count);
        Assert.Equal(new uint[] { 100, 101 }, view.Take(2).Select(s => s.Tid));
        Assert.Equal(5, view.Select(s => s.LP).Distinct().Count());
        Assert.DoesNotContain(view, s => s.LP == 0);
        Assert.DoesNotContain(view, s => s.Tid == 106);                    // the anti-cheat thread is the hottest of all and is never a candidate
        Assert.Equal(0, _f.Threads[106].Prio);
        Assert.Equal(999u, _f.Threads[106].Ideal);
        Assert.Equal(1, _f.Threads[100].Prio);
        Assert.Equal(1, _f.Threads[101].Prio);
        Assert.Equal(2, _f.Threads[102].Prio);                             // audio
        Assert.Equal(view.First(s => s.Tid == 100).LP, _f.Threads[100].Ideal);
        Assert.Contains(view, s => s.Tid == 103);
        Assert.Equal(0, _f.Threads[103].Prio);                             // a worker keeps its own priority
        Assert.Equal(32768u, _f.Procs[500].Cls);                           // class and mask still come from Auto
        Assert.Contains("dynamic Top-5", _c.Status);

        // A steady load: no writes at all, and no thread is opened again.
        long sets = _e.SetCalls;
        int opens = _f.ThreadOpens;
        Seconds(20);
        Assert.Equal(sets, _e.SetCalls);
        Assert.True(_f.ThreadOpens - opens <= 8, $"threads were reopened: {_f.ThreadOpens - opens}");   // at most the half-minute refresh of the names

        // The load changes: the idle helper becomes the hottest thread and takes over.
        _rates[107] = 20_000_000_000;
        Seconds(20);
        view = _c.Dynamic.View();
        Assert.Equal(107u, view[0].Tid);
        Assert.Equal(1, _f.Threads[107].Prio);
        Assert.Equal(view[0].LP, _f.Threads[107].Ideal);
        var outNow = new uint[] { 100, 101, 102, 103, 105 }.Where(t => view.All(s => s.Tid != t)).ToList();
        Assert.Single(outNow);                                             // exactly one thread lost its slot…
        Assert.Equal(0, _f.Threads[outNow[0]].Prio);                       // …and got its own priority and processor back
        Assert.Equal(999u, _f.Threads[outNow[0]].Ideal);
        Assert.True(_c.Dynamic.Moves > 5);
        Assert.Contains("dynamic Top-5", _c.Report());

        // Off: everything is as it was before the program.
        _c.Click();
        Assert.Equal(AutoState.Idle, _c.State);
        foreach (var t in _f.Threads.Values.Where(t => t.Pid == 500))
        {
            Assert.Equal(0, t.Prio);
            Assert.Equal(999u, t.Ideal);
        }
        Assert.Equal(32u, _f.Procs[500].Cls);
        Assert.False(_c.Dynamic.Running);
    }

    [Fact]
    public void SwitchingTheModeOnAnActiveGameChangesThePlanCleanly()
    {
        GameToActive();
        Assert.False(_c.Top5Mode);
        Assert.NotEmpty(_c.PlanView());
        Assert.False(_c.Dynamic.Running);

        _c.SetTop5(true);
        Assert.Equal(AutoState.Active, _c.State);
        Assert.Empty(_c.PlanView());
        Assert.True(_c.Dynamic.Running);
        Seconds(6);
        Assert.Equal(5, _c.Dynamic.View().Count);

        _c.SetTop5(false);
        Assert.False(_c.Dynamic.Running);
        Assert.NotEmpty(_c.PlanView());
        Assert.Equal(1, _f.Threads[100].Prio);                             // the role plan is back
        Seconds(3);
        _c.Click();
        Assert.All(_f.Threads.Values.Where(t => t.Pid == 500), t => Assert.Equal(0, t.Prio));
    }

    [Fact]
    public void ADenylistedGameIsNeverStarted()
    {
        var top = new Top5(_e);
        _f.Procs[900] = new FakeNative.P { Name = "lsass.exe", Created = 3 };
        _f.Threads[901] = new FakeNative.T { Pid = 900, Born = 1 };
        Assert.False(top.Start(900, "lsass.exe", Synth.X7800X3D(), 0xFFFF));
        Assert.Equal(0, top.Tick(_now, new uint[] { 901 }));
        Assert.DoesNotContain(900u, _f.OpenedPids);
        Assert.Equal(0, _f.Writes);
    }
}
