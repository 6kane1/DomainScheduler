using System.Globalization;
using System.Text;

namespace DomainScheduler;

/// <summary>One combination the tuner tries.</summary>
public sealed record TunerVariant(string Name, bool Top5, bool LiveMask);

/// <summary>Frame statistics of one measured phase.</summary>
public sealed class TunerPhase
{
    public int Variant, Round, Frames, Hitches;
    public double AvgFps, Low1, Low01;
    public bool Valid;
}

/// <summary>
/// The automatic A/B tuner: while the user plays, it switches between a few setting combinations, measures the
/// frame time of each, and keeps a combination only when it was better than the current one in every round.
/// Round 1 goes A, B, C…, round 2 in the reverse order, so a scene that slowly gets heavier does not favour one
/// variant. The first seconds after a switch are not measured (threads move, caches refill).
/// Pure logic: the caller applies the variants and supplies frame intervals.
/// </summary>
public sealed class AbTuner
{
    public const int PhaseSeconds = 60, SettleSeconds = 8, Rounds = 2, MinFrames = 600;
    /// <summary>
    /// Frame events reach the program in batches, up to a few seconds late: frames are read this far behind the
    /// clock, and a phase is closed only after its last frames had time to arrive.
    /// </summary>
    public const int LagSeconds = 3;
    /// <summary>A variant must beat the current one by this much (1% low) in every round to be chosen.</summary>
    public const double MinGain = 0.03;

    public enum Step { None, Switch, Finished }

    readonly List<(int Variant, int Round)> _schedule = new();
    readonly List<double> _intervals = new();
    DateTime _stepStart, _lastFetch;
    int _index;

    public IReadOnlyList<TunerVariant> Variants { get; private set; } = Array.Empty<TunerVariant>();
    public int Baseline { get; private set; }
    public List<TunerPhase> Phases { get; } = new();
    public bool Running { get; private set; }
    public int Winner { get; private set; } = -1;
    public string Verdict { get; private set; } = "";
    public TunerVariant Current => Running && _index < _schedule.Count ? Variants[_schedule[_index].Variant] : null;
    public int StepIndex => _index;
    public int StepCount => _schedule.Count;
    public TimeSpan Total => TimeSpan.FromSeconds((PhaseSeconds + LagSeconds) * _schedule.Count);

    /// <summary>Starts; the first variant to apply is returned (the baseline is measured too).</summary>
    public TunerVariant Start(IReadOnlyList<TunerVariant> variants, int baseline, DateTime now)
    {
        Variants = variants;
        Baseline = baseline;
        _schedule.Clear();
        Phases.Clear();
        for (int r = 0; r < Rounds; r++)
        {
            var order = Enumerable.Range(0, variants.Count).ToList();
            if (r % 2 == 1) order.Reverse();
            foreach (int v in order) _schedule.Add((v, r));
        }
        _index = 0;
        Winner = -1;
        Verdict = "";
        Running = true;
        BeginStep(now);
        return Current;
    }

    void BeginStep(DateTime now)
    {
        _stepStart = now;
        _lastFetch = now.AddSeconds(SettleSeconds);
        _intervals.Clear();
    }

    /// <summary>
    /// Once a second. frames(from, to) returns the frame intervals (ms) of frames that ended in that span.
    /// Returns Switch with the next variant to apply, Finished when done, otherwise None.
    /// </summary>
    public Step Tick(DateTime now, Func<DateTime, DateTime, List<double>> frames, out TunerVariant next)
    {
        next = null;
        if (!Running) return Step.None;
        // Measured span of this step: from the end of the settle time to the end of the phase, read LagSeconds late.
        DateTime phaseEnd = _stepStart.AddSeconds(PhaseSeconds);
        DateTime upTo = now.AddSeconds(-LagSeconds);
        if (upTo > phaseEnd) upTo = phaseEnd;
        if (upTo > _lastFetch)
        {
            _intervals.AddRange(frames(_lastFetch, upTo));
            _lastFetch = upTo;
        }
        if ((now - _stepStart).TotalSeconds < PhaseSeconds + LagSeconds) return Step.None;
        var (v, r) = _schedule[_index];
        var ph = Measure(_intervals);
        ph.Variant = v;
        ph.Round = r;
        Phases.Add(ph);
        _index++;
        if (_index >= _schedule.Count)
        {
            Running = false;
            (Winner, Verdict) = Decide(Variants, Phases, Baseline);
            return Step.Finished;
        }
        BeginStep(now);
        next = Current;
        return Step.Switch;
    }

    public void Cancel()
    {
        Running = false;
        Verdict = L.T("Stopped before the end: nothing was changed");
    }

    /// <summary>Average FPS, 1% and 0.1% low (FPS of the 99th / 99.9th percentile frame time), hitches (> 2.5× median).</summary>
    public static TunerPhase Measure(List<double> intervals)
    {
        var ph = new TunerPhase { Frames = intervals.Count };
        if (intervals.Count < MinFrames) return ph;
        var sorted = intervals.Where(x => x > 0 && x < 1000).OrderBy(x => x).ToList();
        if (sorted.Count < MinFrames) return ph;
        double P(double p) => sorted[Math.Clamp((int)Math.Ceiling(p * sorted.Count) - 1, 0, sorted.Count - 1)];
        double median = P(0.5);
        ph.AvgFps = 1000.0 / sorted.Average();
        ph.Low1 = 1000.0 / P(0.99);
        ph.Low01 = 1000.0 / P(0.999);
        ph.Hitches = sorted.Count(x => x > median * 2.5);
        ph.Valid = true;
        return ph;
    }

    /// <summary>
    /// The winner beats the baseline's 1% low by MinGain in every round, with no more hitches on average; among
    /// several, the best average 1% low. Otherwise the baseline stays (-1 = keep current).
    /// </summary>
    public static (int Winner, string Verdict) Decide(IReadOnlyList<TunerVariant> variants, List<TunerPhase> phases, int baseline)
    {
        int rounds = phases.Count == 0 ? 0 : phases.Max(p => p.Round) + 1;
        if (phases.Any(p => !p.Valid))
            return (-1, L.T("Too few frames were measured in some phases (was the game in front all the time?): nothing was changed"));
        TunerPhase Get(int v, int r) => phases.FirstOrDefault(p => p.Variant == v && p.Round == r);
        int best = -1;
        double bestLow = 0;
        for (int v = 0; v < variants.Count; v++)
        {
            if (v == baseline) continue;
            bool allBetter = true;
            for (int r = 0; r < rounds; r++)
            {
                var a = Get(v, r);
                var b = Get(baseline, r);
                if (a == null || b == null || a.Low1 < b.Low1 * (1 + MinGain)) { allBetter = false; break; }
            }
            double hitV = phases.Where(p => p.Variant == v).Average(p => p.Hitches);
            double hitB = phases.Where(p => p.Variant == baseline).Average(p => p.Hitches);
            double low = phases.Where(p => p.Variant == v).Average(p => p.Low1);
            if (allBetter && hitV <= hitB && low > bestLow) { best = v; bestLow = low; }
        }
        return best < 0
            ? (-1, L.T("No combination was better than the current one in every round: the current settings stay"))
            : (best, L.F("\"{0}\" was better in every round: press Apply on the home screen to keep it", variants[best].Name));
    }

    /// <summary>Average 1% low over the valid rounds and total hitches of one variant.</summary>
    public (double Low1, int Hitches) Stats(int variant)
    {
        var ph = Phases.Where(p => p.Variant == variant).ToList();
        var valid = ph.Where(p => p.Valid).ToList();
        return (valid.Count > 0 ? valid.Average(p => p.Low1) : 0, ph.Sum(p => p.Hitches));
    }

    /// <summary>The result table for the log and the report.</summary>
    public string Table()
    {
        var sb = new StringBuilder();
        foreach (var g in Phases.GroupBy(p => p.Variant).OrderBy(g => g.Key))
        {
            string name = Variants[g.Key].Name + (g.Key == Baseline ? " (" + L.T("current") + ")" : "");
            sb.AppendLine(string.Create(CultureInfo.InvariantCulture,
                $"{name}: 1% low {string.Join(" / ", g.OrderBy(p => p.Round).Select(p => p.Valid ? p.Low1.ToString("0", CultureInfo.InvariantCulture) : "—"))} FPS, " +
                $"0.1% low {string.Join(" / ", g.OrderBy(p => p.Round).Select(p => p.Valid ? p.Low01.ToString("0", CultureInfo.InvariantCulture) : "—"))}, " +
                $"avg {g.Where(p => p.Valid).Select(p => p.AvgFps).DefaultIfEmpty(0).Average():0} FPS, hitches {g.Sum(p => p.Hitches)}"));
        }
        return sb.ToString().TrimEnd();
    }
}
