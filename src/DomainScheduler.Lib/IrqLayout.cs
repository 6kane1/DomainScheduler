namespace DomainScheduler;

/// <summary>What a device is for the automatic interrupt layout.</summary>
public enum IrqRole { Mouse, Controller, Gpu, Net, Keyboard }

/// <summary>A device that takes part in the layout, with the role that decides its place.</summary>
public sealed class IrqSlot
{
    public string Key = "";            // device instance id
    public string Name = "";
    public IrqRole Role;
    /// <summary>The result: the processors its interrupts go to (0 = no core: the device keeps the base settings).</summary>
    public ulong Mask;
    /// <summary>Placed, but taken back by the final check (shares a core it must not share).</summary>
    public bool Blocked;
    public string Why = "";
}

/// <summary>
/// The automatic interrupt layout, the same rules as DEVICE-TWEAKER's AUTO mode (its AutoAffinityPlanner, re-written,
/// not copied):
///  - a unit is one physical core and only its first logical processor is used;
///  - only P-cores of the target CCD take part: with two CCDs the one without 3D V-Cache (when its L3 is at least
///    32 MB smaller and the other at least 1.5× bigger), else the CCD with the best-ranked core, else the first;
///  - order of cores: the CPPC preferred-core rank Windows logs at boot (rank, then performance, then position);
///    without a ranking, by position;
///  - core 0 is avoided when there are enough other cores;
///  - order of placement: mouse, game controller, graphics card (two neighbouring cores, preferring no core 0, one
///    CCD, one CCX, then the highest pair), network card (away from the input CCX when a CCD has several), keyboard
///    (shares the mouse core through its SMT sibling when no core is left);
///  - after the graphics card, the network card and the keyboard (after the input only when there is no graphics
///    card) the next free core other than core 0 stays empty, as long as cores are left over;
///  - a final check takes back any slot that shares a core with the graphics card, a network card that shares an
///    input core, and a graphics card without two distinct P-cores.
/// </summary>
public static class IrqLayout
{
    sealed class Unit
    {
        public int Id;          // core index
        public int Lp;          // first logical processor
        public int Sibling = -1;
        public int Ccx, Ccd;
        public bool Core0, E, Used;
        public int Rank = int.MaxValue, Rating;
    }

    /// <summary>Dense CPPC ranks (1 = best) per logical processor, or null when the ranking is missing or flat.</summary>
    public static Dictionary<int, int> Ranks(Topo t, IReadOnlyDictionary<int, int> perf)
    {
        if (perf == null || t?.Sets == null) return null;
        var lps = t.Sets.Where(s => s.Group == 0 && s.LP < 64).Select(s => (int)s.LP).ToList();
        if (lps.Count == 0 || lps.Any(lp => !perf.ContainsKey(lp))) return null;
        var values = lps.Select(lp => perf[lp]).Distinct().OrderByDescending(v => v).ToList();
        if (values.Count < 2) return null;
        return lps.ToDictionary(lp => lp, lp => values.IndexOf(perf[lp]) + 1);
    }

    /// <param name="ccxPairs">AMD family 17h (Zen / Zen 2): two L3 groups (CCX) form one CCD.</param>
    public static List<IrqSlot> Plan(Topo t, IEnumerable<IrqSlot> devices, IReadOnlyDictionary<int, int> perf = null, bool ccxPairs = false)
    {
        var slots = (devices ?? Enumerable.Empty<IrqSlot>()).ToList();
        slots = slots.Select((s, i) => (s, i)).OrderBy(x => x.s.Role).ThenBy(x => x.i).Select(x => x.s).ToList();
        foreach (var s in slots) { s.Mask = 0; s.Blocked = false; s.Why = ""; }
        if (t?.Sets == null || t.Sets.Count == 0 || slots.Count == 0) return slots;

        // ---- units -------------------------------------------------------------------------------------------
        var ranks = Ranks(t, perf);
        byte topEff = t.Sets.Max(s => s.Eff);
        bool hybrid = t.Sets.Select(s => s.Eff).Distinct().Count() > 1;
        var llcOrder = t.Sets.Select(s => s.LLC).Distinct().OrderBy(x => x).ToList();
        var all = t.Sets.Where(s => s.Group == 0 && s.LP < 64).GroupBy(s => s.Core).Select(g =>
        {
            var lps = g.Select(s => (int)s.LP).OrderBy(x => x).ToList();
            int ccx = llcOrder.IndexOf(g.First().LLC);
            return new Unit
            {
                Id = g.Key, Lp = lps[0], Sibling = lps.Count > 1 ? lps[1] : -1, Ccx = ccx, Ccd = ccxPairs ? ccx / 2 : ccx,
                Core0 = lps.Contains(0), E = hybrid && g.First().Eff != topEff,
                Rank = ranks == null ? int.MaxValue : lps.Min(lp => ranks[lp]),
                Rating = perf == null ? 0 : lps.Max(lp => perf.TryGetValue(lp, out int v) ? v : 0),
            };
        }).ToList();
        var p = all.Where(u => !u.E).ToList();
        if (p.Count == 0)
        {
            foreach (var s in slots) s.Why = L.T("Left alone: no performance cores were found");
            return slots;
        }
        // The target CCD.
        var ccds = p.Select(u => u.Ccd).Distinct().OrderBy(x => x).ToList();
        int target = -1;
        if (ccds.Count >= 2)
        {
            ulong L3(int ccd) => (ulong)all.Where(u => u.Ccd == ccd).Select(u => llcOrder[u.Ccx]).Distinct()
                .Sum(l => (decimal)(t.L3ByLlc.TryGetValue(l, out ulong b) ? b : 0));
            if (ccds.Count == 2)
            {
                ulong a = L3(ccds[0]), b = L3(ccds[1]), big = Math.Max(a, b), small = Math.Min(a, b);
                if (small > 0 && big - small >= 32UL << 20 && big * 2 >= small * 3) target = a < b ? ccds[0] : ccds[1];
            }
            if (target < 0) target = ranks != null ? p.OrderBy(u => u.Rank).ThenBy(u => u.Ccd).First().Ccd : ccds[0];
        }
        var pref = target >= 0 ? p.Where(u => u.Ccd == target).ToList() : p;
        if (pref.Count == 0) pref = p;
        pref = ranks != null ? pref.OrderBy(u => u.Rank).ThenByDescending(u => u.Rating).ThenBy(u => u.Lp).ToList() : pref.OrderBy(u => u.Lp).ToList();

        // ---- budget ------------------------------------------------------------------------------------------
        int required = slots.Sum(s => s.Role == IrqRole.Gpu ? 2 : 1);
        int nonCore0 = pref.Count(u => !u.Core0);
        bool avoidCore0 = nonCore0 >= Math.Min(required, pref.Count);
        int eligible = avoidCore0 ? nonCore0 : pref.Count;
        int spare = eligible - Math.Min(eligible, required);
        bool hasGpu = slots.Any(s => s.Role == IrqRole.Gpu);
        bool ccxVisible = pref.GroupBy(u => u.Ccd).Any(g => g.Select(u => u.Ccx).Distinct().Count() > 1);

        Unit Next(IEnumerable<Unit> order = null)
        {
            var list = (order ?? pref).Where(u => !u.Used);
            return avoidCore0 ? list.FirstOrDefault(u => !u.Core0) ?? list.FirstOrDefault() : list.FirstOrDefault();
        }
        void Spacer()
        {
            if (spare <= 0) return;
            var u = pref.FirstOrDefault(x => !x.Used && !x.Core0);
            if (u == null) return;
            u.Used = true;
            spare--;
        }

        // ---- placement ---------------------------------------------------------------------------------------
        Unit input = null;
        var unitsOf = new Dictionary<IrqSlot, List<Unit>>();
        foreach (var s in slots)
        {
            Unit one = null;
            switch (s.Role)
            {
                case IrqRole.Mouse:
                case IrqRole.Controller:
                    one = Next();
                    if (one != null)
                    {
                        input ??= one;
                        s.Why = s.Role == IrqRole.Mouse ? L.T("Mouse controller: a core of its own, the best one") : L.T("Game controller: a core of its own");
                        Take(s, one);
                        if (!hasGpu) Spacer();
                    }
                    break;
                case IrqRole.Gpu:
                {
                    var line = pref.Where(u => !u.E).OrderBy(u => u.Id).ToList();
                    (Unit A, Unit B) best = (null, null);
                    var bestScore = (-1, -1, -1, -1, -1);
                    for (int i = 0; i + 1 < line.Count; i++)
                    {
                        var a = line[i]; var b = line[i + 1];
                        if (a.Used || b.Used) continue;
                        var score = (avoidCore0 && (a.Core0 || b.Core0) ? 0 : 1, a.Ccd == b.Ccd ? 1 : 0, !ccxVisible || a.Ccx == b.Ccx ? 1 : 0,
                            Math.Max(a.Lp, b.Lp), Math.Min(a.Lp, b.Lp));
                        if (score.CompareTo(bestScore) > 0) { bestScore = score; best = (a, b); }
                    }
                    if (best.A == null) { s.Why = L.T("No two neighbouring free cores: the graphics card keeps the base settings"); break; }
                    s.Why = L.T("Graphics card: two neighbouring cores of its own");
                    Take(s, best.A);
                    Take(s, best.B);
                    Spacer();
                    break;
                }
                case IrqRole.Net:
                {
                    IEnumerable<Unit> order = pref;
                    if (ccxVisible && input != null) order = pref.Where(u => u.Ccx != input.Ccx).Concat(pref.Where(u => u.Ccx == input.Ccx));
                    one = Next(order);
                    if (one != null)
                    {
                        s.Why = L.T("Network card: a core of its own; its receive queue (RSS) goes to the same core");
                        Take(s, one);
                        Spacer();
                    }
                    break;
                }
                case IrqRole.Keyboard:
                    one = Next();
                    if (one != null)
                    {
                        s.Why = L.T("Keyboard controller: a core of its own");
                        Take(s, one);
                        Spacer();
                    }
                    else if (input != null)
                    {
                        s.Mask = 1UL << (input.Sibling >= 0 ? input.Sibling : input.Lp);
                        unitsOf[s] = new List<Unit> { input };
                        s.Why = L.T("Keyboard controller: no free core left, shares the mouse core through its second thread");
                    }
                    break;
            }
            if (s.Mask == 0 && s.Why.Length == 0) s.Why = L.T("No free core: the device keeps the base settings");
        }

        void Take(IrqSlot s, Unit u)
        {
            u.Used = true;
            s.Mask |= 1UL << u.Lp;
            if (!unitsOf.TryGetValue(s, out var l)) unitsOf[s] = l = new List<Unit>();
            l.Add(u);
        }

        // ---- final check -------------------------------------------------------------------------------------
        var gpuUnits = slots.Where(s => s.Role == IrqRole.Gpu && unitsOf.ContainsKey(s)).SelectMany(s => unitsOf[s]).ToHashSet();
        var inputUnits = slots.Where(s => s.Role is IrqRole.Mouse or IrqRole.Controller or IrqRole.Keyboard && unitsOf.ContainsKey(s)).SelectMany(s => unitsOf[s]).ToHashSet();
        foreach (var s in slots.Where(unitsOf.ContainsKey))
        {
            var us = unitsOf[s];
            string why = null;
            if (s.Role == IrqRole.Gpu && (us.Distinct().Count() < 2 || us.Any(u => u.E))) why = L.T("needs two distinct performance cores");
            else if (s.Role != IrqRole.Gpu && us.Any(gpuUnits.Contains)) why = L.T("would share a core with the graphics card");
            else if (s.Role == IrqRole.Net && us.Any(inputUnits.Contains)) why = L.T("would share a core with an input device");
            if (why == null) continue;
            s.Mask = 0;
            s.Blocked = true;
            s.Why = L.F("Taken back: {0}; the device keeps the base settings", why);
        }
        return slots;
    }
}
