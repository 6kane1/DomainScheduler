namespace DomainScheduler.Tests;

/// <summary>An in-memory Windows for engine tests; counts opens and writes.</summary>
public sealed class FakeNative : INative
{
    public sealed class P
    {
        public string Name = "";
        public long Created;
        public uint Parent;
        public uint Cls = 32;
        public ulong Aff = ulong.MaxValue;
        public uint[] Sets = Array.Empty<uint>();
        public uint Io = 2, Mem = 5, EcoCtl, EcoState;
        public bool BoostDisabled, Protected, Refuse, ReadOnly, Throw;
        public List<ModRange> Mods = new();
    }

    public sealed class T
    {
        public uint Pid;
        public long Born;
        public ulong Cycles;
        public bool NoCycles;
        public ulong Start;
        public string Desc = "";
        public int Prio;
        public bool BoostDisabled;
        public uint Ideal = 999;
        public int IdealSets;
        public bool Refuse, RefuseSets;
        public uint[] Sets = Array.Empty<uint>();
        public int SetsWrites;
    }

    public readonly Dictionary<uint, P> Procs = new();
    public readonly Dictionary<uint, T> Threads = new();
    readonly Dictionary<IntPtr, (bool thread, uint id)> _h = new();
    long _next = 100;
    public int ThreadOpens, ProcessOpens, Writes;
    public readonly List<uint> OpenedPids = new();
    int _err;

    IntPtr NewHandle(bool thread, uint id)
    {
        var h = new IntPtr(++_next);
        _h[h] = (thread, id);
        return h;
    }

    P Pr(IntPtr h) => Procs[_h[h].id];
    T Th(IntPtr h) => Threads[_h[h].id];

    bool W(bool ok, bool readOnly = false)
    {
        Writes++;
        if (readOnly) { _err = 5; return false; }
        return ok;
    }

    public bool EnableDebugPrivilege() => true;

    public IntPtr OpenProcessGraceful(uint pid, out int level)
    {
        level = -1;
        if (!Procs.TryGetValue(pid, out var p) || p.Refuse) return IntPtr.Zero;
        ProcessOpens++;
        OpenedPids.Add(pid);
        level = 2;
        return NewHandle(false, pid);
    }

    public long ProcessCreated(IntPtr process) => Pr(process).Created;
    public bool IsProtected(IntPtr process) => Pr(process).Protected;
    public void Close(IntPtr handle) => _h.Remove(handle);
    public bool GetPriorityClass(IntPtr p, out uint cls) { cls = Pr(p).Cls; return true; }
    public bool SetPriorityClass(IntPtr p, uint cls) { if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).Cls = cls; return true; }
    public bool GetAffinity(IntPtr p, out ulong mask) { mask = Pr(p).Aff; return true; }
    public bool SetAffinity(IntPtr p, ulong mask) { if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).Aff = mask; return true; }
    public bool GetCpuSets(IntPtr p, out uint[] ids) { ids = Pr(p).Sets; return true; }
    public bool SetCpuSets(IntPtr p, uint[] ids) { if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).Sets = ids ?? Array.Empty<uint>(); return true; }
    public bool GetIo(IntPtr p, out uint v) { v = Pr(p).Io; return true; }
    public bool SetIo(IntPtr p, uint v) { if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).Io = v; return true; }
    public bool GetMem(IntPtr p, out uint v) { v = Pr(p).Mem; return true; }
    public bool SetMem(IntPtr p, uint v) { if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).Mem = v; return true; }
    public bool GetEco(IntPtr p, out uint c, out uint s) { if (Pr(p).Throw) throw new System.ComponentModel.Win32Exception(); c = Pr(p).EcoCtl; s = Pr(p).EcoState; return true; }
    public int EcoError;   // 1 = power throttling is off in this Windows
    public bool SetEco(IntPtr p, uint c, uint s) { if (EcoError != 0) { _err = EcoError; return false; } if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).EcoCtl = c; Pr(p).EcoState = s; return true; }
    public bool GetBoostDisabled(IntPtr p, out bool d) { d = Pr(p).BoostDisabled; return true; }
    public bool SetBoostDisabled(IntPtr p, bool d) { if (!W(true, Pr(p).ReadOnly)) return false; Pr(p).BoostDisabled = d; return true; }
    public List<ModRange> Modules(IntPtr p) => Pr(p).Mods;

    public IntPtr OpenThreadOf(uint pid, uint tid)
    {
        if (!Threads.TryGetValue(tid, out var t) || t.Pid != pid || t.Refuse) return IntPtr.Zero;
        ThreadOpens++;
        return NewHandle(true, tid);
    }

    public bool ThreadCycles(IntPtr th, out ulong cycles) { cycles = Th(th).Cycles; return !Th(th).NoCycles; }
    public bool ThreadTimes(IntPtr th, out long created, out ulong ku) { created = Th(th).Born; ku = Th(th).Cycles / 100; return true; }
    public ulong ThreadStart(IntPtr th) => Th(th).Start;
    public string ThreadDesc(IntPtr th) => Th(th).Desc;
    public bool GetThreadPrio(IntPtr th, out int prio) { prio = Th(th).Prio; return true; }
    public bool SetThreadPrio(IntPtr th, int prio) { W(true); Th(th).Prio = prio; return true; }
    public bool GetThreadBoostDisabled(IntPtr th, out bool d) { d = Th(th).BoostDisabled; return true; }
    public bool SetThreadBoostDisabled(IntPtr th, bool d) { W(true); Th(th).BoostDisabled = d; return true; }
    public bool SetThreadIdeal(IntPtr th, uint lp) { W(true); Th(th).Ideal = lp; Th(th).IdealSets++; return true; }

    public bool GetThreadIdeal(IntPtr th, out uint lp) { lp = Th(th).Ideal; return true; }
    public long[] CpuIdle = new long[16], CpuTotal = new long[16];
    public bool CpuTimes(out long[] idle, out long[] total) { idle = (long[])CpuIdle.Clone(); total = (long[])CpuTotal.Clone(); return true; }
    public bool GetThreadCpuSets(IntPtr th, out uint[] ids) { ids = Th(th).Sets; return true; }
    public bool SetThreadCpuSets(IntPtr th, uint[] ids)
    {
        W(true);
        if (Th(th).RefuseSets) { _err = 87; return false; }
        Th(th).Sets = ids ?? Array.Empty<uint>();
        Th(th).SetsWrites++;
        return true;
    }

    public uint TimerCoarsest = 156250, TimerFinest = 5000, TimerCurrent = 156250;
    public bool TimerHeld, TimerFail;
    public int TimerSets, TimerReleases;
    public bool TimerQuery(out uint coarsest, out uint finest, out uint current)
    {
        coarsest = TimerCoarsest; finest = TimerFinest; current = TimerCurrent;
        return !TimerFail;
    }
    public bool TimerSet(uint resolution, bool set, out uint actual)
    {
        actual = TimerCurrent;
        if (TimerFail) return false;
        if (set) { TimerSets++; TimerHeld = true; TimerCurrent = resolution; }
        else { TimerReleases++; TimerHeld = false; TimerCurrent = TimerCoarsest; }
        actual = TimerCurrent;
        return true;
    }

    public Dictionary<uint, List<uint>> ThreadsByPid() =>
        Threads.GroupBy(kv => kv.Value.Pid).Where(g => Procs.ContainsKey(g.Key)).ToDictionary(g => g.Key, g => g.Select(kv => kv.Key).OrderBy(x => x).ToList());

    public int ProcessLists;
    public Dictionary<uint, (string Name, uint Parent)> Processes() { ProcessLists++; return Procs.ToDictionary(kv => kv.Key, kv => (kv.Value.Name, kv.Value.Parent)); }

    public int LastError() => _err;

    /// <summary>A CS2-like game: MainThrd (first TID), render, audio, pool workers, an NVIDIA worker, one anti-cheat thread.</summary>
    public static FakeNative Cs2(uint pid = 500)
    {
        var f = new FakeNative();
        f.Procs[pid] = new P
        {
            Name = "cs2.exe", Created = 77,
            Mods = new()
            {
                new ModRange { Base = 0x1000, Size = 0x1000, Name = "cs2.exe" },
                new ModRange { Base = 0x10000, Size = 0x10000, Name = "tier0.dll" },
                new ModRange { Base = 0x40000, Size = 0x1000, Name = "EasyAntiCheat_EOS.dll" },
                new ModRange { Base = 0x50000, Size = 0x1000, Name = "nvwgf2umx.dll" },
            },
        };
        void Add(uint tid, ulong start, string desc, ulong cycles) => f.Threads[tid] = new T { Pid = pid, Born = tid * 10, Start = start, Desc = desc, Cycles = cycles };
        Add(100, 0x1100, "MainThrd", 0);
        Add(101, 0x10100, "D3D11RenderThread", 0);
        Add(102, 0x10200, "AudioMixer", 0);
        Add(103, 0x10300, "GlobPool/P0", 0);
        Add(104, 0x10300, "GlobPool/P1", 0);
        Add(105, 0x50010, "", 0);
        Add(106, 0x40010, "", 0);
        Add(107, 0x10400, "idle helper", 0);
        return f;
    }

    /// <summary>Advances the cycle counters as if the game ran: per-thread cycles per second.</summary>
    public void Run(Dictionary<uint, ulong> perSecond, double seconds)
    {
        foreach (var (tid, rate) in perSecond)
            if (Threads.TryGetValue(tid, out var t)) t.Cycles += (ulong)(rate * seconds);
    }
}
