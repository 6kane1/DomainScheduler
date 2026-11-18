using System.Globalization;
using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>1.14.0: the HUD home screen's frame numbers, chart reduction, processor map and timer text.</summary>
public class HudTests
{
    static List<double> Flat(int n, double ms) => Enumerable.Repeat(ms, n).ToList();

    [Fact]
    public void TooFewFramesGiveNoNumbers()
    {
        var s = HudMath.Stats(Flat(HudMath.MinFrames - 1, 5));
        Assert.False(s.Valid);
        Assert.Equal(0, s.Low1Fps);
        Assert.False(HudMath.Stats(null).Valid);
    }

    [Fact]
    public void SteadyFramesGiveEqualNumbers()
    {
        var s = HudMath.Stats(Flat(1200, 4));
        Assert.True(s.Valid);
        Assert.Equal(250, s.AvgFps, 6);
        Assert.Equal(250, s.Low1Fps, 6);
        Assert.Equal(250, s.Low01Fps, 6);
        Assert.Equal(4, s.FrameMs, 6);
        Assert.Equal(4, s.MedianMs, 6);
        Assert.Equal(0, s.Hitches);
        Assert.Equal(4.8, s.SpanSec, 6);
    }

    [Fact]
    public void ZeroPointOneLowNeedsAThousandFrames()
    {
        var s = HudMath.Stats(Flat(500, 4));
        Assert.True(s.Valid);
        Assert.Equal(0, s.Low01Fps);
    }

    [Fact]
    public void HitchesAreFramesLongerThanTwoAndAHalfMedians()
    {
        var list = Flat(2000, 4);
        list[100] = 10.0;      // exactly 2.5 medians: not a hitch
        list[200] = 10.1;      // a hitch
        list[300] = 40;        // a hitch
        var s = HudMath.Stats(list);
        Assert.Equal(2, s.Hitches);
        Assert.True(HudMath.IsHitch(10.1, 4));
        Assert.False(HudMath.IsHitch(10, 4));
        Assert.False(HudMath.IsHitch(100, 0));
        // 2000 frames ≈ 8 s: 2 hitches are 15 a minute.
        Assert.Equal(2 * 60.0 / s.SpanSec, s.HitchesPerMin, 6);
    }

    [Fact]
    public void OnePercentLowIsTheNinetyNinthPercentileFrame()
    {
        var list = Flat(1000, 5);
        for (int i = 0; i < 11; i++) list[i * 7] = 10;    // just over 1% of the frames are slow
        var s = HudMath.Stats(list);
        Assert.Equal(100, s.Low1Fps, 6);
        Assert.True(s.AvgFps < 200 && s.AvgFps > 190);
    }

    [Fact]
    public void BucketsKeepTheLongestFrameAndAlignToTheEnd()
    {
        var t0 = new DateTime(2026, 11, 18, 12, 0, 0, DateTimeKind.Utc);
        var times = new List<DateTime>();
        var t = t0;
        for (int i = 0; i < 400; i++) { t = t.AddMilliseconds(5); times.Add(t); }   // 2 s of 5 ms frames
        t = t.AddMilliseconds(30); times.Add(t);                                    // one hitch
        for (int i = 0; i < 100; i++) { t = t.AddMilliseconds(5); times.Add(t); }
        var end = HudMath.AlignEnd(t.AddSeconds(1), TimeSpan.FromSeconds(1), TimeSpan.FromMilliseconds(250));
        Assert.Equal(0, end.Ticks % TimeSpan.FromMilliseconds(250).Ticks);
        var max = new double[240];
        var endMax = new double[240];
        HudMath.Buckets(times, end, TimeSpan.FromMilliseconds(250), max, endMax);
        Assert.Equal(30, max.Max(), 6);
        Assert.Equal(1, endMax.Count(x => x == 30));
        // Buckets before the first frame stay empty; the ones with frames are 5 ms or the hitch.
        Assert.Equal(0, max[0]);
        Assert.All(max.Where(x => x > 0), x => Assert.True(x == 5 || x == 30));
        Assert.True(max.Count(x => x > 0) >= 10);
    }

    [Fact]
    public void ALongFrameCoversEveryBucketItSpans()
    {
        var b = TimeSpan.FromMilliseconds(250);
        var end = new DateTime(2026, 1, 1, 0, 0, 10, DateTimeKind.Utc);
        var times = new List<DateTime> { end.AddSeconds(-3), end.AddSeconds(-2), end.AddSeconds(-1).AddMilliseconds(-10), end.AddSeconds(-1) };
        var max = new double[8];
        var endMax = new double[8];
        HudMath.Buckets(times, end, b, max, endMax);
        // The 990 ms frame from -2 s to -1.01 s spans buckets 0..3 (the window starts at -2 s); the last 10 ms frame ends at -1 s.
        Assert.Equal(990, max[0], 6);
        Assert.Equal(990, max[3], 6);
        Assert.Equal(990, endMax[3], 6);
        Assert.Equal(0, max[4]);
        Assert.Equal(0, endMax[0]);
    }

    [Fact]
    public void FramesAfterTheEndWaitForTheNextUpdate()
    {
        var b = TimeSpan.FromMilliseconds(250);
        var end = new DateTime(2026, 1, 1, 0, 0, 10, DateTimeKind.Utc);
        var times = new List<DateTime> { end.AddMilliseconds(-20), end.AddMilliseconds(-10), end.AddMilliseconds(30) };
        var max = new double[4];
        HudMath.Buckets(times, end, b, max, null);
        Assert.Equal(10, max[3], 6);
        Assert.Equal(0, max[2]);
    }

    [Fact]
    public void ChartTopIsNiceAndIgnoresLongPauses()
    {
        Assert.Equal(1, HudMath.Nice(0.9));
        Assert.Equal(2, HudMath.Nice(1.3));
        Assert.Equal(2.5, HudMath.Nice(2.2));
        Assert.Equal(5, HudMath.Nice(4.4));
        Assert.Equal(10, HudMath.Nice(7));
        Assert.Equal(20, HudMath.Nice(11));
        Assert.Equal(1, HudMath.Nice(double.NaN));
        // A 4.2 ms spike over 1.6 ms frames: the spike fits.
        Assert.Equal(5, HudMath.ChartTop(4.2, 2.3, 1.6));
        // A 3 s pause over 1.6 ms frames is capped at 8 medians (12.8 ms -> 20).
        Assert.Equal(20, HudMath.ChartTop(3000, 2.3, 1.6));
        // The 1% low line always fits.
        Assert.True(HudMath.ChartTop(2, 4, 1.6) >= 4 * 1.3);
    }

    [Fact]
    public void CoreMapClassification()
    {
        ulong game = 0xFF, irq = 0b1_0100_0000_0000 | 0b10;   // processors 10, 12 and 1
        Assert.Equal(CoreKind.Game, HudMath.Classify(0, game, irq));
        Assert.Equal(CoreKind.Game, HudMath.Classify(1, game, irq));     // the game's even if an interrupt is there
        Assert.Equal(CoreKind.Interrupt, HudMath.Classify(10, game, irq));
        Assert.Equal(CoreKind.Interrupt, HudMath.Classify(12, game, irq));
        Assert.Equal(CoreKind.Background, HudMath.Classify(11, game, irq));
        Assert.Equal(CoreKind.Background, HudMath.Classify(64, ulong.MaxValue, 0));
        Assert.Equal(CoreKind.Game, HudMath.Classify(63, 1UL << 63, 0));
    }

    [Fact]
    public void SessionTimerText()
    {
        Assert.Equal("00:00:00", HudMath.Elapsed(TimeSpan.FromSeconds(-5)));
        Assert.Equal("00:42:17", HudMath.Elapsed(new TimeSpan(0, 42, 17)));
        Assert.Equal("27:03:09", HudMath.Elapsed(new TimeSpan(1, 3, 3, 9)));
        Assert.Equal("123:00:00", HudMath.Elapsed(TimeSpan.FromHours(123)));
    }

    [Fact]
    public void SpacedCaps()
    {
        Assert.Equal("O K", HudMath.Spaced("ok", CultureInfo.InvariantCulture));
        Assert.Equal("", HudMath.Spaced(null));
        Assert.Equal("И Г Р А", HudMath.Spaced("Игра", new CultureInfo("ru-RU")));
        Assert.DoesNotContain(' ', HudMath.Spaced("frame time"));
    }
}
