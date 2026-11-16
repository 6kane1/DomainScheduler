using System.Globalization;

namespace DomainScheduler;

/// <summary>The numbers of the dynamic Top-5 mode. Not settings: nobody can set them wrong.</summary>
internal static class Top5Policy
{
    public const int Slots = 5, HoldTicks = 2, MaxMovesPerTick = 2, ForeignHoldTicks = 3;
    public const int DwellMs = 2000, MetaRefreshMs = 30000;
    public const double Alpha = 0.3;            // smoothing of the load share (about 3 s of memory)
    public const double MinScore = 0.03;        // below this a thread is not worth a slot
    public const double Margin = 0.15;          // how much better a challenger must be
    public const double ForeignBusy = 0.20, ForeignQuiet = 0.10;
    public const double BonusMain = 0.15, BonusRender = 0.10, BonusAudio = 0.08;
}

/// <summary>What one slot holds now (for the page).</summary>
public sealed class Top5Slot
{
    public int Index;
    public uint Tid, LP;
    public int? Prio;                           // null = the thread's own priority
    public double Share, Score;
    public ThreadRole Role;
    public string Name = "";
    public DateTime Since;
}

/// <summary>
/// The decisions of the dynamic Top-5 mode, without any Windows call: five slots (a processor
/// and a priority each), re-ranked every tick. A thread from outside takes the weakest slot when
/// it leads by Margin for HoldTicks ticks; neighbours swap slots the same way; a slot leaves a
/// processor that other work keeps busy. A thread is not moved twice within DwellMs and no more
/// than MaxMovesPerTick moves happen per tick, so the threads migrate but do not thrash.
/// </summary>
public sealed class Top5Plan
{
    public sealed class Cand
    {
        public uint Tid;
        public double Share;
        public ThreadRole Role;
    }

    readonly Dictionary<uint, double> _ema = new();
    readonly Dictionary<uint, ThreadRole> _role = new();
    readonly Dictionary<uint, int> _lead = new(), _low = new();
    readonly Dictionary<uint, DateTime> _moved = new(), _since = new();
    readonly int[] _swapLead = new int[Top5Policy.Slots], _foreignHold = new int[Top5Policy.Slots];
    readonly uint[] _tid = new uint[Top5Policy.Slots];
    readonly uint[] _lp = new uint[Top5Policy.Slots];
    List<uint> _spare = new();
    int _slots;
    public int Moves { get; private set; }
    /// <summary>What happened in the last step, in words (for the page).</summary>
    public readonly List<string> Events = new();

    /// <summary>The processors the slots may use, best first: the first five become the slots, the rest are spares.</summary>
    public void SetCores(IReadOnlyList<uint> lps)
    {
        _slots = Math.Min(Top5Policy.Slots, lps.Count);
        for (int i = 0; i < Top5Policy.Slots; i++) _lp[i] = i < _slots ? lps[i] : 0;
        _spare = lps.Skip(_slots).ToList();
    }

    public int SlotCount => _slots;

    static double Bonus(ThreadRole r) => r switch
    {
        ThreadRole.Main => Top5Policy.BonusMain,
        ThreadRole.Render => Top5Policy.BonusRender,
        ThreadRole.Audio => Top5Policy.BonusAudio,
        _ => 0,
    };

    double Score(uint tid) => _ema.TryGetValue(tid, out double e) ? e + Bonus(_role.GetValueOrDefault(tid, ThreadRole.Aux)) : 0;

    bool Locked(uint tid, DateTime now) => _moved.TryGetValue(tid, out var at) && (now - at).TotalMilliseconds < Top5Policy.DwellMs;

    void Put(int slot, uint tid, DateTime now)
    {
        _tid[slot] = tid;
        if (tid != 0)
        {
            _moved[tid] = now;
            _since[tid] = now;
        }
    }

    /// <summary>One tick: the candidates with their share of the process' load, the foreign load of a processor (0..1).</summary>
    public int Step(DateTime now, IReadOnlyList<Cand> cands, Func<uint, double> foreign)
    {
        Events.Clear();
        int budget = Top5Policy.MaxMovesPerTick, moves = 0;
        var alive = new HashSet<uint>();
        foreach (var c in cands)
        {
            alive.Add(c.Tid);
            _role[c.Tid] = c.Role;
            _ema[c.Tid] = _ema.TryGetValue(c.Tid, out double e) ? e + Top5Policy.Alpha * (c.Share - e) : c.Share;
        }
        foreach (var gone in _ema.Keys.Where(k => !alive.Contains(k)).ToList())
        {
            _ema.Remove(gone);
            _role.Remove(gone);
            _lead.Remove(gone);
            _low.Remove(gone);
            _moved.Remove(gone);
            _since.Remove(gone);
        }

        // Members that ended, may no longer be touched, or went quiet leave their slot.
        for (int i = 0; i < _slots; i++)
        {
            uint t = _tid[i];
            if (t == 0) continue;
            if (!alive.Contains(t))
            {
                _tid[i] = 0;
                Events.Add(L.F("slot {0}: thread {1} is gone", i + 1, t));
                continue;
            }
            if (Score(t) < Top5Policy.MinScore / 2)
            {
                _low[t] = _low.GetValueOrDefault(t) + 1;
                if (_low[t] >= Top5Policy.HoldTicks)
                {
                    _tid[i] = 0;
                    _low.Remove(t);
                    Events.Add(L.F("slot {0}: thread {1} went quiet", i + 1, t));
                }
            }
            else _low.Remove(t);
        }

        var members = new HashSet<uint>(_tid.Where(t => t != 0));
        var outside = alive.Where(t => !members.Contains(t) && Score(t) >= Top5Policy.MinScore).OrderByDescending(Score).ThenBy(t => t).ToList();

        // Free slots are filled at once, the best thread into the best free slot.
        for (int i = 0; i < _slots && budget > 0 && outside.Count > 0; i++)
        {
            if (_tid[i] != 0) continue;
            uint t = outside[0];
            outside.RemoveAt(0);
            Put(i, t, now);
            members.Add(t);
            _lead.Remove(t);
            budget--;
            moves++;
            Events.Add(L.F("slot {0}: thread {1} enters", i + 1, t));
        }

        // A challenger from outside against the weakest member.
        foreach (var k in _lead.Keys.Where(k => outside.Count == 0 || k != outside[0]).ToList()) _lead.Remove(k);
        if (outside.Count > 0 && _tid.Take(_slots).All(t => t != 0))
        {
            uint c = outside[0];
            int weakest = Enumerable.Range(0, _slots).OrderBy(i => Score(_tid[i])).ThenByDescending(i => i).First();
            uint w = _tid[weakest];
            if (Score(c) > Score(w) * (1 + Top5Policy.Margin))
            {
                _lead[c] = _lead.GetValueOrDefault(c) + 1;
                if (_lead[c] >= Top5Policy.HoldTicks && budget > 0 && !Locked(w, now))
                {
                    Put(weakest, c, now);
                    _lead.Remove(c);
                    _since.Remove(w);
                    budget--;
                    moves++;
                    Events.Add(L.F("slot {0}: thread {1} replaces {2}", weakest + 1, c, w));
                }
            }
            else _lead.Remove(c);
        }

        // Order inside the five: a stronger thread moves up one slot at a time.
        for (int i = 0; i + 1 < _slots; i++)
        {
            uint a = _tid[i], b = _tid[i + 1];
            if (b == 0) { _swapLead[i] = 0; continue; }
            bool better = a == 0 || Score(b) > Score(a) * (1 + Top5Policy.Margin);
            _swapLead[i] = better ? _swapLead[i] + 1 : 0;
            if (!better || (a != 0 && _swapLead[i] < Top5Policy.HoldTicks) || budget <= 0 || Locked(b, now) || (a != 0 && Locked(a, now))) continue;
            Put(i, b, now);
            Put(i + 1, a, now);
            _swapLead[i] = 0;
            budget--;
            moves++;
            Events.Add(a == 0 ? L.F("slot {0}: thread {1} moves up", i + 1, b)
                : L.F("slots {0} and {1}: threads {2} and {3} swap", i + 1, i + 2, b, a));
        }

        // A slot leaves a processor that other work keeps busy, for the quietest spare one.
        for (int i = 0; i < _slots && foreign != null; i++)
        {
            if (_tid[i] == 0) { _foreignHold[i] = 0; continue; }
            _foreignHold[i] = foreign(_lp[i]) > Top5Policy.ForeignBusy ? _foreignHold[i] + 1 : 0;
            if (_foreignHold[i] < Top5Policy.ForeignHoldTicks || budget <= 0 || Locked(_tid[i], now)) continue;
            var quiet = _spare.Select(lp => (lp, f: foreign(lp))).Where(x => x.f < Top5Policy.ForeignQuiet).OrderBy(x => x.f).ThenBy(x => x.lp).ToList();
            if (quiet.Count == 0) continue;
            uint old = _lp[i];
            _spare.Remove(quiet[0].lp);
            _spare.Add(old);
            _lp[i] = quiet[0].lp;
            _moved[_tid[i]] = now;
            _foreignHold[i] = 0;
            budget--;
            moves++;
            Events.Add(L.F("slot {0}: processor {1} is busy with other work, moved to {2}", i + 1, old, quiet[0].lp));
        }
        Moves += moves;
        return moves;
    }

    /// <summary>Priority of a slot: the two leading slots +1, the audio thread +2, the others keep their own.</summary>
    int? PrioOf(int slot) => _role.GetValueOrDefault(_tid[slot], ThreadRole.Aux) == ThreadRole.Audio ? 2 : slot < 2 ? 1 : null;

    /// <summary>What should be in force now: slot, thread, processor, priority.</summary>
    public List<Top5Slot> Desired()
    {
        var list = new List<Top5Slot>();
        for (int i = 0; i < _slots; i++)
        {
            if (_tid[i] == 0) continue;
            uint t = _tid[i];
            list.Add(new Top5Slot
            {
                Index = i, Tid = t, LP = _lp[i], Prio = PrioOf(i), Share = _ema.GetValueOrDefault(t), Score = Score(t),
                Role = _role.GetValueOrDefault(t, ThreadRole.Aux), Since = _since.GetValueOrDefault(t),
            });
        }
        return list;
    }
}

/// <summary>
/// Runs the plan on a game: keeps one handle per thread open (a thread is opened once, when it
/// appears), reads the cycle counters every tick, and writes only the differences through the
/// engine (baseline, birth check, revert, crash-safe session). Threads of denylisted modules
/// and threads whose module is not known yet are never candidates.
/// </summary>
public sealed class Top5 : IDisposable
{
    sealed class H
    {
        public IntPtr Handle;
        public long Born;
        public ulong Cycles, Cpu;
        public bool First = true;
    }

    sealed class Meta
    {
        public long Born;
        public ThreadRole Role;
        public string Name = "";
    }

    readonly Engine _e;
    readonly INative _n;
    readonly object _gate = new();
    readonly Dictionary<uint, H> _h = new();
    readonly Dictionary<uint, Meta> _meta = new();
    readonly Dictionary<uint, (uint Lp, int? Prio)> _applied = new();
    readonly LinkedList<string> _log = new();
    readonly HashSet<uint> _known = new();      // every thread the last snapshot saw, candidate or not
    Top5Plan _plan = new();
    List<Top5Slot> _view = new();
    uint _pid;
    long _reverts;
    DateTime _prevAt;
    long[] _idle, _total;
    int _failures;

    public bool Running { get; private set; }
    public string Exe { get; private set; } = "";
    public int Moves => _plan.Moves;
    public int Threads { get { lock (_gate) return _h.Count; } }
    /// <summary>True when threads appeared that are not classified yet.</summary>
    public bool NeedsMeta { get { lock (_gate) return _h.Keys.Any(t => !_known.Contains(t)); } }

    public Top5(Engine engine)
    {
        _e = engine;
        _n = engine.Native;
    }

    public List<Top5Slot> View()
    {
        lock (_gate) return _view.ToList();
    }

    public List<string> Log()
    {
        lock (_gate) return _log.ToList();
    }

    /// <summary>Starts on a game: the slots take the best processors of the game's mask (never processor 0).</summary>
    public bool Start(uint pid, string exe, Topo t, ulong mask)
    {
        lock (_gate)
        {
            StopLocked(false);
            if (t == null || _e.Gate(pid) != null) return false;
            var cores = Masks.IdealCandidates(t, true, mask).Select(c => (uint)c.LP).ToList();
            _plan = new Top5Plan();
            _plan.SetCores(cores);
            if (_plan.SlotCount == 0) return false;
            _pid = pid;
            Exe = exe ?? "";
            _reverts = _e.Reverts;
            _prevAt = default;
            _idle = _total = null;
            _failures = 0;
            _log.Clear();
            Running = true;
            return true;
        }
    }

    /// <summary>Names and roles of the threads from a full snapshot (taken at the start and at most twice a minute).</summary>
    public void SetMeta(ThreadSnap snap)
    {
        if (snap == null) return;
        lock (_gate)
        {
            _meta.Clear();
            _known.Clear();
            foreach (var r in snap.Rows)
            {
                _known.Add(r.Tid);
                // The thread ID was reused: the old handle is dropped, the new thread is opened on the next tick.
                if (_h.TryGetValue(r.Tid, out var stale) && stale.Born != r.Born)
                {
                    _n.Close(stale.Handle);
                    _h.Remove(r.Tid);
                    _applied.Remove(r.Tid);
                }
                if (Engine.IsDeniedModule(r) || string.IsNullOrEmpty(r.Sym) && string.IsNullOrEmpty(r.Desc)) continue;   // never a candidate
                var role = AutoLogic.Classify(r, snap);
                if (role == ThreadRole.Skip) continue;
                _meta[r.Tid] = new Meta { Born = r.Born, Role = role, Name = r.Desc.Length > 0 ? r.Desc : Engine.ModuleOf(r.Sym) };
            }
        }
    }

    /// <summary>Every second: measure, re-rank, move what has to move. Returns the number of moves.</summary>
    public int Tick(DateTime now, IReadOnlyCollection<uint> tids)
    {
        lock (_gate)
        {
            if (!Running || tids == null) return 0;
            if (_e.Reverts != _reverts)
            {
                _applied.Clear();        // the engine put everything back (a revert or the frame-time check)
                _reverts = _e.Reverts;
            }
            // Handles: a thread is opened once; ended threads are closed.
            var live = new HashSet<uint>(tids);
            foreach (uint gone in _h.Keys.Where(k => !live.Contains(k)).ToList())
            {
                _n.Close(_h[gone].Handle);
                _h.Remove(gone);
                _applied.Remove(gone);
            }
            foreach (uint tid in tids)
            {
                if (_h.ContainsKey(tid)) continue;
                IntPtr th = _n.OpenThreadOf(_pid, tid);
                if (th == IntPtr.Zero) continue;
                _n.ThreadTimes(th, out long born, out _);
                _h[tid] = new H { Handle = th, Born = born };
            }
            double dt = _prevAt == default ? 0 : (now - _prevAt).TotalSeconds;
            _prevAt = now;

            // Load of every thread since the last tick.
            var delta = new Dictionary<uint, ulong>();
            var cpuShare = new Dictionary<uint, double>();
            ulong total = 0;
            foreach (var (tid, h) in _h)
            {
                if (!_n.ThreadCycles(h.Handle, out ulong cycles)) continue;
                _n.ThreadTimes(h.Handle, out _, out ulong cpu);
                if (!h.First && cycles >= h.Cycles)
                {
                    delta[tid] = cycles - h.Cycles;
                    total += cycles - h.Cycles;
                    if (dt > 0 && cpu >= h.Cpu) cpuShare[tid] = (cpu - h.Cpu) / 1e7 / dt;
                }
                h.Cycles = cycles;
                h.Cpu = cpu;
                h.First = false;
            }
            if (dt < 0.2 || total == 0) return 0;

            var cands = new List<Top5Plan.Cand>();
            foreach (var (tid, d) in delta)
                if (_meta.TryGetValue(tid, out var m) && m.Born == _h[tid].Born)
                    cands.Add(new Top5Plan.Cand { Tid = tid, Share = (double)d / total, Role = m.Role });

            // Foreign load of a processor: everything on it except the thread the slot holds there.
            Func<uint, double> foreign = null;
            if (_n.CpuTimes(out long[] idle, out long[] tot) && _idle != null && _idle.Length == idle.Length)
            {
                var busy = new double[idle.Length];
                for (int i = 0; i < idle.Length; i++)
                {
                    long dTot = tot[i] - _total[i], dIdle = idle[i] - _idle[i];
                    busy[i] = dTot > 0 ? Math.Clamp(1.0 - (double)dIdle / dTot, 0, 1) : 0;
                }
                var own = _applied.GroupBy(a => a.Value.Lp).ToDictionary(g => g.Key, g => g.Sum(a => cpuShare.GetValueOrDefault(a.Key)));
                foreign = lp => lp < busy.Length ? Math.Max(0, busy[lp] - own.GetValueOrDefault(lp)) : 0;
            }
            _idle = idle;
            _total = tot;

            int moves = _plan.Step(now, cands, foreign);
            foreach (var ev in _plan.Events)
            {
                _log.AddFirst(now.ToLocalTime().ToString("HH:mm:ss", CultureInfo.InvariantCulture) + "  " + ev);
                while (_log.Count > 12) _log.RemoveLast();
            }
            Reconcile();
            return moves;
        }
    }

    /// <summary>Makes Windows match the plan: only the differences are written.</summary>
    void Reconcile()
    {
        var want = _plan.Desired();
        var keep = new HashSet<uint>(want.Select(w => w.Tid));
        foreach (uint tid in _applied.Keys.Where(k => !keep.Contains(k)).ToList())
        {
            if (_h.TryGetValue(tid, out var h)) _e.ReleaseThread(_pid, tid, h.Handle);     // back to its own priority and processor
            _applied.Remove(tid);
        }
        foreach (var w in want)
        {
            if (_meta.TryGetValue(w.Tid, out var m)) w.Name = m.Name;
            if (!_h.TryGetValue(w.Tid, out var h)) continue;
            if (_applied.TryGetValue(w.Tid, out var a) && a.Lp == w.LP && a.Prio == w.Prio) continue;
            if (_e.PlaceThread(_pid, w.Tid, h.Handle, h.Born, w.Prio, w.LP)) _applied[w.Tid] = (w.LP, w.Prio);
            else if (++_failures == 1) AppLog.Add(L.F("Dynamic Top-5: Windows refused a change for thread {0} (error {1})", w.Tid, _n.LastError()), AppLog.Sev.Warn);
        }
        _view = want;
    }

    /// <summary>Stops: with release = true the threads get their own values back here; otherwise the engine's revert does it.</summary>
    public void Stop(bool release)
    {
        lock (_gate) StopLocked(release);
    }

    void StopLocked(bool release)
    {
        if (release)
            foreach (uint tid in _applied.Keys.ToList())
                if (_h.TryGetValue(tid, out var h)) _e.ReleaseThread(_pid, tid, h.Handle);
        foreach (var h in _h.Values) _n.Close(h.Handle);
        _h.Clear();
        _applied.Clear();
        _meta.Clear();
        _known.Clear();
        _view = new();
        Running = false;
    }

    public void Dispose() => Stop(false);
}
