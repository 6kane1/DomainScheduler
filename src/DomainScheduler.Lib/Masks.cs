using System.Globalization;

namespace DomainScheduler;

/// <summary>Named and numeric core masks (section 5).</summary>
public static class Masks
{
    static readonly Dictionary<string, string> Aliases = new(StringComparer.OrdinalIgnoreCase)
    {
        ["auto"] = "auto", ["a"] = "auto", ["all"] = "all", ["game"] = "game", ["g"] = "game",
        ["nongame"] = "nongame", ["n"] = "nongame", ["vcache"] = "vcache", ["v"] = "vcache",
        ["pcores"] = "pcores", ["p"] = "pcores", ["ecores"] = "ecores", ["e"] = "ecores",
    };

    static string[] Tokens(string text) =>
        (text ?? "").Split(new[] { ',', ';', '+' }, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

    /// <summary>A named spec ("vcache+nosmt", "ccd1;nocore0", "g,nosmt"), null when the text is not one.</summary>
    public static MaskSpec TryParseSpec(string text)
    {
        var tok = Tokens(text);
        if (tok.Length == 0) return null;
        var s = new MaskSpec();
        bool haveBase = false;
        foreach (var raw in tok)
        {
            string t = raw.ToLowerInvariant();
            if (t is "nosmt" or "ns") { s.NoSmt = true; continue; }
            if (t is "nocore0" or "nc0") { s.NoCore0 = true; continue; }
            if (haveBase) return null;
            if (Aliases.TryGetValue(t, out var b)) { s.Base = b; haveBase = true; continue; }
            string num = t.StartsWith("ccd") ? t[3..] : t.StartsWith('c') ? t[1..] : null;
            if (num != null && num.Length > 0 && int.TryParse(num, NumberStyles.None, CultureInfo.InvariantCulture, out int n))
            {
                s.Base = "ccd";
                s.Ccd = n;
                haveBase = true;
                continue;
            }
            return null;
        }
        return s;
    }

    public static string ToSpec(string text) => TryParseSpec(text)?.ToString();

    /// <summary>0xFF, 255 or "0,2,4"; false when the text is none of these.</summary>
    public static bool Parse(string text, out ulong mask)
    {
        mask = 0;
        text = (text ?? "").Trim();
        if (text.Length == 0) return false;
        if (text.StartsWith("0x", StringComparison.OrdinalIgnoreCase))
            return ulong.TryParse(text[2..], NumberStyles.HexNumber, CultureInfo.InvariantCulture, out mask);
        if (text.Contains(',') || text.Contains(' '))
        {
            foreach (var p in text.Split(new[] { ',', ' ' }, StringSplitOptions.RemoveEmptyEntries))
            {
                if (!int.TryParse(p, NumberStyles.None, CultureInfo.InvariantCulture, out int lp) || lp > 63) { mask = 0; return false; }
                mask |= 1UL << lp;
            }
            return mask != 0;
        }
        return ulong.TryParse(text, NumberStyles.None, CultureInfo.InvariantCulture, out mask);
    }

    public sealed class LimitResult
    {
        public ulong Kept, Dropped;
        public bool Ok;
        public string Note = "";
    }

    public static LimitResult Limit(Topo t, ulong mask)
    {
        ulong full = t.FullMask();
        var r = new LimitResult { Kept = mask & full, Dropped = mask & ~full };
        r.Ok = r.Kept != 0;
        if (r.Dropped != 0) r.Note = L.F("Processors {0} do not exist and were dropped", Ranges(r.Dropped));
        return r;
    }

    public static MaskResult FromValue(Topo t, ulong mask)
    {
        var list = t.Sets.Where(x => x.Group == 0 && x.LP < 64 && ((mask >> x.LP) & 1) != 0).ToList();
        string hex = "0x" + mask.ToString("x", CultureInfo.InvariantCulture);
        return new MaskResult
        {
            Mask = mask, Hex = hex, LPs = string.Join(",", list.OrderBy(x => x.LP).Select(x => x.LP)),
            SetIds = list.Select(x => x.Id).ToArray(), Count = list.Count,
            Domains = list.Select(x => x.LLC).Distinct().Count(),
            Portable = false, Satisfied = list.Count > 0, Name = hex, Spec = hex,
        };
    }

    /// <summary>A spec or a number to a mask.</summary>
    public static MaskResult Resolve(Topo t, string text)
    {
        var spec = TryParseSpec(text);
        if (spec != null) return BuildSpec(t, spec);
        if (!Parse(text, out ulong v))
        {
            var r = BuildSpec(t, new MaskSpec());
            r.Satisfied = false;
            r.Notes.Add(L.F("Unknown mask: {0}", text));
            return r;
        }
        var lim = Limit(t, v);
        if (!lim.Ok)
        {
            var r = BuildSpec(t, new MaskSpec());
            r.Satisfied = false;
            r.Notes.Add(lim.Note);
            r.Notes.Add("empty");
            return r;
        }
        var res = FromValue(t, lim.Kept);
        if (lim.Note.Length > 0) res.Notes.Add(lim.Note);
        return res;
    }

    public static MaskResult BuildSpec(Topo t, MaskSpec s)
    {
        bool ok = true;
        var notes = new List<string>();
        IEnumerable<CpuSet> Fail(string why)
        {
            ok = false;
            notes.Add(why);
            return Enumerable.Empty<CpuSet>();
        }
        var top = t.EffClasses.Count > 0 ? t.EffClasses.Max() : (byte)0;
        IEnumerable<CpuSet> src = s.Base switch
        {
            "all" or "auto" => t.Sets,
            "vcache" => t.BigCacheLlc is byte v ? t.Sets.Where(x => x.LLC == v) : Fail("no-vcache"),
            "game" => t.Split.CanSplit ? t.Sets.Where(x => x.LP < 64 && (t.Split.Game >> x.LP & 1) != 0) : Fail(t.Split.Why()),
            "nongame" => t.Split.CanSplit ? t.Sets.Where(x => x.LP < 64 && (t.Split.NonGame >> x.LP & 1) != 0)
                       : t.BigCacheLlc is byte w ? t.Sets.Where(x => x.LLC != w) : Fail("no-split"),
            "pcores" => t.Hybrid ? t.Sets.Where(x => x.Eff == top) : Fail("not-hybrid"),
            "ecores" => t.Hybrid ? t.Sets.Where(x => x.Eff < top) : Fail("not-hybrid"),
            _ when s.Base == "ccd" && s.Ccd >= 0 && s.Ccd < t.LLCGroups.Count => t.Sets.Where(x => x.LLC == t.LLCGroups[s.Ccd]),
            _ => Fail("bad-spec"),
        };
        if (s.NoSmt) src = src.GroupBy(x => x.Core).Select(g => g.OrderBy(x => x.LP).First());
        if (s.NoCore0)
        {
            byte fc = t.FirstCore();
            src = src.Where(x => x.Core != fc);
        }
        var list = src.ToList();
        if (list.Count == 0)
        {
            ok = false;
            notes.Add("empty");
            list = t.Sets.ToList();
        }
        if (s.Base == "vcache" && t.BigCacheLlc.HasValue && t.Split.CanSplit && TopoBuilder.Mask(list) != t.Split.Game && !s.NoSmt && !s.NoCore0)
            notes.Add("vcache-split-mismatch");
        ulong m = TopoBuilder.Mask(list);
        return new MaskResult
        {
            Mask = m, Hex = "0x" + m.ToString("x", CultureInfo.InvariantCulture),
            LPs = string.Join(",", list.OrderBy(x => x.LP).Select(x => x.LP)),
            SetIds = list.Select(x => x.Id).ToArray(), Count = list.Count,
            Domains = list.Select(x => x.LLC).Distinct().Count(),
            Portable = true, Satisfied = ok, Notes = notes, Name = s.ToString(), Spec = s.ToString(),
        };
    }

    /// <summary>"0-7,16-23".</summary>
    public static string Ranges(ulong mask)
    {
        var parts = new List<string>();
        for (int i = 0; i < 64;)
        {
            if (((mask >> i) & 1) == 0) { i++; continue; }
            int j = i;
            while (j + 1 < 64 && ((mask >> (j + 1)) & 1) != 0) j++;
            parts.Add(j > i ? i.ToString(CultureInfo.InvariantCulture) + "-" + j.ToString(CultureInfo.InvariantCulture) : i.ToString(CultureInfo.InvariantCulture));
            i = j + 1;
        }
        return string.Join(",", parts);
    }

    /// <summary>Ideal-processor candidates: one per physical core (lowest LP), P-cores only on hybrid; SMT siblings only when fewer than 6.</summary>
    public static List<CpuSet> IdealCandidates(Topo t, bool skipCpu0, ulong allowMask)
    {
        IEnumerable<CpuSet> src = t.Sets;
        if (allowMask != 0)
        {
            var f = src.Where(s => s.LP < 64 && (allowMask >> s.LP & 1) != 0).ToList();
            if (f.Count > 0) src = f;
        }
        if (t.EffClasses.Count > 1)
        {
            byte mx = t.Sets.Max(s => s.Eff);
            src = src.Where(s => s.Eff == mx).ToList();
        }
        // Section 22.7: on a dual-CCD X3D the big-cache domain goes first, whichever half of the
        // processor numbers it is (so UpperHalf does not change which processors are picked).
        byte? big = t.Split.Family == "x3d" ? t.BigCacheLlc : null;
        var list = src.GroupBy(s => s.Core).Select(g => g.OrderBy(s => s.LP).First())
            .OrderBy(s => big.HasValue && s.LLC != big.Value ? 1 : 0).ThenBy(s => s.LP).ToList();
        if (skipCpu0) list = list.Where(s => s.LP != 0).ToList();
        if (list.Count == 0) list = t.Sets.OrderBy(s => s.LP).ToList();
        if (list.Count < 6) list = list.Concat(src.OrderBy(s => s.LP).Where(s => !list.Any(x => x.LP == s.LP))).ToList();
        return list;
    }

    /// <summary>Where background work goes next to a game mask (not the plain complement).</summary>
    public static ulong BgMask(Topo t, ulong game)
    {
        ulong m = TopoBuilder.Mask(t.Sets.GroupBy(s => s.Core).Where(g => g.All(s => s.LP >= 64 || (game >> s.LP & 1) == 0)).SelectMany(g => g));
        if (m != 0) return m;                                   // 1) whole free cores
        byte fc = t.FirstCore();
        m = TopoBuilder.Mask(t.Sets.Where(s => s.Core == fc && (game >> s.LP & 1) == 0));
        if (m != 0) return m;                                   // 2) free LPs of the first core
        m = t.FullMask() ^ game;
        if (m != 0) return m;                                   // 3) the complement
        m = TopoBuilder.Mask(t.Sets.Where(s => s.Core == fc)); // 4) the first core
        return m != 0 ? m : 1UL;
    }

    /// <summary>What to tell the user about a mask on this processor (empty: nothing).</summary>
    public static List<string> Advice(Topo t, MaskResult r)
    {
        var a = new List<string>();
        foreach (var n in r.Notes) a.Add(NoteText(n));
        if (CoreSplit.IsCoreUltra(t.Name)) a.Add(L.T("Core Ultra: a third core class (LP E-cores) may exist; the game gets only the top class"));
        if (t.Hybrid) a.Add(L.T("Intel hybrid: Thread Director may ignore the ideal processor; the CPU Sets and EcoQoS off matter more"));
        if (t.BigCacheLlc.HasValue) a.Add(L.T("The V-Cache die runs at a lower clock: a CPU-bound game may do better on the other die"));
        return a;
    }

    public static string NoteText(string note) => note switch
    {
        "no-vcache" => L.T("No V-Cache die (needs two L3 domains, one at least twice as large)"),
        "no-split" => L.T("The processor has no game / non-game halves"),
        "not-hybrid" => L.T("Not a hybrid processor: no P- and E-cores"),
        "bad-spec" => L.T("Unknown mask"),
        "empty" => L.T("The mask left no processor: all were taken"),
        "vcache-split-mismatch" => L.T("The V-Cache detection and the split plan give different masks"),
        _ => note,
    };
}
