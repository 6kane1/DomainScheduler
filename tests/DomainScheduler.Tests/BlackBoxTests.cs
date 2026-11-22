using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

public class BlackBoxTests
{
    const double Tpm = 10_000;             // 10 MHz clock: 10 000 ticks per ms
    const uint Game = 100, Steam = 900;

    static (List<long> P, int Hitch, long B, long E) Frames(double hitchMs)
    {
        var p = new List<long>(); long t = 0;
        for (int i = 0; i < 300; i++) { t += (long)(1.4 * Tpm); p.Add(t); }
        long b = t; t += (long)(hitchMs * Tpm); p.Add(t); int h = p.Count - 1;
        for (int i = 0; i < 100; i++) { t += (long)(1.4 * Tpm); p.Add(t); }
        return (p, h, b, p[h]);
    }

    static BbReport Explain(List<long> p, int h, List<BbWait> w, List<BbDpc> d) =>
        BlackBoxAnalyzer.Explain("cs2.exe", DateTime.Now, p, h, Tpm, w, d, new HashSet<uint> { Game },
            pid => pid == Steam ? "steamwebhelper.exe" : pid == 800 ? "lsass.exe" : "?", _ => "nvlddmkm.sys", 240);

    [Fact]
    public void StutterNeedsThreeTimesTheMedianAndTwoRefreshes()
    {
        Assert.True(BlackBoxPolicy.IsStutter(9, 1.4, 240));
        Assert.False(BlackBoxPolicy.IsStutter(4, 1.4, 240));      // 3x median but less than two refreshes (8.3 ms)
        Assert.False(BlackBoxPolicy.IsStutter(9, 4, 240));        // not 3x the median
        Assert.True(BlackBoxPolicy.IsStutter(9, 2, 0));           // refresh unknown: 8 ms
    }

    [Fact]
    public void CleanProcessorSideIsGameOrGpu()
    {
        var (p, h, b, _) = Frames(12);
        var w = new List<BbWait> { new(b + (long)(2 * Tpm), (long)(0.002 * Tpm), 3, 11, 12, 0, 0) };
        var r = Explain(p, h, w, new());
        Assert.Equal(BbCause.Game, r.Cause);
        Assert.Equal(12, r.FrameMs, 1);
        Assert.Contains("игры или видеокарты", r.TextRu);
    }

    [Fact]
    public void AnotherProgramHoldingTheCoreIsNamed()
    {
        var (p, h, b, _) = Frames(12);
        var w = new List<BbWait> { new(b + (long)(5 * Tpm), (long)(1.5 * Tpm), 3, 11, 12, 77, Steam) };
        var r = Explain(p, h, w, new());
        Assert.Equal(BbCause.Other, r.Cause);
        Assert.Equal("steamwebhelper.exe", r.Who);
        Assert.Contains("steamwebhelper.exe", r.TextRu);
        Assert.Equal(1500, r.WaitUs, 0);
    }

    [Fact]
    public void DriverCountsOnlyOnACoreTheGameUsed()
    {
        var (p, h, b, _) = Frames(12);
        var w = new List<BbWait> { new(b + (long)(4 * Tpm), (long)(0.002 * Tpm), 5, 11, 12, 0, 0) };   // main thread ran on cpu 5
        var off = Explain(p, h, w, new() { new(14, b + (long)(2 * Tpm), b + (long)(2.5 * Tpm), 1) });
        Assert.Equal(BbCause.Game, off.Cause);                                                          // DPC on a device core
        var on = Explain(p, h, w, new() { new(5, b + (long)(2 * Tpm), b + (long)(2.5 * Tpm), 1) });
        Assert.Equal(BbCause.Driver, on.Cause);
        Assert.Equal("nvlddmkm.sys", on.Who);
        Assert.Contains("видеокарты", on.AdviceRu);
    }

    [Fact]
    public void TheGamesOwnThreadsMeanNotEnoughCoresAndBackgroundThreadsNeverDecide()
    {
        var (p, h, b, _) = Frames(12);
        var cores = Explain(p, h, new() { new(b + (long)(5 * Tpm), (long)(0.8 * Tpm), 3, 11, 12, 55, Game) }, new());
        Assert.Equal(BbCause.Cores, cores.Cause);
        var bg = Explain(p, h, new() { new(b + (long)(5 * Tpm), (long)(3 * Tpm), 3, 11, 2, 77, Steam) }, new());   // priority 2: a worker
        Assert.Equal(BbCause.Game, bg.Cause);
    }

    [Fact]
    public void RingKeepsTheNewestInOrder()
    {
        var r = new Ring<int>(4);
        for (int i = 1; i <= 6; i++) r.Add(i);
        Assert.Equal(new[] { 3, 4, 5, 6 }, r.ToList());
        r.Clear(); r.Add(9);
        Assert.Equal(new[] { 9 }, r.ToList());
    }

    [Fact]
    public void LongPausesAreLoadingAndWindowsProcessesAreNeverToldToClose()
    {
        var (p, h, b, _) = Frames(210);
        var r = Explain(p, h, new() { new(b + (long)(5 * Tpm), (long)(20 * Tpm), 3, 11, 12, 77, Steam) }, new());
        Assert.Equal(BbCause.Loading, r.Cause);                                       // a 210 ms frame: load or map change
        var (p2, h2, b2, _) = Frames(12);
        var sys = Explain(p2, h2, new() { new(b2 + (long)(5 * Tpm), (long)(1.5 * Tpm), 3, 11, 12, 66, 800) }, new());
        Assert.Equal(BbCause.Other, sys.Cause);
        Assert.Equal("lsass.exe", sys.Who);
        Assert.DoesNotContain("Закройте", sys.AdviceRu);
        Assert.Contains("закрывать его нельзя", sys.AdviceRu);
    }
}
