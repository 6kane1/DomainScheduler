using System.Globalization;
using System.Text.RegularExpressions;

namespace DomainScheduler;

/// <summary>
/// Applies masks, priorities and ideal processors with baselines (section 10).
/// Every write happens only when the current value differs, every change can be
/// reverted, SetThreadAffinityMask is never called.
/// </summary>
public sealed class Engine
{
    public static readonly Dictionary<string, uint> PRIO = new(StringComparer.OrdinalIgnoreCase)
    { ["idle"] = 64, ["below"] = 16384, ["normal"] = 32, ["above"] = 32768, ["high"] = 128, ["realtime"] = 256 };
    public static readonly Dictionary<string, int> TPRIO = new(StringComparer.OrdinalIgnoreCase)
    { ["idle"] = -15, ["lowest"] = -2, ["below"] = -1, ["normal"] = 0, ["above"] = 1, ["highest"] = 2, ["critical"] = 15 };
    public static readonly Dictionary<string, uint> IOP = new(StringComparer.OrdinalIgnoreCase)
    { ["verylow"] = 0, ["low"] = 1, ["normal"] = 2, ["high"] = 3, ["critical"] = 4 };
    public static readonly Dictionary<string, uint> MEM = new(StringComparer.OrdinalIgnoreCase)
    { ["verylow"] = 1, ["low"] = 2, ["medium"] = 3, ["belownormal"] = 4, ["normal"] = 5 };

    /// <summary>Section 22.10: masks and affinity are always compiled in; there is no runtime gate.</summary>
    public const bool FeatureAffinity = true;

    readonly INative _n;
    readonly object _gate = new();
    readonly Dictionary<string, uint> _thrSets = new();        // "pid:tid" → processor whose CPU Set the thread is limited to
    readonly HashSet<string> _warnOnce = new();
    bool _timerRequested;
    uint _timerValue;
    readonly Dictionary<uint, ProcBefore> _procBefore = new();
    readonly Dictionary<uint, string> _procName = new();
    readonly HashSet<uint> _bgPids = new();
    readonly Dictionary<string, ThrBefore> _thrBefore = new();
    readonly Dictionary<string, uint> _ideal = new();
    readonly Dictionary<string, ThrPrevE> _thrPrev = new();
    readonly Dictionary<string, ThrMetaE> _thrMeta = new();
    readonly Dictionary<uint, ModCacheE> _modCache = new();
    readonly Dictionary<uint, long> _born = new();
    Dictionary<uint, List<uint>> _tidCache;
    Dictionary<uint, (string Name, uint Parent)> _pidCache;
    readonly Dictionary<string, int> _presetTidCount = new();
    readonly HashSet<string> _adviceOnce = new();
    readonly HashSet<string> _skipField = new();               // "pid:field" Windows refused once: not tried again for this process
    readonly Dictionary<string, ulong> _written = new();       // "pid:field" written while the value cannot be read back
    readonly HashSet<string> _unsupported = new();             // fields this Windows does not support at all (error 1 / 50)
    long _setCalls;
    bool _debug;

    public readonly List<ApplyReport> LastReports = new();
    public readonly List<string> Advice = new();
    public Func<DateTime> Now = () => DateTime.UtcNow;
    public Action<int> Sleep = ms => Thread.Sleep(ms);
    /// <summary>Called after baselines were captured and before the first change (crash-safe session).</summary>
    public Action BeforeMutate;
    /// <summary>Why a process must not be touched (anti-cheat); null = may.</summary>
    public Func<uint, string, string> GateHook;

    public Engine(INative native) => _n = native;

    public long SetCalls => Interlocked.Read(ref _setCalls);

    bool Set(bool ok)
    {
        Interlocked.Increment(ref _setCalls);
        return ok;
    }

    static string Key(uint pid, uint tid) => pid.ToString(CultureInfo.InvariantCulture) + ":" + tid.ToString(CultureInfo.InvariantCulture);

    void AdviceOnce(string key, string text)
    {
        if (string.IsNullOrEmpty(text) || !_adviceOnce.Add(key)) return;
        Advice.Add(text);
        AppLog.Add(text, AppLog.Sev.Warn);
    }

    public static string ModuleOf(string sym)
    {
        if (string.IsNullOrEmpty(sym)) return "";
        int i = sym.IndexOf('+');
        return i >= 0 ? sym[..i] : sym;
    }

    public static bool IsDeniedModule(ThreadRow r) => Guard.IsAntiCheat(ModuleOf(r.Sym));
    static bool SafeModule(ThreadRow r) => !IsDeniedModule(r);
    public static bool ThreadEligible(ThreadRow r, bool prot) => prot ? !IsDeniedModule(r) : SafeModule(r);

    // ---- passes -----------------------------------------------------------------------------------

    /// <summary>Starts a pass: one toolhelp snapshot of threads and processes for all Applies of the pass.</summary>
    public void BeginPass()
    {
        lock (_gate)
        {
            _tidCache = _n.ThreadsByPid();
            FillPidCache();
        }
    }

    public void EndPass()
    {
        lock (_gate)
        {
            _tidCache = null;
            _pidCache = null;
        }
    }

    void FillPidCache() => _pidCache = _n.Processes();

    public string NameOf(uint pid)
    {
        lock (_gate)
        {
            // Outside a pass the list is read anew: a cache left behind would hide ended processes.
            var procs = _pidCache ?? _n.Processes();
            return procs.TryGetValue(pid, out var p) ? p.Name : "";
        }
    }

    /// <summary>The processes of the current pass (or read now).</summary>
    public Dictionary<uint, (string Name, uint Parent)> ProcessesNow()
    {
        lock (_gate) return _pidCache ?? _n.Processes();
    }

    List<uint> TidsOf(uint pid)
    {
        var tc = _tidCache ?? _n.ThreadsByPid();
        return tc.TryGetValue(pid, out var l) ? l : new List<uint>();
    }

    void Forget(uint pid)
    {
        string prefix = pid.ToString(CultureInfo.InvariantCulture) + ":";
        _procBefore.Remove(pid);
        _procName.Remove(pid);
        _bgPids.Remove(pid);
        _modCache.Remove(pid);
        _born.Remove(pid);
        _snaps.Remove(pid);
        RemovePrefix(_thrBefore, prefix);
        RemovePrefix(_ideal, prefix);
        RemovePrefix(_thrSets, prefix);
        RemovePrefix(_thrPrev, prefix);
        RemovePrefix(_thrMeta, prefix);
        RemovePrefix(_presetTidCount, prefix);
        _skipField.RemoveWhere(k => k.StartsWith(prefix, StringComparison.Ordinal));
        RemovePrefix(_written, prefix);
    }

    static void RemovePrefix<T>(Dictionary<string, T> d, string prefix)
    {
        foreach (var k in d.Keys.Where(k => k.StartsWith(prefix, StringComparison.Ordinal)).ToList()) d.Remove(k);
    }

    // ---- snapshot ---------------------------------------------------------------------------------

    /// <summary>The threads of a process with their cycle rate, start symbol and name.</summary>
    public ThreadSnap Snapshot(uint pid)
    {
        lock (_gate) return SnapshotLocked(pid);
    }

    ThreadSnap SnapshotLocked(uint pid)
    {
        var snap = SnapshotCore(pid);
        if (snap.Level >= 0) _snaps[pid] = snap;
        return snap;
    }

    ThreadSnap SnapshotCore(uint pid)
    {
        var snap = new ThreadSnap();
        IntPtr ph = _n.OpenProcessGraceful(pid, out int level);
        snap.Level = level;
        if (level < 0) return snap;
        try
        {
            long created = _n.ProcessCreated(ph);
            if (_born.TryGetValue(pid, out long b) && b != created) Forget(pid);
            _born[pid] = created;
            DateTime now = Now();
            List<ModRange> mods = null;
            foreach (uint tid in TidsOf(pid))
            {
                IntPtr th = _n.OpenThreadOf(pid, tid);
                if (th == IntPtr.Zero) continue;
                try
                {
                    string key = Key(pid, tid);
                    _n.ThreadTimes(th, out long born, out ulong kernelUser);
                    if (_thrMeta.TryGetValue(key, out var meta) && meta.Born != born)
                    {
                        _thrMeta.Remove(key);
                        _thrPrev.Remove(key);
                        _thrBefore.Remove(key);
                        _ideal.Remove(key);
                        meta = null;
                    }
                    if (meta == null)
                    {
                        meta = new ThrMetaE { Born = born, Desc = _n.ThreadDesc(th) ?? "" };
                        if (level >= 2)
                        {
                            mods ??= ModulesOf(pid, ph);
                            meta.Sym = Symbol(_n.ThreadStart(th), ref mods, pid, ph);
                        }
                        _thrMeta[key] = meta;
                    }
                    meta.Seen = now;
                    ulong c;
                    if (!_n.ThreadCycles(th, out c))
                    {
                        c = kernelUser;
                        snap.CycleFallbackTids.Add(tid);
                    }
                    ulong rate = 0;
                    if (_thrPrev.TryGetValue(key, out var prev))
                    {
                        double dt = (now - prev.T).TotalSeconds;
                        if (dt > 0.05 && c >= prev.C) rate = (ulong)((c - prev.C) / dt);
                    }
                    _thrPrev[key] = new ThrPrevE { C = c, T = now };
                    _n.GetThreadPrio(th, out int prio);
                    snap.Rows.Add(new ThreadRow { Tid = tid, Rate = rate, Prio = prio, Sym = meta.Sym, Desc = meta.Desc, Born = born });
                }
                finally { _n.Close(th); }
            }
            snap.Rows.Sort((a, b2) => b2.Rate.CompareTo(a.Rate));
            snap.Ok = true;
        }
        finally { _n.Close(ph); }
        return snap;
    }

    List<ModRange> ModulesOf(uint pid, IntPtr ph)
    {
        if (_modCache.TryGetValue(pid, out var c) && Now() - c.T < TimeSpan.FromSeconds(60)) return c.M;
        c = new ModCacheE { M = _n.Modules(ph), T = Now() };
        _modCache[pid] = c;
        return c.M;
    }

    string Symbol(ulong addr, ref List<ModRange> mods, uint pid, IntPtr ph)
    {
        if (addr == 0) return "";
        string Find(List<ModRange> m)
        {
            foreach (var r in m)
                if (addr >= r.Base && addr < r.Base + r.Size)
                    return r.Name + "+0x" + (addr - r.Base).ToString("x", CultureInfo.InvariantCulture);
            return "";
        }
        string s = Find(mods);
        if (s.Length > 0) return s;
        // Not resolved: a module may have loaded since; refresh when the cache is older than 5 s.
        if (_modCache.TryGetValue(pid, out var c) && Now() - c.T > TimeSpan.FromSeconds(5))
        {
            _modCache.Remove(pid);
            mods = ModulesOf(pid, ph);
            s = Find(mods);
        }
        return s;
    }

    // ---- matching ---------------------------------------------------------------------------------

    static readonly Regex ModuleExt = new(@"\.(dll|exe|sys|drv|ocx|cpl|mui|bin)$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);

    public static string MatchFor(string pattern) => ModuleExt.IsMatch((pattern ?? "").Trim()) ? "module" : "desc";

    public static Regex Wildcard(string p) =>
        new("^" + Regex.Escape(p ?? "").Replace("\\*", ".*").Replace("\\?", ".") + "$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);

    public static List<ThreadRow> RuleHits(ThreadRule rule, IEnumerable<ThreadRow> rows)
    {
        string pat = (rule.Pattern ?? "").Trim();
        if (pat.Length == 0) return new List<ThreadRow>();
        if (rule.Match == "tid")
            return uint.TryParse(pat, NumberStyles.None, CultureInfo.InvariantCulture, out uint tid) ? rows.Where(r => r.Tid == tid).ToList() : new List<ThreadRow>();
        var re = Wildcard(pat);
        string match = string.IsNullOrEmpty(rule.Match) ? MatchFor(pat) : rule.Match;
        bool noExt = !pat.Contains('.');
        var hits = new List<ThreadRow>();
        foreach (var r in rows)
        {
            bool hit = false;
            if (match == "module")
            {
                string mod = ModuleOf(r.Sym);
                if (noExt && mod.Contains('.')) mod = mod[..mod.LastIndexOf('.')];
                hit = mod.Length > 0 && re.IsMatch(mod);
            }
            if (!hit) hit = r.Desc.Length > 0 && re.IsMatch(r.Desc);   // fallback to the thread name
            if (hit) hits.Add(r);
        }
        return hits;
    }

    public static List<ThreadRow> FilterTid(List<ThreadRow> hits, string tid)
    {
        tid = (tid ?? "").Trim().ToLowerInvariant();
        if (tid.Length == 0 || hits.Count == 0) return hits;
        var s = hits.OrderBy(r => r.Tid).ToList();
        if (tid == "first") return s.Take(1).ToList();
        if (tid == "last") return s.Skip(s.Count - 1).ToList();
        return int.TryParse(tid, NumberStyles.None, CultureInfo.InvariantCulture, out int n) && n < s.Count ? new List<ThreadRow> { s[n] } : new List<ThreadRow>();
    }

    // ---- baselines ----------------------------------------------------------------------------------

    ProcBefore Baseline(uint pid, IntPtr ph, long created, string name, bool background)
    {
        if (_procBefore.TryGetValue(pid, out var b) && b.Created == created) return b;
        b = new ProcBefore { Have = true, Created = created };
        b.PrioSet = _n.GetPriorityClass(ph, out b.Prio);
        b.AffSet = _n.GetAffinity(ph, out b.Aff);
        b.SetsSet = _n.GetCpuSets(ph, out b.Sets);
        b.IoSet = _n.GetIo(ph, out b.Io);
        b.MemSet = _n.GetMem(ph, out b.Mem);
        b.EcoSet = _n.GetEco(ph, out b.EcoCtl, out b.EcoState);
        b.BoostSet = _n.GetBoostDisabled(ph, out b.BoostDisabled);
        _procBefore[pid] = b;
        _procName[pid] = name ?? "";
        if (background) _bgPids.Add(pid);
        else _bgPids.Remove(pid);
        return b;
    }

    static string IdsKey(uint[] ids) => string.Join(",", (ids ?? Array.Empty<uint>()).OrderBy(x => x));

    uint[] IdsFor(Topo t, ulong mask)
    {
        if (t == null || mask == t.FullMask()) return Array.Empty<uint>();
        return t.Sets.Where(s => s.Group == 0 && s.LP < 64 && ((mask >> s.LP) & 1) != 0).Select(s => s.Id).OrderBy(x => x).ToArray();
    }

    // ---- apply ------------------------------------------------------------------------------------

    /// <summary>The preset path (rules from the store); the mask comes from the options or the rule.</summary>
    public ApplyReport Apply(uint pid, Preset preset, ApplyOptions o, Dictionary<string, string> affRules = null)
    {
        lock (_gate)
        {
            var t = TopoBuilder.Live();
            string exe = NameOf(pid);
            ulong mask = 0;
            var rep = new ApplyReport { Exe = exe, Pid = (int)pid };
            if (t != null && o.DoMask)
            {
                string spec = affRules != null && affRules.TryGetValue(exe, out var r) ? r : o.MaskName;
                var mr = Masks.Resolve(t, spec ?? "auto");
                if (!mr.Satisfied)
                    foreach (var n in mr.Notes) AdviceOnce("mask:" + exe + n, Masks.NoteText(n));
                var gm = GameMask.ForRule(t, mr.Mask, GameMask.Mode);
                mask = Reservation.Unmix(gm.Final, Reservation.Reserved(), false);
                if (!t.Split.CanSplit) AdviceOnce("split", t.Split.Why());
                foreach (var a in Masks.Advice(t, mr)) AdviceOnce("adv:" + a, a);
            }
            return ApplyCore(pid, exe, preset, mask, o, rep);
        }
    }

    /// <summary>The Auto path (section 17.2): a synthesized preset and a mask computed by the caller.</summary>
    public ApplyReport ApplyAuto(uint pid, Preset synthesized, ulong mask, ApplyOptions o)
    {
        lock (_gate)
        {
            string exe = NameOf(pid);
            return ApplyCore(pid, exe, synthesized, mask, o, new ApplyReport { Exe = exe, Pid = (int)pid });
        }
    }

    ApplyReport ApplyCore(uint pid, string exe, Preset preset, ulong mask, ApplyOptions o, ApplyReport rep)
    {
        // 1. Debug privilege (once).
        if (!_debug) _debug = _n.EnableDebugPrivilege();
        // 2. Gate.
        string why = GateHook?.Invoke(pid, exe);
        if (why != null)
        {
            AdviceOnce("gate:" + exe, why);
            rep.Failures.Add(why);
            Report(rep);
            return rep;
        }
        // 3. Topology.
        var t = TopoBuilder.Live();
        if (t != null && t.Groups > 1) AdviceOnce("groups", L.T("More than one processor group: masks and ideal processors work in group 0 only"));
        // 7. The process.
        IntPtr ph = _n.OpenProcessGraceful(pid, out int level);
        rep.HandleLevel = level;
        if (level < 0)
        {
            rep.Failures.Add(L.T("The process cannot be opened"));
            Report(rep);
            return rep;
        }
        try
        {
            bool prot = _n.IsProtected(ph);
            if (prot && !o.AllowElevatedPriority)
            {
                rep.Failures.Add(L.F("{0} is a protected process", exe));
                Report(rep);
                return rep;
            }
            long created = _n.ProcessCreated(ph);
            if (_born.TryGetValue(pid, out long b) && b != created) Forget(pid);
            _born[pid] = created;
            bool fresh = !_procBefore.ContainsKey(pid);
            var before = Baseline(pid, ph, created, exe, false);
            if (fresh) BeforeMutate?.Invoke();

            if (PRIO.TryGetValue(preset.Class ?? "", out uint cls) && cls != PRIO["realtime"] && cls != PRIO["high"])
            {
                bool r = _n.GetPriorityClass(ph, out uint cur);
                Field(pid, "class", r, cur, cls, () => _n.SetPriorityClass(ph, cls), rep, L.T("Priority class"));
            }
            if (IOP.TryGetValue(preset.Io ?? "", out uint io))
            {
                bool r = _n.GetIo(ph, out uint cur);
                Field(pid, "io", r, cur, io, () => _n.SetIo(ph, io), rep, L.T("I/O priority"));
            }
            if (MEM.TryGetValue(preset.Mem ?? "", out uint mem))
            {
                bool r = _n.GetMem(ph, out uint cur);
                Field(pid, "mem", r, cur, mem, () => _n.SetMem(ph, mem), rep, L.T("Memory priority"));
            }
            var (ectl, estate, eco) = EcoOf(preset.Eco);
            if (eco)
            {
                bool r = _n.GetEco(ph, out uint c0, out uint s0);
                Field(pid, "eco", r, ((ulong)c0 << 32) | s0, ((ulong)ectl << 32) | estate, () => _n.SetEco(ph, ectl, estate), rep, L.T("EcoQoS"));
            }
            if (preset.Boost == "on" || preset.Boost == "off")
            {
                bool dis = preset.Boost == "off";
                bool r = _n.GetBoostDisabled(ph, out bool cur);
                Field(pid, "boost", r, cur ? 1UL : 0, dis ? 1UL : 0, () => _n.SetBoostDisabled(ph, dis), rep, L.T("Priority boost"));
            }
            if (FeatureAffinity && o.DoMask && t != null && mask != 0)
            {
                if (level >= 1)
                {
                    bool r = _n.GetAffinity(ph, out ulong curAff);
                    Field(pid, "affinity", r, curAff, mask, () => _n.SetAffinity(ph, mask), rep, L.T("Affinity"));
                }
                var ids = IdsFor(t, mask);
                string want = IdsKey(ids);
                string key = pid.ToString(CultureInfo.InvariantCulture) + ":sets";
                bool readable = _n.GetCpuSets(ph, out uint[] curIds);
                if (!_skipField.Contains(key) && (readable ? IdsKey(curIds) != want : !_written.TryGetValue(key, out ulong wh) || wh != (ulong)want.GetHashCode()))
                {
                    if (Set(_n.SetCpuSets(ph, ids)))
                    {
                        if (!readable) _written[key] = (ulong)want.GetHashCode();
                    }
                    else
                    {
                        _skipField.Add(key);
                        rep.Failures.Add(L.F("CPU Sets: error {0}", _n.LastError()));
                    }
                }
                rep.MaskHex = "0x" + mask.ToString("x", CultureInfo.InvariantCulture);
                rep.MaskLps = Masks.Ranges(mask);
            }
            // 8. Threads.
            if (o.DoThreads && (preset.Threads.Any(r => r.Enabled && !string.IsNullOrWhiteSpace(r.Pattern)) || HeatOn(preset)))
                ApplyThreads(pid, exe, preset, mask, o, rep, t, prot);
        }
        finally { _n.Close(ph); }
        Report(rep);
        return rep;
    }

    /// <summary>
    /// Writes one process field only when it differs. A value that cannot be read is compared with
    /// what was written before; a field Windows refused is not tried again for this process.
    /// </summary>
    void Field(uint pid, string field, bool readable, ulong current, ulong want, Func<bool> write, ApplyReport rep, string what)
    {
        string key = pid.ToString(CultureInfo.InvariantCulture) + ":" + field;
        if (_unsupported.Contains(field) || _skipField.Contains(key)) return;
        if (readable ? current == want : _written.TryGetValue(key, out ulong w) && w == want) return;
        if (Set(write()))
        {
            if (!readable) _written[key] = want;
            return;
        }
        int err = _n.LastError();
        if (err is 1 or 50)   // ERROR_INVALID_FUNCTION / ERROR_NOT_SUPPORTED: not a failure, the feature is off here
        {
            if (_unsupported.Add(field))
                AppLog.Add(field == "eco" ? L.T("EcoQoS is off in this Windows (power throttling disabled): the game cannot be throttled anyway, nothing to set")
                    : L.F("{0}: not supported by this Windows, skipped", what), AppLog.Sev.Info);
            return;
        }
        _skipField.Add(key);
        rep.Failures.Add(L.F("{0}: error {1}", what, err));
    }

    static (uint ctl, uint state, bool set) EcoOf(string eco) => (eco ?? "").ToLowerInvariant() switch
    {
        "off" => (1u, 0u, true),
        "on" => (1u, 1u, true),
        "default" => (0u, 0u, true),
        _ => (0u, 0u, false),
    };

    /// <summary>Threads of the last snapshot per target (read by the Auto planner).</summary>
    public ThreadSnap LastSnap { get; private set; }
    /// <summary>Share of the target threads that could not be opened or changed in the last Apply.</summary>
    public double LastDeniedShare { get; private set; }

    void ApplyThreads(uint pid, string exe, Preset preset, ulong mask, ApplyOptions o, ApplyReport rep, Topo t, bool prot)
    {
        var cand = t != null ? Masks.IdealCandidates(t, false, mask) : new List<CpuSet>();
        if (o.Warmup)
        {
            SnapshotLocked(pid);
            Sleep(700);
        }
        var snap = SnapshotLocked(pid);
        LastSnap = snap;
        rep.LastThreads = snap.Rows.Count;
        rep.CycleFallbackTids.AddRange(snap.CycleFallbackTids);
        var ac = snap.Rows.FirstOrDefault(IsDeniedModule);
        if (ac != null) AdviceOnce("ac:" + exe, L.F("{0} has anti-cheat threads ({1}): they are not touched", exe, ModuleOf(ac.Sym)));
        var alive = new HashSet<string>(snap.Rows.Select(r => Key(pid, r.Tid)));
        var used = new HashSet<uint>(_ideal.Where(kv => alive.Contains(kv.Key)).Select(kv => kv.Value));
        int ci = 0;
        uint Next()
        {
            for (int i = 0; i < cand.Count; i++)
            {
                uint lp = cand[(ci + i) % cand.Count].LP;
                if (used.Contains(lp)) continue;
                used.Add(lp);
                ci += i + 1;
                return lp;
            }
            return cand[ci++ % cand.Count].LP;
        }
        int targets = 0, denied = 0;
        var hot = new HashSet<uint>();
        foreach (var rule in preset.Threads.Where(r => r.Enabled && !string.IsNullOrWhiteSpace(r.Pattern)))
        {
            foreach (var row in FilterTid(RuleHits(rule, snap.Rows), rule.Tid))
            {
                if (!ThreadEligible(row, prot)) continue;
                targets++;
                string key = Key(pid, row.Tid);
                IntPtr th = _n.OpenThreadOf(pid, row.Tid);
                if (th == IntPtr.Zero)
                {
                    denied++;
                    rep.Failures.Add(L.F("Thread {0} cannot be opened", row.Tid));
                    continue;
                }
                try
                {
                    _n.ThreadTimes(th, out long born, out _);
                    if (born != row.Born) continue;   // the ID was reused since the snapshot
                    rep.NamedHits++;
                    hot.Add(row.Tid);
                    ThreadBaseline(th, key, born);
                    bool failed = false;
                    if (TPRIO.TryGetValue(rule.Prio ?? "", out int want) || int.TryParse(rule.Prio, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out want) && TPRIO.ContainsValue(want))
                        if (want != 15 && (!_n.GetThreadPrio(th, out int cur) || cur != want))
                            if (!Set(_n.SetThreadPrio(th, want))) { failed = true; rep.Failures.Add(L.F("Thread {0} priority: error {1}", row.Tid, _n.LastError())); }
                    if (rule.Boost == "on" || rule.Boost == "off")
                    {
                        bool dis = rule.Boost == "off";
                        if (!_n.GetThreadBoostDisabled(th, out bool cur) || cur != dis)
                            if (!Set(_n.SetThreadBoostDisabled(th, dis))) { failed = true; rep.Failures.Add(L.F("Thread {0} boost: error {1}", row.Tid, _n.LastError())); }
                    }
                    if (t != null && cand.Count > 0 && !string.IsNullOrEmpty(rule.Ideal))
                    {
                        if (rule.Ideal == "auto")
                        {
                            if (!_ideal.ContainsKey(key))
                            {
                                uint lp = Next();
                                _ideal[key] = lp;
                                if (!Set(_n.SetThreadIdeal(th, lp))) { failed = true; _ideal.Remove(key); rep.Failures.Add(L.F("Thread {0} ideal processor: error {1}", row.Tid, _n.LastError())); }
                            }
                        }
                        else if (int.TryParse(rule.Ideal, NumberStyles.None, CultureInfo.InvariantCulture, out int n) && n >= 0 && n < t.LogicalCount)
                        {
                            if (!_ideal.TryGetValue(key, out uint old) || old != (uint)n)
                            {
                                _ideal[key] = (uint)n;
                                if (!Set(_n.SetThreadIdeal(th, (uint)n))) { failed = true; _ideal.Remove(key); rep.Failures.Add(L.F("Thread {0} ideal processor: error {1}", row.Tid, _n.LastError())); }
                            }
                        }
                    }
                    if (!string.IsNullOrEmpty(rule.Ideal) && t != null && _ideal.TryGetValue(key, out uint ilp)) ThreadSets(pid, exe, th, key, t, ilp, rule.Pin);
                    if (failed && _n.LastError() == 5) denied++;
                }
                finally { _n.Close(th); }
            }
        }
        // Heat policy (section 22.6): the hottest matched thread gets HotFirst, the next ones HotOthers.
        if (HeatOn(preset) && t != null && cand.Count > 0)
        {
            bool anyRule = preset.Threads.Any(r => r.Enabled && !string.IsNullOrWhiteSpace(r.Pattern));
            var ranked = snap.Rows.Where(r => r.Rate > 0 && ThreadEligible(r, prot) && (!anyRule || hot.Contains(r.Tid))).OrderByDescending(r => r.Rate).ToList();
            int budget = Math.Max(1, cand.Count - AutoPolicy.OsReservedCores);
            var slots = ranked.Take(budget).Select(r => new Classified { Row = r, Role = AutoLogic.Classify(r, snap) }).ToList();
            bool sure = AutoLogic.Confidence(snap, slots) >= AutoPolicy.ConfidenceFloor;
            for (int i = 0; i < slots.Count; i++)
            {
                string policy = i == 0 ? preset.HotFirst : preset.HotOthers;
                if (policy is not ("pin" or "ideal")) continue;
                var row = slots[i].Row;
                string key = Key(pid, row.Tid);
                IntPtr th = _n.OpenThreadOf(pid, row.Tid);
                if (th == IntPtr.Zero) continue;
                try
                {
                    _n.ThreadTimes(th, out long born, out _);
                    if (born != row.Born) continue;
                    ThreadBaseline(th, key, born);
                    if (!_ideal.TryGetValue(key, out uint lp))
                    {
                        lp = Next();
                        _ideal[key] = lp;
                        if (!Set(_n.SetThreadIdeal(th, lp)))
                        {
                            _ideal.Remove(key);
                            rep.Failures.Add(L.F("Thread {0} ideal processor: error {1}", row.Tid, _n.LastError()));
                            continue;
                        }
                    }
                    bool role = slots[i].Role is ThreadRole.Main or ThreadRole.Render or ThreadRole.Audio or ThreadRole.Worker;
                    ThreadSets(pid, exe, th, key, t, lp, policy == "pin" && sure && role);
                }
                finally { _n.Close(th); }
            }
        }
        _presetTidCount[Key(pid, 0)] = targets;
        LastDeniedShare = targets == 0 ? 0 : (double)denied / targets;
    }

    static bool HeatOn(Preset p) => p.HotFirst is "pin" or "ideal" || p.HotOthers is "pin" or "ideal";

    /// <summary>What the thread had before the first change: priority, boost, selected CPU Sets.</summary>
    void ThreadBaseline(IntPtr th, string key, long born)
    {
        if (_thrBefore.ContainsKey(key)) return;
        var tb = new ThrBefore { Born = born };
        tb.PrioSet = _n.GetThreadPrio(th, out tb.Prio);
        tb.BoostSet = _n.GetThreadBoostDisabled(th, out tb.BoostDisabled);
        tb.SetsSet = _n.GetThreadCpuSets(th, out tb.Sets);
        tb.IdealSet = _n.GetThreadIdeal(th, out tb.Ideal) && tb.Ideal < 64;     // kept in the session too: a crash restore puts it back
        _thrBefore[key] = tb;
        BeforeMutate?.Invoke();
    }

    /// <summary>
    /// Section 22.4: limits a pinned thread to the CPU Set of its ideal processor (pin = true) or
    /// gives back what it had (pin = false). A refused call degrades to "ideal only" with one
    /// warning; it is not a failure of the pin.
    /// </summary>
    void ThreadSets(uint pid, string exe, IntPtr th, string key, Topo t, uint lp, bool pin)
    {
        if (!pin)
        {
            if (_thrSets.Remove(key) && _thrBefore.TryGetValue(key, out var tb) && tb.SetsSet) Set(_n.SetThreadCpuSets(th, tb.Sets));
            return;
        }
        string refused = key + ":tsets";
        if (_skipField.Contains(refused) || _thrSets.TryGetValue(key, out uint old) && old == lp) return;
        var sets = t.Sets.Where(s => s.Group == 0 && s.LP == lp).Select(s => s.Id).Take(1).ToArray();
        if (sets.Length == 1 && Set(_n.SetThreadCpuSets(th, sets)))
        {
            _thrSets[key] = lp;
            return;
        }
        _skipField.Add(refused);
        if (_warnOnce.Add("tsets:" + pid.ToString(CultureInfo.InvariantCulture)))
            AppLog.Add(L.F("{0}: thread CPU Sets were refused (error {1}); the threads keep the ideal processor only", exe, _n.LastError()), AppLog.Sev.Warn);
    }

    // ---- dynamic placement (the Top-5 mode): the caller keeps the thread handle open ---------------------

    internal INative Native => _n;

    /// <summary>The threads of a process from the current pass (or read now).</summary>
    public List<uint> ThreadsNow(uint pid)
    {
        lock (_gate) return TidsOf(pid).ToList();
    }

    /// <summary>Why the process may not be touched (denylist); null = it may.</summary>
    public string Gate(uint pid)
    {
        lock (_gate) return GateHook?.Invoke(pid, NameOf(pid));
    }

    /// <summary>
    /// Puts a thread on a processor (ideal, a hint) with a priority; prio = null gives the thread's
    /// own priority back. The baseline (priority, boost, ideal) is taken before the first change.
    /// Writes only what differs. Never above "highest".
    /// </summary>
    public bool PlaceThread(uint pid, uint tid, IntPtr th, long born, int? prio, uint lp)
    {
        lock (_gate)
        {
            string key = Key(pid, tid);
            if (_thrBefore.TryGetValue(key, out var old) && old.Born != born) _thrBefore.Remove(key);
            ThreadBaseline(th, key, born);
            var tb = _thrBefore[key];
            if (!tb.IdealSet) tb.IdealSet = _n.GetThreadIdeal(th, out tb.Ideal);
            bool ok = true;
            int want = prio.HasValue ? Math.Clamp(prio.Value, -2, 2) : tb.Prio;
            if ((prio.HasValue || tb.PrioSet) && (!_n.GetThreadPrio(th, out int cur) || cur != want)) ok &= Set(_n.SetThreadPrio(th, want));
            if (!_ideal.TryGetValue(key, out uint had) || had != lp)
            {
                if (Set(_n.SetThreadIdeal(th, lp))) _ideal[key] = lp;
                else ok = false;
            }
            return ok;
        }
    }

    /// <summary>Gives a thread back what it had before PlaceThread; returns how many values were written.</summary>
    public int ReleaseThread(uint pid, uint tid, IntPtr th)
    {
        lock (_gate)
        {
            string key = Key(pid, tid);
            _ideal.Remove(key);
            if (!_thrBefore.TryGetValue(key, out var tb)) return 0;
            int n = 0;
            _n.ThreadTimes(th, out long born, out _);
            if (born == tb.Born)
            {
                if (tb.PrioSet && (!_n.GetThreadPrio(th, out int cur) || cur != tb.Prio) && Set(_n.SetThreadPrio(th, tb.Prio))) n++;
                if (tb.IdealSet && Set(_n.SetThreadIdeal(th, tb.Ideal))) n++;
            }
            _thrBefore.Remove(key);
            return n;
        }
    }

    /// <summary>Threads limited to one CPU Set right now ("pid:tid" → processor).</summary>
    public IReadOnlyDictionary<string, uint> ThreadPins
    {
        get { lock (_gate) return new Dictionary<string, uint>(_thrSets); }
    }

    // ---- timer resolution (section 22.5) ------------------------------------------------------------

    public bool TimerRequested { get { lock (_gate) return _timerRequested; } }

    /// <summary>Asks for the finest system timer resolution while a game is active. False when refused or unavailable.</summary>
    public bool TimerRequest()
    {
        lock (_gate)
        {
            if (_timerRequested) return true;
            if (Flags.Refuse(Flags.TimerRes, L.T("Timer resolution"))) return false;
            if (!_n.TimerQuery(out _, out uint finest, out uint current))
            {
                AppLog.Add(L.T("Timer resolution: Windows did not answer, left as it is"), AppLog.Sev.Warn);
                return false;
            }
            _timerRequested = true;      // written to the session before the request
            _timerValue = finest;
            BeforeMutate?.Invoke();
            if (!_n.TimerSet(finest, true, out uint actual))
            {
                _timerRequested = false;
                AppLog.Add(L.T("Timer resolution: the request was refused"), AppLog.Sev.Warn);
                return false;
            }
            AppLog.Add(L.F("Timer resolution requested: {0:0.00} ms (was {1:0.00} ms)", actual / 10000.0, current / 10000.0));
            return true;
        }
    }

    /// <summary>Gives the timer request back (revert, fault, game exit). Idempotent.</summary>
    public bool TimerRelease()
    {
        lock (_gate)
        {
            if (!_timerRequested) return false;
            _timerRequested = false;
            bool ok = _n.TimerSet(_timerValue, false, out uint actual);
            AppLog.Add(ok ? L.F("Timer resolution released: {0:0.00} ms now", actual / 10000.0) : L.T("Timer resolution: the release was refused"), ok ? AppLog.Sev.Info : AppLog.Sev.Warn);
            return ok;
        }
    }

    readonly HashSet<string> _failureOnce = new();
    readonly Dictionary<uint, ThreadSnap> _snaps = new();
    long _reverts;
    /// <summary>Grows with every revert: layers that cache "already applied" drop their cache.</summary>
    public long Reverts => Interlocked.Read(ref _reverts);

    ThreadSnap LastSnapOf(uint pid) => _snaps.TryGetValue(pid, out var s) ? s : null;

    void Report(ApplyReport rep)
    {
        foreach (var f in rep.Failures)
            // a process Windows does not let us change (protected, system): expected, it is skipped — a note in the log file, not an error window
            if (_failureOnce.Add(rep.Exe + ": " + f)) AppLog.Add(rep.Exe + ": " + f, AppLog.Sev.Warn);
        LastReports.RemoveAll(r => r.Pid == rep.Pid);
        LastReports.Add(rep);
        if (LastReports.Count > 32) LastReports.RemoveAt(0);
    }

    // ---- all targets of one pass (section 10) -------------------------------------------------------

    public sealed class Target
    {
        public uint Pid;
        public Preset Preset;
        public ApplyOptions Options;
    }

    /// <summary>
    /// One toolhelp snapshot and one pid → name cache for the whole pass; the caller picks the
    /// targets from that snapshot (watchlist presets, then system presets), each goes through
    /// Apply with the per-game mask rules; the caches are dropped in the end whatever happens.
    /// </summary>
    public List<ApplyReport> ApplyAll(Func<Dictionary<uint, (string Name, uint Parent)>, Dictionary<uint, List<uint>>, IEnumerable<Target>> pick,
        Dictionary<string, string> affRules = null)
    {
        var reports = new List<ApplyReport>();
        bool own;
        lock (_gate)
        {
            own = _tidCache == null;
            if (own)
            {
                _tidCache = _n.ThreadsByPid();
                FillPidCache();
            }
        }
        try
        {
            IEnumerable<Target> targets;
            lock (_gate) targets = pick(_pidCache, _tidCache)?.ToList() ?? new List<Target>();
            foreach (var t in targets)
                reports.Add(Apply(t.Pid, t.Preset, t.Options, t.Options.DoAffRules ? affRules : null));
        }
        finally
        {
            if (own) EndPass();
        }
        return reports;
    }

    /// <summary>What a process has now (read for the Processes page). Denied processes are not opened.</summary>
    public sealed class ProcInfo
    {
        public bool Ok, Denied, Changed, BoostOff;
        public uint Class;
        public ulong Affinity;
        public int Level;
    }

    public ProcInfo ReadProcess(uint pid, string exe)
    {
        lock (_gate)
        {
            var info = new ProcInfo { Level = -1 };
            if (GateHook?.Invoke(pid, exe) != null)
            {
                info.Denied = true;
                return info;
            }
            IntPtr ph = _n.OpenProcessGraceful(pid, out int level);
            info.Level = level;
            if (level < 0) return info;
            try
            {
                info.Ok = _n.GetPriorityClass(ph, out info.Class);
                _n.GetAffinity(ph, out info.Affinity);
                _n.GetBoostDisabled(ph, out info.BoostOff);
                info.Changed = _procBefore.ContainsKey(pid);
            }
            finally { _n.Close(ph); }
            return info;
        }
    }

    /// <summary>Process ids with a saved baseline (changed by the program).</summary>
    public HashSet<uint> ChangedPids()
    {
        lock (_gate) return new HashSet<uint>(_procBefore.Keys.Concat(_thrBefore.Keys.Select(k => uint.Parse(k[..k.IndexOf(':')], CultureInfo.InvariantCulture))));
    }

    /// <summary>Removes the ideal-processor pins of a process (they stay as hints in Windows; the allocator forgets them).</summary>
    public void DropPins(uint pid)
    {
        lock (_gate)
        {
            string prefix = pid.ToString(CultureInfo.InvariantCulture) + ":";
            foreach (var key in _thrSets.Keys.Where(k => k.StartsWith(prefix, StringComparison.Ordinal)).ToList())
            {
                _thrSets.Remove(key);
                if (!_thrBefore.TryGetValue(key, out var tb) || !tb.SetsSet) continue;
                IntPtr th = _n.OpenThreadOf(pid, uint.Parse(key[prefix.Length..], CultureInfo.InvariantCulture));
                if (th == IntPtr.Zero) continue;
                try
                {
                    _n.ThreadTimes(th, out long born, out _);
                    if (born == tb.Born) Set(_n.SetThreadCpuSets(th, tb.Sets));
                }
                finally { _n.Close(th); }
            }
            RemovePrefix(_ideal, prefix);
        }
    }

    public IReadOnlyDictionary<string, uint> Ideals
    {
        get { lock (_gate) return new Dictionary<string, uint>(_ideal); }
    }

    // ---- manual API (section 21): escape hatch for debugging, never called by Auto ----------------

    public int ManualCalls;

    /// <summary>Sets a thread's priority, ideal processor and boost by hand (baseline kept, birth checked).</summary>
    public string ThreadManual(int pid, uint tid, string prio, string ideal, string boost)
    {
        lock (_gate)
        {
            ManualCalls++;
            uint upid = (uint)pid;
            string denied = GateHook?.Invoke(upid, NameOf(upid));
            if (denied != null) return denied;
            var known = LastSnapOf(upid)?.Rows.FirstOrDefault(r => r.Tid == tid);
            if (known != null && IsDeniedModule(known)) return L.F("Thread {0} belongs to a denylisted module: not changed", tid);
            IntPtr th = _n.OpenThreadOf(upid, tid);
            if (th == IntPtr.Zero) return L.F("Thread {0} of PID {1} cannot be opened", tid, pid);
            try
            {
                string key = Key(upid, tid);
                _n.ThreadTimes(th, out long born, out _);
                if (_thrBefore.TryGetValue(key, out var old) && old.Born != born) _thrBefore.Remove(key);
                ThreadBaseline(th, key, born);
                var done = new List<string>();
                if (!string.IsNullOrEmpty(prio))
                {
                    if (!(TPRIO.TryGetValue(prio, out int p) || int.TryParse(prio, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out p) && TPRIO.ContainsValue(p)) || p == 15)
                        return L.F("Priority {0} is not allowed", prio);
                    if (!_n.GetThreadPrio(th, out int cur) || cur != p)
                        done.Add(Set(_n.SetThreadPrio(th, p)) ? L.F("priority {0}", p) : L.F("priority: error {0}", _n.LastError()));
                }
                if (boost == "on" || boost == "off")
                {
                    bool dis = boost == "off";
                    if (!_n.GetThreadBoostDisabled(th, out bool cur) || cur != dis)
                        done.Add(Set(_n.SetThreadBoostDisabled(th, dis)) ? L.F("boost {0}", boost) : L.F("boost: error {0}", _n.LastError()));
                }
                if (!string.IsNullOrEmpty(ideal))
                {
                    var t = TopoBuilder.Live();
                    if (!int.TryParse(ideal, NumberStyles.None, CultureInfo.InvariantCulture, out int n) || t == null || n >= t.LogicalCount)
                        return L.F("Ideal processor {0} is not valid", ideal);
                    if (!_ideal.TryGetValue(key, out uint o) || o != (uint)n)
                    {
                        _ideal[key] = (uint)n;
                        done.Add(Set(_n.SetThreadIdeal(th, (uint)n)) ? L.F("ideal processor {0}", n) : L.F("ideal: error {0}", _n.LastError()));
                    }
                }
                return done.Count == 0 ? L.F("Thread {0}: nothing to change", tid) : L.F("Thread {0}: {1}", tid, string.Join(", ", done));
            }
            finally { _n.Close(th); }
        }
    }

    bool ManualProcess(int pid, List<string> log, Func<IntPtr, string> act)
    {
        lock (_gate)
        {
            ManualCalls++;
            uint upid = (uint)pid;
            string exe = NameOf(upid);
            string denied = GateHook?.Invoke(upid, exe);
            if (denied != null)
            {
                log?.Add(denied);
                return false;
            }
            IntPtr ph = _n.OpenProcessGraceful(upid, out int level);
            if (level < 0)
            {
                log?.Add(L.F("PID {0} cannot be opened", pid));
                return false;
            }
            try
            {
                string why = _n.IsProtected(ph) ? L.F("{0} is a protected process", exe) : null;
                if (why != null)
                {
                    log?.Add(why);
                    return false;
                }
                long created = _n.ProcessCreated(ph);
                if (_born.TryGetValue(upid, out long b) && b != created) Forget(upid);
                _born[upid] = created;
                bool fresh = !_procBefore.ContainsKey(upid);
                Baseline(upid, ph, created, exe, false);
                if (fresh) BeforeMutate?.Invoke();
                string err = act(ph);
                log?.Add(err ?? L.F("{0}: done", exe));
                return err == null;
            }
            finally { _n.Close(ph); }
        }
    }

    /// <summary>Sets a process' priority class by hand (never realtime).</summary>
    public bool ProcessClassManual(int pid, string cls, List<string> log) => ManualProcess(pid, log, ph =>
    {
        if (!PRIO.TryGetValue(cls ?? "", out uint want) || want == PRIO["realtime"]) return L.F("Class {0} is not allowed", cls);
        if (_n.GetPriorityClass(ph, out uint cur) && cur == want) return null;
        return Set(_n.SetPriorityClass(ph, want)) ? null : L.F("Class: error {0}", _n.LastError());
    });

    /// <summary>Turns a process' dynamic priority boost off (or back on) by hand.</summary>
    public bool ProcessBoost(int pid, bool off, List<string> log) => ManualProcess(pid, log, ph =>
    {
        if (_n.GetBoostDisabled(ph, out bool cur) && cur == off) return null;
        return Set(_n.SetBoostDisabled(ph, off)) ? null : L.F("Boost: error {0}", _n.LastError());
    });

    // ---- background -------------------------------------------------------------------------------

    /// <summary>
    /// Lowers background programs (section 12.8): baselines of all of them first (then the
    /// session is saved), then BelowNormal (only lowering), I/O low (never "very low": that starves a program's disk
    /// reads behind the game's), memory low, EcoQoS on
    /// and, when the mask does not mix with the reserved processors, the background mask.
    /// Returns how many processes were changed.
    /// </summary>
    public int ApplyBackground(IEnumerable<uint> pids, ulong bgMask, bool doAffinity, IEnumerable<uint> timerOnly = null)
    {
        lock (_gate)
        {
            if (!_debug) _debug = _n.EnableDebugPrivilege();
            ulong res = Reservation.Reserved();
            ulong bg = bgMask != 0 ? Reservation.Unmix(bgMask, res, true) : 0;
            bool affOk = doAffinity && bg != 0 && !Reservation.Mixes(bg, res);
            var open = new List<(uint pid, IntPtr h, int level)>();
            try
            {
                bool added = false;
                var names = _pidCache ?? _n.Processes();
                foreach (uint pid in pids)
                {
                    string exe = names.TryGetValue(pid, out var np) ? np.Name : "";
                    IntPtr h = IntPtr.Zero;
                    try
                    {
                        if (BackgroundDeny.Contains(exe) || GateHook?.Invoke(pid, exe) != null || Guard.IsAntiCheatProcess(exe)) continue;
                        h = _n.OpenProcessGraceful(pid, out int level);
                        if (level < 0) continue;
                        if (_n.IsProtected(h)) { _n.Close(h); continue; }
                        long created = _n.ProcessCreated(h);
                        if (_born.TryGetValue(pid, out long b) && b != created) Forget(pid);
                        _born[pid] = created;
                        if (!_procBefore.ContainsKey(pid)) added = true;
                        Baseline(pid, h, created, exe, true);
                        open.Add((pid, h, level));
                    }
                    catch (Exception ex)
                    {
                        // One odd process must never stop the background layer (or Auto with it).
                        if (h != IntPtr.Zero && !open.Any(o => o.h == h)) _n.Close(h);
                        BgSkip(exe, ex);
                    }
                }
                if (added) BeforeMutate?.Invoke();
                int changed = 0;
                uint below = PRIO["below"];
                foreach (var (pid, h, level) in open)
                try
                {
                    long before = SetCalls;
                    if (_n.GetPriorityClass(h, out uint cls) && (cls == PRIO["normal"] || cls == PRIO["above"] || cls == PRIO["high"]))
                        Set(_n.SetPriorityClass(h, below));
                    if (_n.GetIo(h, out uint io) && io > IOP["low"]) Set(_n.SetIo(h, IOP["low"]));
                    if (_n.GetMem(h, out uint mem) && mem > MEM["low"]) Set(_n.SetMem(h, MEM["low"]));
                    uint want = TimerGuard && !KeepsTimer(names.TryGetValue(pid, out var nm) ? nm.Name : "") ? EcoAndTimer : EcoOnly;
                    if (!_n.GetEco(h, out uint c, out uint s) || c != want || s != want) SetEcoGuarded(h, want);
                    if (affOk && level >= 1 && _n.GetAffinity(h, out ulong aff) && aff != bg) Set(_n.SetAffinity(h, bg));
                    if (SetCalls != before) changed++;
                }
                catch (Exception ex) { BgSkip(names.TryGetValue(pid, out var nx) ? nx.Name : pid.ToString(CultureInfo.InvariantCulture), ex); }
                // Timer guard for processes outside the user's session (services such as RGB and peripheral
                // daemons): only their timer-resolution requests are ignored; priority, I/O, memory, EcoQoS and
                // affinity stay untouched. The baseline keeps their original masks for the revert.
                if (TimerGuard && timerOnly != null && !_timerUnsupported)
                {
                    var extra = new List<IntPtr>();
                    try
                    {
                        bool addedT = false;
                        foreach (uint pid in timerOnly)
                        try
                        {
                            string exe = names.TryGetValue(pid, out var np) ? np.Name : "";
                            if (KeepsTimer(exe) || BackgroundDeny.Contains(exe) || GateHook?.Invoke(pid, exe) != null || Guard.IsAntiCheatProcess(exe)) continue;
                            IntPtr h = _n.OpenProcessGraceful(pid, out int level);
                            if (level < 0) continue;
                            extra.Add(h);
                            if (_n.IsProtected(h) || !_n.GetEco(h, out uint c, out uint st)) continue;
                            if ((c & TimerBit) != 0 && (st & TimerBit) != 0) continue;
                            long created = _n.ProcessCreated(h);
                            if (_born.TryGetValue(pid, out long b) && b != created) Forget(pid);
                            _born[pid] = created;
                            if (!_procBefore.ContainsKey(pid)) { addedT = true; Baseline(pid, h, created, exe, true); }
                            if (addedT) { BeforeMutate?.Invoke(); addedT = false; }
                            long before = SetCalls;
                            SetEcoGuarded(h, c | TimerBit, st | TimerBit);
                            if (SetCalls != before) changed++;
                        }
                        catch (Exception ex) { BgSkip(names.TryGetValue(pid, out var nt) ? nt.Name : pid.ToString(CultureInfo.InvariantCulture), ex); }
                    }
                    finally { foreach (var h in extra) _n.Close(h); }
                }
                return changed;
            }
            finally
            {
                foreach (var (_, h, _) in open) _n.Close(h);
            }
        }
    }

    readonly HashSet<string> _bgSkipLogged = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>A process the background layer could not handle is skipped; the reason is logged once per name.</summary>
    void BgSkip(string exe, Exception ex)
    {
        if (_bgSkipLogged.Add(exe ?? "")) AppLog.Add(L.F("Background: {0} skipped ({1})", exe, AppLog.Describe(ex)), AppLog.Sev.Warn);
    }

    // ---- timer guard ------------------------------------------------------------------------------

    /// <summary>
    /// When on, background processes also get PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION (Windows 11):
    /// Windows stops honouring their requests for a finer system timer (a peripheral service asking for 0.5 ms
    /// keeps the whole system ticking at 2000 Hz). The game's own request is never touched. Reverted with the
    /// rest of the background baseline; on Windows 10 the flag is refused and only EcoQoS is set, as before.
    /// </summary>
    public bool TimerGuard { get; set; } = true;
    const uint TimerBit = 4;                 // PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION
    const uint EcoOnly = 1, EcoAndTimer = 1 | TimerBit;
    bool _timerUnsupported;

    /// <summary>Processes whose timing must not be coarsened: system, audio and voice, streaming, browsers (media).</summary>
    static readonly HashSet<string> TimerKeepers = new(StringComparer.OrdinalIgnoreCase)
    {
        "System", "Registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "svchost.exe",
        "dwm.exe", "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "explorer.exe",
        "Discord.exe", "Vesktop.exe", "ts3client_win64.exe", "TeamSpeak.exe", "mumble.exe", "Spotify.exe",
        "obs64.exe", "obs32.exe", "Streamlabs OBS.exe", "Streamlabs Desktop.exe", "NVIDIA Share.exe", "nvcontainer.exe",
        "chrome.exe", "msedge.exe", "firefox.exe", "opera.exe", "brave.exe", "vivaldi.exe", "msedgewebview2.exe",
        "steam.exe", "steamwebhelper.exe",
    };

    public static bool KeepsTimer(string exe) => TimerKeepers.Contains(exe ?? "") || BackgroundDeny.Contains(exe);

    /// <summary>Writes the power-throttling masks; if Windows refuses the timer flag (Windows 10), retries without it.</summary>
    void SetEcoGuarded(IntPtr h, uint control, uint state = uint.MaxValue)
    {
        if (state == uint.MaxValue) state = control;
        if ((control & TimerBit) != 0 && _timerUnsupported) { control &= ~TimerBit; state &= ~TimerBit; }
        if (_n.SetEco(h, control, state)) { Set(true); return; }
        if ((control & TimerBit) != 0)
        {
            int err = _n.LastError();
            if (err is 87 or 1 or 50)
            {
                _timerUnsupported = true;
                AppLog.Add(L.T("Timer guard: this Windows does not support ignoring timer requests; only EcoQoS is set"), AppLog.Sev.Info);
                Set(_n.SetEco(h, control & ~TimerBit, state & ~TimerBit));
                return;
            }
        }
        Set(false);
    }

    // ---- revert -----------------------------------------------------------------------------------

    /// <summary>Gives a process back what was changed: process values, then the threads whose birth time still matches.</summary>
    public int Revert(uint pid)
    {
        lock (_gate) return RevertLocked(pid);
    }

    /// <summary>
    /// Restores one process and its threads. A value Windows refused to put back (the process still lives) keeps its
    /// baseline: <see cref="Pending"/> stays true, the session is not marked clean and the next revert or the crash
    /// restore of the next start tries again. A process or thread that ended (or whose ID now belongs to another one)
    /// is forgotten: there is nothing left to put back.
    /// </summary>
    int RevertLocked(uint pid)
    {
        Interlocked.Increment(ref _reverts);
        int n = 0;
        bool procLeft = false;
        if (_procBefore.TryGetValue(pid, out var b))
        {
            IntPtr ph = _n.OpenProcessGraceful(pid, out int level);
            if (level >= 0)
                try
                {
                    long created = _n.ProcessCreated(ph);
                    if (created == 0 || created == b.Created)
                    {
                        var (done, failed) = RestoreProcCounted(ph, level, b);
                        n += done;
                        procLeft = failed > 0;
                    }
                }
                finally { _n.Close(ph); }
        }
        string prefix = pid.ToString(CultureInfo.InvariantCulture) + ":";
        var threadsLeft = new List<string>();
        foreach (var (key, tb) in _thrBefore.Where(kv => kv.Key.StartsWith(prefix, StringComparison.Ordinal)).ToList())
        {
            uint tid = uint.Parse(key[prefix.Length..], CultureInfo.InvariantCulture);
            IntPtr th = _n.OpenThreadOf(pid, tid);
            if (th == IntPtr.Zero) continue;
            try
            {
                _n.ThreadTimes(th, out long born, out _);
                if (born != tb.Born) continue;   // another thread with the same ID: skipped
                int bad = 0;
                if (tb.PrioSet && (!_n.GetThreadPrio(th, out int cur) || cur != tb.Prio)) { if (Set(_n.SetThreadPrio(th, tb.Prio))) n++; else bad++; }
                if (tb.BoostSet && (!_n.GetThreadBoostDisabled(th, out bool cb) || cb != tb.BoostDisabled)) { if (Set(_n.SetThreadBoostDisabled(th, tb.BoostDisabled))) n++; else bad++; }
                if (tb.SetsSet && _thrSets.ContainsKey(key)) { if (Set(_n.SetThreadCpuSets(th, tb.Sets))) n++; else bad++; }
                if (tb.IdealSet && _ideal.ContainsKey(key)) { if (Set(_n.SetThreadIdeal(th, tb.Ideal))) n++; else bad++; }
                if (bad > 0) threadsLeft.Add(key);
            }
            finally { _n.Close(th); }
        }
        if (!procLeft && threadsLeft.Count == 0) { Forget(pid); return n; }
        // Something could not be put back: its baseline stays (only what was restored or ended is dropped).
        var keepProc = procLeft && b != null ? b : null;
        var keepThreads = threadsLeft.ToDictionary(k => k, k => _thrBefore[k]);
        var keepIdeal = threadsLeft.Where(_ideal.ContainsKey).ToDictionary(k => k, k => _ideal[k]);
        var keepSets = threadsLeft.Where(_thrSets.ContainsKey).ToDictionary(k => k, k => _thrSets[k]);
        string name = _procName.GetValueOrDefault(pid, "");
        bool bg = _bgPids.Contains(pid);
        Forget(pid);
        if (keepProc != null) { _procBefore[pid] = keepProc; _procName[pid] = name; if (bg) _bgPids.Add(pid); }
        foreach (var (k, tb) in keepThreads) _thrBefore[k] = tb;
        foreach (var (k, v) in keepIdeal) _ideal[k] = v;
        foreach (var (k, v) in keepSets) _thrSets[k] = v;
        AppLog.Add(L.F("Revert: some values of {0} (PID {1}) could not be put back; they are tried again later", name.Length > 0 ? name : "?", pid), AppLog.Sev.Warn);
        return n;
    }

    int RestoreProc(IntPtr ph, int level, ProcBefore b) => RestoreProcCounted(ph, level, b).Done;

    (int Done, int Failed) RestoreProcCounted(IntPtr ph, int level, ProcBefore b)
    {
        int n = 0, bad = 0;
        void One(bool needed, Func<bool> write) { if (!needed) return; if (Set(write())) n++; else bad++; }
        One(b.SetsSet && (!_n.GetCpuSets(ph, out uint[] cur) || IdsKey(cur) != IdsKey(b.Sets)), () => _n.SetCpuSets(ph, b.Sets));
        One(b.AffSet && level >= 1 && (!_n.GetAffinity(ph, out ulong a) || a != b.Aff), () => _n.SetAffinity(ph, b.Aff));
        One(b.PrioSet && (!_n.GetPriorityClass(ph, out uint p) || p != b.Prio), () => _n.SetPriorityClass(ph, b.Prio));
        One(b.IoSet && (!_n.GetIo(ph, out uint io) || io != b.Io), () => _n.SetIo(ph, b.Io));
        One(b.MemSet && (!_n.GetMem(ph, out uint m) || m != b.Mem), () => _n.SetMem(ph, b.Mem));
        One(b.EcoSet && (!_n.GetEco(ph, out uint c, out uint s) || c != b.EcoCtl || s != b.EcoState), () => _n.SetEco(ph, b.EcoCtl, b.EcoState));
        One(b.BoostSet && (!_n.GetBoostDisabled(ph, out bool d) || d != b.BoostDisabled), () => _n.SetBoostDisabled(ph, b.BoostDisabled));
        return (n, bad);
    }

    /// <summary>Baselines are left (a value could not be put back) or the timer request is still held.</summary>
    public bool Pending
    {
        get { lock (_gate) return _procBefore.Count > 0 || _thrBefore.Count > 0 || _timerRequested; }
    }

    /// <summary>Reverts every process (game and background); one process failing does not stop the others. Idempotent.</summary>
    public int RevertAll()
    {
        lock (_gate)
        {
            int n = 0;
            var pids = _procBefore.Keys.Concat(_thrBefore.Keys.Select(k => uint.Parse(k[..k.IndexOf(':')], CultureInfo.InvariantCulture))).Distinct().ToList();
            foreach (uint pid in pids)
            {
                try { n += RevertLocked(pid); }
                catch (Exception e) { AppLog.Add(L.F("Revert of PID {0}: {1}", pid, e.Message), AppLog.Sev.Warn); }
            }
            return n;
        }
    }

    public int RevertBackground()
    {
        lock (_gate)
        {
            int n = 0;
            foreach (uint pid in _bgPids.ToList())
            {
                try { n += RevertLocked(pid); }
                catch (Exception e) { AppLog.Add(L.F("Revert of PID {0}: {1}", pid, e.Message), AppLog.Sev.Warn); }
            }
            return n;
        }
    }

    public bool IsBackground(uint pid)
    {
        lock (_gate) return _bgPids.Contains(pid);
    }

    public int BackgroundCount
    {
        get { lock (_gate) return _bgPids.Count; }
    }

    /// <summary>Forgets ended processes, ended threads of living processes, and thread data older than 5 minutes.</summary>
    public void Prune(Dictionary<uint, List<uint>> live = null)
    {
        lock (_gate)
        {
            live ??= _n.ThreadsByPid();
            foreach (uint pid in _procBefore.Keys.Concat(_born.Keys).Concat(_modCache.Keys).Distinct().Where(p => !live.ContainsKey(p)).ToList())
                Forget(pid);
            bool Alive(string key)
            {
                int i = key.IndexOf(':');
                uint pid = uint.Parse(key[..i], CultureInfo.InvariantCulture), tid = uint.Parse(key[(i + 1)..], CultureInfo.InvariantCulture);
                return live.TryGetValue(pid, out var tids) && (tid == 0 || tids.Contains(tid));
            }
            DateTime cut = Now() - TimeSpan.FromMinutes(5);
            foreach (var k in _thrBefore.Keys.Where(k => !Alive(k)).ToList()) _thrBefore.Remove(k);
            foreach (var k in _ideal.Keys.Where(k => !Alive(k)).ToList()) _ideal.Remove(k);
            foreach (var k in _thrSets.Keys.Where(k => !Alive(k)).ToList()) _thrSets.Remove(k);
            foreach (var k in _presetTidCount.Keys.Where(k => !Alive(k)).ToList()) _presetTidCount.Remove(k);
            foreach (var (k, v) in _thrPrev.ToList()) if (!Alive(k) || v.T < cut) _thrPrev.Remove(k);
            foreach (var (k, v) in _thrMeta.ToList()) if (!Alive(k) || v.Seen < cut) _thrMeta.Remove(k);
            if (Advice.Count > 200) Advice.RemoveRange(0, Advice.Count - 200);
        }
    }

    /// <summary>Sizes of the per-thread maps (leak checks).</summary>
    public (int Ideal, int Meta, int Prev, int Before) Sizes()
    {
        lock (_gate) return (_ideal.Count, _thrMeta.Count, _thrPrev.Count, _thrBefore.Count);
    }

    // ---- crash-safe session -----------------------------------------------------------------------

    public SessionData Export()
    {
        lock (_gate)
        {
            var s = new SessionData { TimerRequested = _timerRequested };
            foreach (var (pid, b) in _procBefore)
                s.Procs.Add(new SessionProc
                {
                    Pid = pid, Name = _procName.GetValueOrDefault(pid, ""), Created = b.Created, Background = _bgPids.Contains(pid),
                    Prio = b.Prio, PrioSet = b.PrioSet, Aff = b.Aff, AffSet = b.AffSet, Sets = b.Sets, SetsSet = b.SetsSet,
                    Io = b.Io, IoSet = b.IoSet, Mem = b.Mem, MemSet = b.MemSet, EcoCtl = b.EcoCtl, EcoState = b.EcoState, EcoSet = b.EcoSet,
                    BoostDisabled = b.BoostDisabled, BoostSet = b.BoostSet,
                });
            foreach (var (key, tb) in _thrBefore)
            {
                int i = key.IndexOf(':');
                s.Threads.Add(new SessionThread
                {
                    Pid = uint.Parse(key[..i], CultureInfo.InvariantCulture), Tid = uint.Parse(key[(i + 1)..], CultureInfo.InvariantCulture),
                    Born = tb.Born, Prio = tb.Prio, PrioSet = tb.PrioSet, BoostDisabled = tb.BoostDisabled, BoostSet = tb.BoostSet,
                    Sets = tb.Sets, SetsSet = tb.SetsSet, Ideal = tb.Ideal, IdealSet = tb.IdealSet && _ideal.ContainsKey(key),
                });
            }
            return s;
        }
    }

    /// <summary>
    /// The values a crash restore may write (pure): the ones the program itself could have saved as a baseline. A
    /// priority class of realtime, a time-critical thread priority, the critical I/O priority, an empty affinity or a
    /// processor number past the last one are refused (the session file is never trusted further than that).
    /// </summary>
    public static bool SessionProcValid(SessionProc p) =>
        p != null && (!p.PrioSet || (PRIO.ContainsValue(p.Prio) && p.Prio != PRIO["realtime"]))
        && (!p.AffSet || p.Aff != 0) && (!p.IoSet || p.Io <= 3) && (!p.MemSet || p.Mem is >= 1 and <= 5)
        && (!p.SetsSet || (p.Sets ?? Array.Empty<uint>()).Length <= 1024);

    /// <summary>The ideal processor is a number inside the thread's processor group (0–63).</summary>
    public static bool SessionThreadValid(SessionThread t) =>
        t != null && (!t.PrioSet || (TPRIO.ContainsValue(t.Prio) && t.Prio != TPRIO["critical"]))
        && (!t.IdealSet || t.Ideal < 64) && (!t.SetsSet || (t.Sets ?? Array.Empty<uint>()).Length <= 1024);

    /// <summary>
    /// Restores a session left by a crash: only processes and threads whose creation times still match, only values
    /// that pass <see cref="SessionProcValid"/> / <see cref="SessionThreadValid"/>, and only processes the guard lets
    /// the program touch in normal operation (the name must still be the one saved; anti-cheat and protected ones are skipped).
    /// </summary>
    public int RestoreSession(SessionData s)
    {
        lock (_gate)
        {
            int n = 0;
            if (s == null) return 0;
            var procs = _n.Processes();
            string Refused(uint pid, string savedName)
            {
                string now = procs.TryGetValue(pid, out var pr) ? pr.Name : "";
                if (now.Length == 0) return "gone";
                if (!string.IsNullOrEmpty(savedName) && !now.Equals(savedName, StringComparison.OrdinalIgnoreCase)) return "renamed";
                return GateHook?.Invoke(pid, now);
            }
            var names = (s.Procs ?? new()).Where(p => p != null).GroupBy(p => p.Pid).ToDictionary(g => g.Key, g => g.First().Name);
            // Section 22.5: a timer request left by a crashed run is released before anything else.
            if (s.TimerRequested && _n.TimerQuery(out _, out uint finest, out _) && _n.TimerSet(finest, false, out _))
            {
                n++;
                AppLog.Add(L.T("Timer resolution: the request of the crashed run was released"));
            }
            foreach (var p in s.Procs ?? new())
            {
                if (!SessionProcValid(p)) { AppLog.Add(L.F("Crash restore: the saved values of PID {0} are not ones the program writes; skipped", p?.Pid ?? 0), AppLog.Sev.Warn); continue; }
                if (Refused(p.Pid, p.Name) != null) continue;
                IntPtr ph = _n.OpenProcessGraceful(p.Pid, out int level);
                if (level < 0) continue;
                try
                {
                    if (_n.ProcessCreated(ph) != p.Created) continue;   // someone else's PID now
                    var b = new ProcBefore
                    {
                        Have = true, Created = p.Created, Prio = p.Prio, PrioSet = p.PrioSet, Aff = p.Aff, AffSet = p.AffSet, Sets = p.Sets ?? Array.Empty<uint>(),
                        SetsSet = p.SetsSet, Io = p.Io, IoSet = p.IoSet, Mem = p.Mem, MemSet = p.MemSet, EcoCtl = p.EcoCtl, EcoState = p.EcoState,
                        EcoSet = p.EcoSet, BoostDisabled = p.BoostDisabled, BoostSet = p.BoostSet,
                    };
                    n += RestoreProc(ph, level, b);
                }
                finally { _n.Close(ph); }
            }
            foreach (var t in s.Threads ?? new())
            {
                if (!SessionThreadValid(t)) { AppLog.Add(L.F("Crash restore: the saved values of thread {0} are not ones the program writes; skipped", t?.Tid ?? 0), AppLog.Sev.Warn); continue; }
                if (Refused(t.Pid, names.GetValueOrDefault(t.Pid)) != null) continue;
                IntPtr th = _n.OpenThreadOf(t.Pid, t.Tid);
                if (th == IntPtr.Zero) continue;
                try
                {
                    _n.ThreadTimes(th, out long born, out _);
                    if (born != t.Born) continue;
                    if (t.PrioSet && (!_n.GetThreadPrio(th, out int cur) || cur != t.Prio) && Set(_n.SetThreadPrio(th, t.Prio))) n++;
                    if (t.BoostSet && (!_n.GetThreadBoostDisabled(th, out bool cb) || cb != t.BoostDisabled) && Set(_n.SetThreadBoostDisabled(th, t.BoostDisabled))) n++;
                    if (t.SetsSet && _n.GetThreadCpuSets(th, out uint[] cs) && IdsKey(cs) != IdsKey(t.Sets) && Set(_n.SetThreadCpuSets(th, t.Sets))) n++;
                    if (t.IdealSet && Set(_n.SetThreadIdeal(th, t.Ideal))) n++;
                }
                finally { _n.Close(th); }
            }
            return n;
        }
    }
}
