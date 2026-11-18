using System.Globalization;
using System.Text;

namespace DomainScheduler;

/// <summary>Frame-time numbers of the last minute for the home screen. Zero values mean "not enough frames".</summary>
public readonly record struct FrameStats(int Frames, double SpanSec, double AvgFps, double Low1Fps, double Low01Fps,
    double FrameMs, double MedianMs, double P99Ms, int Hitches, double HitchesPerMin)
{
    public bool Valid => Frames >= HudMath.MinFrames;
}

/// <summary>What a logical processor is on the home screen's processor map.</summary>
public enum CoreKind { Background, Game, Interrupt }

/// <summary>
/// The pure logic behind the HUD home screen (1.14.0): frame statistics, hitch detection, the reduction of a minute of
/// frames to a fixed number of chart points, the chart scale, the processor-map colours and the session timer text.
/// </summary>
public static class HudMath
{
    /// <summary>A frame longer than this many medians is a hitch (a visible stutter).</summary>
    public const double HitchFactor = 2.5;

    /// <summary>Fewer frames than this give no 1% low (the old home tile used the same limit).</summary>
    public const int MinFrames = 200;

    /// <summary>The 0.1% low needs this many frames to mean anything.</summary>
    public const int MinFrames01 = 1000;

    public static bool IsHitch(double ms, double medianMs) => medianMs > 0 && ms > HitchFactor * medianMs;

    /// <summary>Nearest-rank percentile of an ascending list (the same rule as the 1.13 home tile).</summary>
    public static double Percentile(IReadOnlyList<double> sorted, double q)
    {
        int n = sorted.Count;
        if (n == 0) return 0;
        return sorted[Math.Min(n - 1, Math.Max(0, (int)Math.Ceiling(n * q) - 1))];
    }

    public static FrameStats Stats(IReadOnlyList<double> intervals)
    {
        int n = intervals?.Count ?? 0;
        if (n < MinFrames) return new FrameStats { Frames = n };
        var sorted = new List<double>(n);
        double sum = 0;
        foreach (double x in intervals)
        {
            sorted.Add(x);
            sum += x;
        }
        if (sum <= 0) return new FrameStats { Frames = n };
        sorted.Sort();
        double median = Percentile(sorted, 0.5);
        double p99 = Percentile(sorted, 0.99);
        double p999 = n >= MinFrames01 ? Percentile(sorted, 0.999) : 0;
        int hitches = 0;
        foreach (double x in intervals)
            if (IsHitch(x, median)) hitches++;
        double span = sum / 1000.0;
        return new FrameStats(n, span, n / span, p99 > 0 ? 1000.0 / p99 : 0, p999 > 0 ? 1000.0 / p999 : 0,
            sum / n, median, p99, hitches, span >= 1.0 ? hitches * 60.0 / span : 0);
    }

    /// <summary>
    /// Reduces frame times (ascending UTC present times) to `max.Length` buckets that end at `end`.
    /// max[k]: the longest frame that covered bucket k (a long frame covers every bucket it spans, so a freeze is a
    /// plateau, not a gap); 0 = no frame there. endMax[k]: the longest frame that ENDED in bucket k (one hitch marker
    /// per frame). Frames after `end` are left for the next update.
    /// </summary>
    public static void Buckets(IReadOnlyList<DateTime> times, DateTime end, TimeSpan bucket, double[] max, double[] endMax)
    {
        int count = max.Length;
        Array.Clear(max);
        if (endMax != null) Array.Clear(endMax);
        if (times == null || times.Count < 2 || count == 0 || bucket <= TimeSpan.Zero) return;
        DateTime start = end - TimeSpan.FromTicks(bucket.Ticks * count);
        for (int i = 1; i < times.Count; i++)
        {
            DateTime a = times[i - 1], b = times[i];
            if (b <= start || b > end || b <= a) continue;
            double ms = (b - a).TotalMilliseconds;
            int bi = Index(b), ai = Index(a);
            for (int k = ai; k <= bi; k++)
                if (ms > max[k]) max[k] = ms;
            if (endMax != null && ms > endMax[bi]) endMax[bi] = ms;
        }

        int Index(DateTime t)
        {
            if (t <= start) return 0;
            // A frame that ends exactly on a bucket edge belongs to the bucket before it.
            long ticks = (t - start).Ticks;
            int k = (int)((ticks - 1) / bucket.Ticks);
            return Math.Clamp(k, 0, count - 1);
        }
    }

    /// <summary>The chart end: the clock minus the ETW delivery lag, cut down to a whole bucket so points do not jitter.</summary>
    public static DateTime AlignEnd(DateTime now, TimeSpan lag, TimeSpan bucket)
    {
        long t = (now - lag).Ticks;
        return new DateTime(t - t % bucket.Ticks, now.Kind);
    }

    /// <summary>Rounds up to 1, 2, 2.5 or 5 times a power of ten.</summary>
    public static double Nice(double v)
    {
        if (!(v > 0) || double.IsInfinity(v)) return 1;
        double p = Math.Pow(10, Math.Floor(Math.Log10(v)));
        double f = v / p;
        double nf = f <= 1.0000001 ? 1 : f <= 2.0000001 ? 2 : f <= 2.5000001 ? 2.5 : f <= 5.0000001 ? 5 : 10;
        return nf * p;
    }

    /// <summary>The top of the frame-time chart in ms: room for the longest point and the 1% low line, but a long
    /// pause (alt-tab, loading) does not squash the normal frames into the floor: it is capped at 8 medians.</summary>
    public static double ChartTop(double maxMs, double low1Ms, double medianMs)
    {
        double want = maxMs * 1.1;
        if (medianMs > 0) want = Math.Min(want, medianMs * 8);
        want = Math.Max(want, low1Ms * 1.3);
        want = Math.Max(want, medianMs * 1.5);
        return Nice(Math.Max(want, 1));
    }

    public static CoreKind Classify(int lp, ulong gameMask, ulong irqMask)
    {
        if (lp < 0 || lp > 63) return CoreKind.Background;
        ulong bit = 1UL << lp;
        // A processor the game may use is the game's, whatever else runs there; amber marks only the interrupt
        // processors the game was kept off.
        if ((gameMask & bit) != 0) return CoreKind.Game;
        if ((irqMask & bit) != 0) return CoreKind.Interrupt;
        return CoreKind.Background;
    }

    /// <summary>The session timer of the status pill: hh:mm:ss, hours keep counting past 24.</summary>
    public static string Elapsed(TimeSpan t)
    {
        if (t < TimeSpan.Zero) t = TimeSpan.Zero;
        long h = (long)Math.Floor(t.TotalHours);
        return string.Create(CultureInfo.InvariantCulture, $"{h:00}:{t.Minutes:00}:{t.Seconds:00}");
    }

    /// <summary>Upper case with a narrow no-break space after every character: the letter spacing of the HUD captions
    /// (WPF has no letter spacing; U+202F does not allow a line break inside a word).</summary>
    public static string Spaced(string s, CultureInfo culture = null)
    {
        if (string.IsNullOrEmpty(s)) return "";
        string up = s.ToUpper(culture ?? CultureInfo.CurrentCulture);
        var sb = new StringBuilder(up.Length * 2);
        for (int i = 0; i < up.Length; i++)
        {
            if (i > 0) sb.Append(' ');
            sb.Append(up[i]);
        }
        return sb.ToString();
    }
}
