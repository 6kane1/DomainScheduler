using System.Globalization;
using System.Text.RegularExpressions;

namespace DomainScheduler;

/// <summary>Game / non-game split of the processor (section 4).</summary>
public static class CoreSplit
{
    /// <summary>Dual-CCD X3D models: data, not code (Tables).</summary>
    static Dictionary<string, int> X3dDual => Tables.X3d;

    static readonly Regex SkuRx = new(@"\b(\d{4}X3D)\b", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    static readonly Regex UltraRx = new(@"Core\(TM\)\s+Ultra|Core Ultra", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);

    public static ulong Bits(int n) => n < 64 ? (1UL << n) - 1 : ulong.MaxValue;

    public static string SkuOf(string name)
    {
        var m = SkuRx.Match(name ?? "");
        return m.Success ? m.Groups[1].Value.ToUpperInvariant() : "";
    }

    public static bool IsCoreUltra(string name) => UltraRx.IsMatch(name ?? "");

    static string Mb(ulong b) => (b >> 20).ToString(CultureInfo.InvariantCulture) + " MB";

    static SplitPlan Stop(SplitPlan p, string code, params object[] args)
    {
        p.Family = "none";
        p.Code = code;
        p.Args = args;
        return p;
    }

    public static SplitPlan Detect(Topo t)
    {
        var p = new SplitPlan { Name = t.Name ?? "", Sku = SkuOf(t.Name) };
        if (t.Groups > 1) return Stop(p, "groups", t.Groups);
        bool known = t.LLCGroups.All(l => t.L3ByLlc.TryGetValue(l, out var v) && v != 0);
        if (t.LLCGroups.Count > 2 && known) return Stop(p, "domains", t.LLCGroups.Count);
        if (!known) return Stop(p, "nocache");
        p.Probe = t.LogicalCount;
        p.Reported = t.Reported > 0 ? t.Reported : t.LogicalCount;
        if (p.Probe != p.Reported) return Stop(p, "probe", p.Probe, p.Reported);
        if (p.Probe > 64 || p.Reported > 64) return Stop(p, "64+");

        var list2 = t.Sets.Where(s => s.Group == 0).ToList();
        var cores = list2.GroupBy(s => s.Core).ToList();
        p.All = TopoBuilder.Mask(list2);
        p.SmtOn = cores.Any(g => g.Count() > 1);
        p.Cores = t.PhysicalCount;
        bool mixedSmt = p.SmtOn && cores.Any(g => g.Count() == 1);

        // A. Hybrid: split by EfficiencyClass, not by cache.
        if (t.EffClasses.Count > 1)
        {
            byte top = t.EffClasses.Max();
            p.Family = "hybrid";
            p.TopEff = top;
            p.Game = TopoBuilder.Mask(list2.Where(s => s.Eff == top));
            p.NonGame = p.All & ~p.Game;
            p.PCores = cores.Count(c => c.First().Eff == top);
            p.ECores = cores.Count - p.PCores;
            p.Expected = p.Reported;
            p.Code = t.LLCGroups.Count > 1 ? "split" : "single";
            return Validate(t, p);
        }

        // B. Symmetric, exactly two L3 domains.
        if (t.LLCGroups.Count == 2)
        {
            if (mixedSmt) return Stop(p, "mixed-smt");
            ulong s0 = t.L3ByLlc[t.LLCGroups[0]], s1 = t.L3ByLlc[t.LLCGroups[1]];
            if (t.LLCGroups.Select(l => t.L3ByLlc[l]).Distinct().Count() <= 1) return Stop(p, "equal", Mb(s0), Mb(s1), t.Name);
            byte big = s0 > s1 ? t.LLCGroups[0] : t.LLCGroups[1];
            p.GameL3 = Math.Max(s0, s1);
            p.NonGameL3 = Math.Min(s0, s1);
            p.Game = TopoBuilder.Mask(list2.Where(s => s.LLC == big)) & p.All;
            p.NonGame = p.All & ~p.Game;
            int f = p.SmtOn ? 2 : 1;
            if (p.Sku.Length > 0 && X3dDual.TryGetValue(p.Sku, out int c))
            {
                p.Expected = c * f;
                if (p.Reported != p.Expected) return Stop(p, "sku", p.Sku, p.Expected, p.SmtOn, p.Reported);
                ulong lower = Bits(c / 2 * f);       // 7900X3D → 0xFFF, 7950X3D → 0xFFFF
                p.Family = "x3d";
                p.UpperHalf = p.NonGame == lower;
            }
            p.Code = t.LLCGroups.Count > 1 ? "split" : "single";
            return Validate(t, p);
        }

        // One domain: nothing to split.
        p.Family = "none";
        p.Code = "single";
        p.Game = p.NonGame = 0;
        return p;
    }

    public static SplitPlan Validate(Topo t, SplitPlan p)
    {
        if (p.Game == 0) p.Problems.Add("empty");
        if (p.NonGame == 0) p.Problems.Add("nongame-empty");
        if ((p.Game | p.NonGame) != p.All) p.Problems.Add("coverage");
        if ((p.Game & p.NonGame) != 0) p.Problems.Add("overlap");
        if (!SmtWhole(t, p.Game)) p.Problems.Add("smt-game");
        if (!SmtWhole(t, p.NonGame)) p.Problems.Add("smt-nongame");
        return p;
    }

    public static bool SmtWhole(Topo t, ulong mask)
    {
        foreach (var g in t.Sets.Where(s => s.Group == 0).GroupBy(s => s.Core))
        {
            ulong core = TopoBuilder.Mask(g), inter = core & mask;
            if (inter != 0 && inter != core) return false;
        }
        return true;
    }

    /// <summary>Human-readable text of a split code.</summary>
    public static string Text(string code, object[] args)
    {
        object A(int i) => args != null && i < args.Length ? args[i] : "?";
        return code switch
        {
            "split" => L.T("The processor splits into game and non-game cores"),
            "single" => L.T("One shared L3 and one core class: nothing to split. Priorities, the IRQ exclusion, the background and EcoQoS are what helps; no FPS gain is promised"),
            "groups" => L.F("More than one processor group ({0}): the split is not supported", A(0)),
            "domains" => L.F("More than two L3 domains ({0}): the split needs exactly two", A(0)),
            "nocache" => L.T("Windows did not report the L3 size of every domain: no split by cache"),
            "probe" => L.F("CPU Sets show {0} logical processors, Windows reports {1}: some are hidden, the split is off", A(0), A(1)),
            "64+" => L.T("More than 64 logical processors: the split is not supported"),
            "equal" => L.F("Two L3 domains of the same size ({0} and {1}, {2}): not an X3D, no split by cache; ccd0/ccd1 still work", A(0), A(1), A(2)),
            "mixed-smt" => L.T("SMT is on for some cores and off for others: no symmetric split"),
            "sku" => L.F("{0} should have {1} logical processors (SMT {2}), {3} are visible: the split is off", A(0), A(1), A(2), A(3)),
            _ => code ?? "",
        };
    }
}
