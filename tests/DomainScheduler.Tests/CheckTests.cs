using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>1.16.0 Test 1: the Check page's arithmetic (single runs).</summary>
public class CheckTests
{
    const double Tps = 10_000_000;   // 100 ns ticks: easy numbers

    static long[] Presents(int n, double ms, long start = 0)
    {
        var p = new long[n + 1];
        for (int i = 0; i <= n; i++) p[i] = start + (long)(i * ms * 10_000);
        return p;
    }

    [Fact]
    public void FramesAndWaitsLikeSchedTest()
    {
        var pr = Presents(1000, 2.0).ToList();
        for (int i = 501; i < pr.Count; i++) pr[i] += 80_000;                    // one 10 ms frame in the middle
        long[] waits = Enumerable.Repeat(20L, 989).Concat(Enumerable.Repeat(6_000L, 11)).ToArray();   // 2 µs ×989, 600 µs ×11
        var r = CheckStats.From("with Auto", 1, waits, pr.ToArray(), 0, 300_000_000, Tps, 0);
        Assert.Equal(1000, r.Frames);
        Assert.Equal(10.0, r.MaxMs, 3);
        Assert.Equal(1, r.Hitches);
        Assert.Equal(11, r.Over500);
        Assert.Equal(600, r.WaitMaxUs, 3);
        Assert.Equal(600, r.WaitP99Us, 3);         // the 990th of 1000 sorted waits is the first 600 µs one
        Assert.Equal(1000 / 2.008, r.Fps, 0);
    }

    [Fact]
    public void EmptyRunDoesNotThrow()
    {
        var r = CheckStats.From("x", 1, Array.Empty<long>(), Array.Empty<long>(), 0, 0, Tps, 3);
        Assert.Equal(0, r.Frames);
        Assert.Equal(0, r.Fps);
        Assert.Equal(0, r.Waits);
    }

    [Fact]
    public void ReportListsTheRun()
    {
        var r = new CheckResult("with Auto", 1, 30, 1000, 700, 400, 200, 2.3, 8, 10, 1000, 4, 6, 100, 0, 2);
        string text = CheckStats.Report(new[] { r }, "cs2.exe", "header");
        Assert.Contains("cs2.exe", text);
        Assert.Contains("with Auto", text);
    }
}
