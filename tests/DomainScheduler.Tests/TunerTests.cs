using Xunit;

namespace DomainScheduler.Tests;

public class TunerTests
{
    static readonly List<TunerVariant> V = new()
    {
        new("roles", false, false), new("roles + live", false, true), new("top5", true, false), new("top5 + live", true, true),
    };

    static List<double> Frames(double ms, double spikeMs, int spikes, int n = 2000)
    {
        var l = Enumerable.Repeat(ms, n).ToList();
        for (int i = 0; i < spikes; i++) l[i * 37 % n] = spikeMs;
        return l;
    }

    [Fact]
    public void ScheduleIsReversedInTheSecondRoundAndSkipsTheSettleTime()
    {
        var t = new AbTuner();
        var now = new DateTime(2026, 1, 1);
        var order = new List<string> { t.Start(V, 0, now).Name };
        var asked = new List<DateTime>();
        for (int s = 1; s <= (AbTuner.PhaseSeconds + AbTuner.LagSeconds) * 8; s++)
        {
            now = now.AddSeconds(1);
            var st = t.Tick(now, (a, b) => { asked.Add(a); return Frames(2, 2, 0, 50); }, out var next);
            if (st == AbTuner.Step.Switch) order.Add(next.Name);
        }
        Assert.Equal(new[] { "roles", "roles + live", "top5", "top5 + live", "top5 + live", "top5", "roles + live", "roles" }, order);
        Assert.False(t.Running);
        Assert.DoesNotContain(asked, a => a < new DateTime(2026, 1, 1).AddSeconds(AbTuner.SettleSeconds));
    }

    [Fact]
    public void FramesThatArriveLateAreStillCounted()
    {
        // Frames reach the program in batches about 2 s late: a frame is visible only 2 s after it ended.
        var t = new AbTuner();
        var start = new DateTime(2026, 1, 1);
        var now = start;
        var delivered = new List<DateTime>();
        t.Start(V.Take(2).ToList(), 0, now);
        TunerPhase first = null;
        for (int ms = 0; ms <= (AbTuner.PhaseSeconds + AbTuner.LagSeconds) * 1000 + 1000 && first == null; ms += 2)
        {
            var frameEnd = start.AddMilliseconds(ms);           // 500 FPS, one frame every 2 ms
            delivered.Add(frameEnd);
            if (ms % 1000 != 0) continue;
            now = frameEnd;
            var visible = delivered.Where(f => f <= now.AddSeconds(-2)).ToList();
            t.Tick(now, (a, b) =>
            {
                var l = new List<double>();
                foreach (var f in visible) if (f > a && f <= b) l.Add(2.0);
                return l;
            }, out _);
            if (t.Phases.Count > 0) first = t.Phases[0];
        }
        Assert.NotNull(first);
        Assert.True(first.Valid);
        Assert.InRange(first.Frames, (AbTuner.PhaseSeconds - AbTuner.SettleSeconds) * 500 - 600, (AbTuner.PhaseSeconds - AbTuner.SettleSeconds) * 500 + 10);
    }

    [Fact]
    public void MeasureGivesLowsAndHitches()
    {
        var ph = AbTuner.Measure(Frames(2.0, 8.0, 30));     // 500 FPS, 30 spikes of 8 ms in 2000 frames
        Assert.True(ph.Valid);
        Assert.Equal(125, ph.Low1, 0);                       // 1.5% of frames are 8 ms → p99 = 8 ms
        Assert.Equal(30, ph.Hitches);
        Assert.False(AbTuner.Measure(Frames(2, 2, 0, 100)).Valid);
    }

    [Fact]
    public void WinnerMustBeBetterInEveryRound()
    {
        TunerPhase P(int v, int r, double low1, int hitches = 0) => new() { Variant = v, Round = r, Low1 = low1, Hitches = hitches, Valid = true };
        var both = new List<TunerPhase> { P(0, 0, 300), P(1, 0, 330), P(0, 1, 300), P(1, 1, 320) };
        Assert.Equal(1, AbTuner.Decide(V.Take(2).ToList(), both, 0).Winner);
        var once = new List<TunerPhase> { P(0, 0, 300), P(1, 0, 360), P(0, 1, 300), P(1, 1, 302) };
        Assert.Equal(-1, AbTuner.Decide(V.Take(2).ToList(), once, 0).Winner);           // better only once: no change
        var hitchy = new List<TunerPhase> { P(0, 0, 300, 2), P(1, 0, 330, 9), P(0, 1, 300, 2), P(1, 1, 330, 9) };
        Assert.Equal(-1, AbTuner.Decide(V.Take(2).ToList(), hitchy, 0).Winner);         // higher lows but more hitches
        var invalid = new List<TunerPhase> { P(0, 0, 300), new() { Variant = 1, Round = 0 } };
        Assert.Equal(-1, AbTuner.Decide(V.Take(2).ToList(), invalid, 0).Winner);
    }
}
