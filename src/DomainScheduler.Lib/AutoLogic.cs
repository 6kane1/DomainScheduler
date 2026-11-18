using System.Globalization;

namespace DomainScheduler;

/// <summary>Every Auto parameter (section 12.3): constants, never read from the UI or the registry.</summary>
internal static class AutoPolicy
{
    public const int EnterStableMs = 5000, ExitStableMs = 15000, ReapplyMs = 5000;
    public const int MinGameLp = 4, OsReservedCores = 1;
    public const double MinMaskFraction = 0.5, HotRateFraction = 0.05, CoverTarget = 0.80;
    public const double ConfidenceFloor = 0.60;
    public const string GameClass = "above", BgClass = "below";
    public const int ArmingTimeoutMs = 8000, BaselineMs = 5000, WarmupMs = 700, WarmupRounds = 3;
    public const string IrqMode = GameMask.CoreMode;
    public const double GameCpuShare = 0.15;          // of one core
    public const double PlanChangeFraction = 0.25;    // thread composition change that rebuilds the plan
    public const double DeniedShareLimit = 0.30;
    public const int ThreadDropFactor = 3, MaxApplyErrors = 3, MinFramesForBaseline = 200;
}

public enum ThreadRole { Main, Render, Audio, Worker, Aux, Skip }

public sealed class Classified
{
    public ThreadRow Row;
    public ThreadRole Role;
    public uint LP;
    public byte Core, LLC;
}

/// <summary>The pure decisions of the Auto mode (sections 12.5–12.7, 17.2).</summary>
public static class AutoLogic
{
    // ---- mask (12.5) ------------------------------------------------------------------------------

    public static string PickMask(Topo t) => t.Split.CanSplit && t.Split.Family == "x3d" ? "vcache" : t.Hybrid ? "pcores" : "auto";

    public static int Domains(Topo t, ulong m) => t.Sets.Where(s => s.LP < 64 && ((m >> s.LP) & 1) != 0).Select(s => s.LLC).Distinct().Count();

    /// <summary>A single L3 domain can hold the game (at least MinGameLp processors).</summary>
    public static bool HasSingleDomain(Topo t) => t.Sets.GroupBy(s => s.LLC).Any(g => g.Count() >= AutoPolicy.MinGameLp);

    /// <summary>The best single domain: the largest L3, then the most processors, then the lowest index.</summary>
    public static ulong BestSingleDomain(Topo t)
    {
        var best = t.Sets.GroupBy(s => s.LLC).Where(g => g.Count() >= AutoPolicy.MinGameLp)
            .OrderByDescending(g => t.L3ByLlc.GetValueOrDefault(g.Key)).ThenByDescending(g => g.Count()).ThenBy(g => g.Key).First();
        return TopoBuilder.Mask(best);
    }

    public static ulong Harden(Topo t, ulong p)
    {
        ulong full = t.FullMask();
        if (p == 0 || TopoBuilder.PopCount(p) < AutoPolicy.MinGameLp) return full;
        if (TopoBuilder.PopCount(p) < TopoBuilder.PopCount(full) * AutoPolicy.MinMaskFraction) return full;
        if (Domains(t, p) > 1 && HasSingleDomain(t)) return BestSingleDomain(t);
        return p;
    }

    public sealed class MaskPlan
    {
        public string Spec = "";
        public MaskResult Resolved;
        public GameMaskInfo Irq;
        public ulong Mask;          // 0: no mask is applied
        public bool Satisfied;
        public List<string> Notes = new();
    }

    /// <summary>PickMask → Resolve → ForRule(IRQ, Core) → Unmix(false) → Harden; unsatisfied → no mask.</summary>
    public static MaskPlan BuildMask(Topo t, ulong reserved)
    {
        var mp = new MaskPlan { Spec = PickMask(t) };
        mp.Resolved = Masks.Resolve(t, mp.Spec);
        mp.Satisfied = mp.Resolved.Satisfied;
        if (!mp.Satisfied)
        {
            mp.Notes.AddRange(mp.Resolved.Notes.Select(Masks.NoteText));
            return mp;
        }
        ulong? busy = GameMask.BusyHook?.Invoke(t);
        mp.Irq = busy.HasValue ? GameMask.ForLive(t, mp.Resolved.Mask, busy.Value) : GameMask.ForRule(t, mp.Resolved.Mask, AutoPolicy.IrqMode);
        ulong m = Reservation.Unmix(mp.Irq.Final, reserved, false);
        mp.Mask = Harden(t, m);
        if (mp.Irq.IrqIgnored) mp.Notes.Add(L.T("IRQ exclusion ignored: it would cut more than half of the mask"));
        return mp;
    }

    // ---- threads (12.6) ---------------------------------------------------------------------------

    static bool Hint(string d, string m, params string[] words) => words.Any(w => d.Contains(w) || m.Contains(w));

    public static ulong TotalRate(ThreadSnap s) => s.Rows.Aggregate(0UL, (a, r) => a + r.Rate);

    public static ulong HotThreshold(ThreadSnap s) => (ulong)(TotalRate(s) * AutoPolicy.HotRateFraction);

    public static ThreadRole Classify(ThreadRow r, ThreadSnap s)
    {
        if (Guard.IsAntiCheat(Engine.ModuleOf(r.Sym))) return ThreadRole.Skip;
        string d = r.Desc.ToLowerInvariant(), m = Engine.ModuleOf(r.Sym).ToLowerInvariant();
        if (Hint(d, m, "audio", "wasapi", "xaudio", "fmod", "wwise", "dsound")) return ThreadRole.Audio;
        if (Hint(d, m, "render", "gfx", "d3d", "vulkan", "opengl", "nvapi", "atidxx")) return ThreadRole.Render;
        if (s.Rows.Count > 0 && r.Tid == s.Rows.Min(x => x.Tid)) return ThreadRole.Main;
        if (r.Rate > HotThreshold(s)) return ThreadRole.Worker;
        return ThreadRole.Aux;
    }

    public static double Confidence(ThreadSnap s, IReadOnlyList<Classified> plan)
    {
        double n = Math.Max(1, s.Rows.Count);
        double s1 = s.Rows.Count(r => r.Desc.Length > 0) / n;
        double s2 = s.Rows.Count(r => Engine.ModuleOf(r.Sym).Length > 0) / n;
        double s3 = s.CycleFallbackTids.Count == 0 ? 1.0 : 0.5;
        double s4 = plan.Any(x => x.Role == ThreadRole.Main) ? 1.0 : 0.0;
        double s5 = s.Rows.Count(r => r.Rate > 0) >= 2 ? 1.0 : 0.0;
        return 0.25 * s1 + 0.25 * s2 + 0.15 * s3 + 0.20 * s4 + 0.15 * s5;
    }

    static int Order(ThreadRole r) => r switch { ThreadRole.Main => 0, ThreadRole.Render => 1, ThreadRole.Audio => 2, _ => 3 };

    /// <summary>
    /// The pin plan (12.7): roles Main/Render/Audio/Worker, at most cores−1 threads, one thread per
    /// physical core; Main and Render on different cores of one L3 domain; workers only until
    /// their rate covers CoverTarget of the process; earlier LPs are kept (sticky).
    /// </summary>
    public static List<Classified> Plan(Topo t, ThreadSnap s, ulong mask, IReadOnlyDictionary<uint, uint> previous = null)
    {
        var cores = Masks.IdealCandidates(t, true, mask);
        int budget = Math.Max(1, cores.Count - AutoPolicy.OsReservedCores);
        var classified = s.Rows.Select(r => new Classified { Row = r, Role = Classify(r, s) }).ToList();
        var pick = classified.Where(x => x.Role is ThreadRole.Main or ThreadRole.Render or ThreadRole.Audio or ThreadRole.Worker)
            .OrderBy(x => Order(x.Role)).ThenByDescending(x => x.Row.Rate).ToList();
        ulong total = TotalRate(s), covered = 0;
        var plan = new List<Classified>();
        foreach (var c in pick)
        {
            if (plan.Count >= budget) break;
            if (c.Role == ThreadRole.Worker && total > 0 && covered >= total * AutoPolicy.CoverTarget) continue;   // cold ones are not added
            plan.Add(c);
            covered += c.Row.Rate;
        }
        // Processors: one thread per physical core; Main and Render on different cores of one L3
        // domain; a thread keeps the processor of the previous plan when it can (sticky).
        var usedCores = new HashSet<byte>();
        var done = new HashSet<Classified>();
        if (previous != null)
            foreach (var c in plan)
            {
                if (!previous.TryGetValue(c.Row.Tid, out uint lp)) continue;
                var cs = cores.FirstOrDefault(x => x.LP == lp);
                if (cores.Any(x => x.LP == lp) && usedCores.Add(cs.Core))
                {
                    Assign(c, cs);
                    done.Add(c);
                }
            }
        byte? mainLlc = plan.Where(c => c.Role == ThreadRole.Main && done.Contains(c)).Select(c => (byte?)c.LLC).FirstOrDefault();
        foreach (var c in plan)
        {
            if (done.Contains(c)) continue;
            var free = cores.Where(x => !usedCores.Contains(x.Core)).ToList();
            if (c.Role == ThreadRole.Render && mainLlc.HasValue)
            {
                var same = free.Where(x => x.LLC == mainLlc.Value).ToList();
                if (same.Count > 0) free = same;
            }
            if (free.Count == 0) break;
            var pickCore = free[0];
            usedCores.Add(pickCore.Core);
            Assign(c, pickCore);
            done.Add(c);
            if (c.Role == ThreadRole.Main) mainLlc = pickCore.LLC;
        }
        return plan.Where(done.Contains).ToList();
    }

    static void Assign(Classified c, CpuSet s)
    {
        c.LP = s.LP;
        c.Core = s.Core;
        c.LLC = s.LLC;
    }

    public static string RolePrio(ThreadRole r) => r switch
    {
        ThreadRole.Main or ThreadRole.Render or ThreadRole.Audio => "above",
        _ => "",
    };

    /// <summary>The synthesized preset (17.2): numeric ideal processors, Match = tid.</summary>
    /// <param name="hotFirst">22.6: policy of the first slot of the plan ("pin" | "ideal" | "none").</param>
    /// <param name="hotOthers">22.6: policy of the other slots. "none" keeps the plan as it is.</param>
    public static Preset BuildAutoPreset(string exe, IReadOnlyList<Classified> plan, bool pins, string hotFirst = "none", string hotOthers = "none")
    {
        var p = new Preset { Exe = exe, Class = AutoPolicy.GameClass, Io = "normal", Mem = "normal", Eco = "off", Boost = "" };
        for (int i = 0; i < plan.Count; i++)
        {
            var c = plan[i];
            // The roles give the order; a set heat field overrides what the slot gets.
            string heat = i == 0 ? hotFirst : hotOthers;
            bool hard = heat == "pin";
            p.Threads.Add(new ThreadRule
            {
                Match = "tid",
                Pattern = c.Row.Tid.ToString(CultureInfo.InvariantCulture),
                Prio = RolePrio(c.Role),
                Ideal = pins ? c.LP.ToString(CultureInfo.InvariantCulture) : "",
                // Hard limits only with pins, i.e. only when the confidence passed the floor (the guard of 22.4).
                Pin = pins && hard && c.Role is ThreadRole.Main or ThreadRole.Render or ThreadRole.Audio or ThreadRole.Worker,
                Boost = c.Role is ThreadRole.Main or ThreadRole.Audio ? "on" : "off",
                Enabled = true,
            });
        }
        return p;
    }

    /// <summary>Share of the planned thread set that changed (ended or new heavy threads).</summary>
    public static double CompositionChange(IReadOnlyCollection<uint> planned, IReadOnlyCollection<uint> now)
    {
        if (planned.Count == 0) return now.Count > 0 ? 1 : 0;
        var a = planned.ToHashSet();
        var b = now.ToHashSet();
        int gone = a.Count(x => !b.Contains(x));
        int added = b.Count(x => !a.Contains(x));
        return (double)(gone + added) / Math.Max(a.Count, 1);
    }
}
