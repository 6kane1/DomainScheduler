using System.Globalization;
using System.Text;

namespace DomainScheduler;

/// <summary>One check run: frames and the game's ready-thread waits (1.16.0 Test 1.1: single runs, no CPU/GPU hint).</summary>
public sealed record CheckResult(string Group, int Run, double Seconds, int Frames, double Fps, double Low1, double Low01,
    double P99Ms, double MaxMs, int Hitches, int Waits, double WaitP99Us, double WaitP999Us, double WaitMaxUs, int Over500, uint Lost);

/// <summary>
/// The pure arithmetic of the Check page (1.16.0 Test 1): the same definitions as Domain SchedTest (lows are frame-time
/// percentiles, a hitch is a frame longer than 2.5× the median).
/// </summary>
public static class CheckStats
{
    public const double HitchFactor = 2.5;

    static double P(double[] sorted, double q) => sorted.Length == 0 ? 0 : sorted[Math.Clamp((int)Math.Ceiling(q * sorted.Length) - 1, 0, sorted.Length - 1)];

    public static CheckResult From(string group, int run, long[] waits, long[] presents, long first, long last, double ticksPerSecond, uint lost)
    {
        double seconds = Math.Max(1e-9, (last - first) / ticksPerSecond);
        double usPerTick = 1e6 / ticksPerSecond, msPerTick = 1e3 / ticksPerSecond;
        var w = waits.Select(t => t * usPerTick).OrderBy(x => x).ToArray();
        int frames = Math.Max(0, presents.Length - 1);
        var ft = new double[frames];
        for (int i = 0; i < frames; i++) ft[i] = (presents[i + 1] - presents[i]) * msPerTick;
        var fs = ft.OrderBy(x => x).ToArray();
        double fsec = frames > 0 ? (presents[^1] - presents[0]) * msPerTick / 1000 : 0;
        double med = P(fs, .5);
        return new CheckResult(group, run, seconds, frames,
            frames > 0 && fsec > 0 ? frames / fsec : 0,
            fs.Length > 0 ? 1000 / P(fs, .99) : 0, fs.Length > 0 ? 1000 / P(fs, .999) : 0,
            P(fs, .99), fs.Length > 0 ? fs[^1] : 0, ft.Count(x => x > med * HitchFactor),
            w.Length, P(w, .99), P(w, .999), w.Length > 0 ? w[^1] : 0, w.Count(x => x > 500), lost);
    }

    /// <summary>A plain-text report (the file saved on the desktop) in the language of the program.</summary>
    public static string Report(IReadOnlyList<CheckResult> all, string game, string header)
    {
        var ci = CultureInfo.InvariantCulture;
        var sb = new StringBuilder();
        sb.AppendLine(header);
        sb.AppendLine(L.F("Game: {0}", game));
        sb.AppendLine();
        sb.AppendLine(L.T("run  group             FPS   1% low  0.1% low  p99 ms  max ms  hitches  waits p99 µs  p99.9 µs  max µs  >500 µs"));
        foreach (var r in all)
            sb.AppendLine(string.Format(ci, "{0,3}  {1,-16} {2,5:0} {3,8:0} {4,9:0} {5,7:0.00} {6,7:0.00} {7,8} {8,13:0.0} {9,9:0.0} {10,7:0} {11,8}",
                r.Run, r.Group, r.Fps, r.Low1, r.Low01, r.P99Ms, r.MaxMs, r.Hitches, r.WaitP99Us, r.WaitP999Us, r.WaitMaxUs, r.Over500) + (r.Lost > 0 ? "  " + L.F("lost events: {0}", r.Lost) : ""));
        return sb.ToString();
    }
}
