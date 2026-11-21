using System.Runtime.InteropServices;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// Burst guard (1.16.0 Test 1.10, test builds only). The idea of an owner's "low-latency module", rewritten so it can
/// only help and acts only where it measures a real problem:
///  1. CPU: while a game is handled, background programs of the user's session are sampled every 2 s. One that keeps
///     using more than <see cref="BurstPolicy.HeavyCores"/> of a logical processor for 3 samples in a row gets a hard
///     CPU cap of <see cref="BurstPolicy.CapCores"/> processor through a Job Object of its own. The job lets child
///     processes break away silently, so a game started later from a capped launcher is never capped. The cap is lifted
///     at once when the program comes to the front, and for all of them when the game ends, Auto stops or Toolbox
///     exits; the job names are saved, so after a crash the next start lifts them (<see cref="RecoverAfterCrash"/>).
///  2. Memory: only when free memory is low (under 10% of RAM or 1.5 GB) the game's current working set becomes its
///     hard minimum (at most 25% of RAM), so Windows does not trim the pages the game already uses. Lifted with the game.
/// It never touches services, other sessions, the shell, launchers, audio / voice / streaming / VR, network boosters,
/// security software or anti-cheats (the same deny lists as the background layer, plus <see cref="BurstPolicy.Never"/>),
/// and the game itself only for the working set and only when Auto may touch the game (not under FACEIT and the like).
/// On a clean PC it does nothing at all: that is the point.
/// </summary>
public static class BurstPolicy
{
    /// <summary>A program is "heavy" above this share of one logical processor (averaged over a sample).</summary>
    public const double HeavyCores = 0.30;
    /// <summary>The cap a heavy program gets, in logical processors.</summary>
    public const double CapCores = 0.50;
    /// <summary>Samples in a row above <see cref="HeavyCores"/> before the cap.</summary>
    public const int HeavySamples = 3;
    public const int SampleSeconds = 2;
    /// <summary>Soft mode: the job's scheduling weight (1..9, 5 = what everyone else has; 1 = the smallest share).</summary>
    public const int SoftWeight = 1;

    /// <summary>Programs never capped even when heavy: network boosters carry the game's traffic.</summary>
    public static readonly HashSet<string> Never = new(StringComparer.OrdinalIgnoreCase)
    {
        "ExitLag.exe", "GearUP_Booster.exe", "GearUP Booster.exe", "NoPing.exe", "WTFast.exe", "Mudfish.exe",
        "MsMpEng.exe", "NisSrv.exe", "MpDefenderCoreService.exe", "SecurityHealthService.exe", "SecurityHealthSystray.exe",
        "smartscreen.exe", "dwm.exe", "explorer.exe", "csrss.exe", "ctfmon.exe", "TextInputHost.exe",
    };

    /// <summary>Processors used during a sample: CPU time (100 ns units) over wall time.</summary>
    public static double Cores(long cpuDelta100ns, double seconds) => seconds <= 0 ? 0 : cpuDelta100ns / 1e7 / seconds;

    /// <summary>JOBOBJECT_CPU_RATE_CONTROL_INFORMATION.CpuRate: share of the whole machine in 1/100 of a percent.</summary>
    public static uint CpuRate(double cores, int logical) =>
        (uint)Math.Clamp(Math.Round(cores / Math.Max(1, logical) * 10000.0), 1, 10000);

    /// <summary>Low free memory: under 10% of RAM or under 1.5 GB, whichever is larger.</summary>
    public static bool MemoryPressure(ulong availBytes, ulong totalBytes) =>
        totalBytes > 0 && availBytes < Math.Max(totalBytes / 10, 1536UL << 20);

    /// <summary>The hard working-set minimum for the game: what it uses now, at most 25% of RAM; 0 = do not set.</summary>
    public static ulong GameMinimum(ulong workingSet, ulong totalBytes)
    {
        ulong cap = totalBytes / 4;
        ulong want = Math.Min(workingSet, cap);
        return want < (256UL << 20) ? 0 : want;
    }

    /// <summary>One sample of one program: the new count of heavy samples in a row.</summary>
    public static int NextStreak(int streak, double cores) => cores >= HeavyCores ? streak + 1 : 0;
}

/// <summary>The Windows calls of the burst guard (a fake in the tests).</summary>
public interface IBurstNative
{
    bool CpuTime(uint pid, out long created, out long cpu100ns);
    /// <summary>
    /// Creates the named job, lets children break away, sets the limit and puts the process in it. weight = 0: a hard
    /// cap of <paramref name="cpuRate"/>; weight 1..9: weight-based sharing (1 = the smallest share), never a stop.
    /// </summary>
    bool Cap(uint pid, string jobName, uint cpuRate, int weight, out IntPtr job, out int error);
    /// <summary>Lifts the cap (the process stays in the job until it exits; a job without limits does nothing).</summary>
    void Lift(IntPtr job);
    /// <summary>Opens a job left by a crashed Toolbox and lifts its cap; false when it no longer exists.</summary>
    bool LiftByName(string jobName);
    void Close(IntPtr job);
    bool Memory(out ulong avail, out ulong total);
    bool WorkingSet(uint pid, out ulong ws, out ulong min, out ulong max);
    bool SetWorkingSetMin(uint pid, ulong min, ulong max, bool hard);
}

public sealed class BurstGuard
{
    sealed class Watch { public long Created; public long Cpu; public DateTime At; public int Streak; public double Peak; public string Name = ""; }
    sealed class Capped { public long Created; public string Name = ""; public string Job = ""; public IntPtr Handle; public double Peak; }
    sealed record SavedCap(uint Pid, long Created, string Job);
    sealed record SavedState(List<SavedCap> Caps);

    readonly IBurstNative _n;
    readonly Func<DateTime> _now;
    readonly object _gate = new();
    readonly Dictionary<uint, Watch> _watch = new();
    readonly Dictionary<uint, Capped> _capped = new();
    readonly List<(string Name, double Peak)> _history = new();
    DateTime _last;
    uint _game; long _gameCreated; bool _wsSet; ulong _wsMin0, _wsMax0;
    bool _pressureSeen;

    /// <summary>The switch (setting BurstGuard; on by default since 1.16.0).</summary>
    public bool On;
    /// <summary>
    /// Soft mode (1.16.0 Test 1.11, setting BurstGuardSoft, on by default): instead of a hard cap (which stops the
    /// program until the next accounting interval once its share is used, up to hundreds of ms), the job gets the
    /// smallest scheduling weight: the program is never stopped, it only gives way when the processors are busy.
    /// </summary>
    public bool Soft = true;
    /// <summary>Where the names of open jobs are kept for crash recovery (null: not kept, tests).</summary>
    public string StatePath;
    public int Logical = Environment.ProcessorCount;
    public Action<string, AppLog.Sev> Log = (m, s) => AppLog.Add(m, s);

    public BurstGuard(IBurstNative native = null, Func<DateTime> now = null)
    {
        _n = native ?? new WinBurstNative();
        _now = now ?? (() => DateTime.UtcNow);
    }

    public int CappedCount { get { lock (_gate) return _capped.Count; } }

    /// <summary>
    /// One tick of Auto while a game is handled. <paramref name="candidates"/>: background processes of the user's
    /// session that the background layer may change (pid, exe); <paramref name="foreground"/>: the window in front.
    /// </summary>
    public void Tick(uint game, long gameCreated, bool touchGame, IEnumerable<(uint Pid, string Name)> candidates, uint foreground)
    {
        lock (_gate)
        {
            if (!On) { ReleaseLocked(false); return; }
            if (game != _game || gameCreated != _gameCreated) { ReleaseLocked(false); _game = game; _gameCreated = gameCreated; }
            DateTime now = _now();
            if (_last != default && (now - _last).TotalSeconds < BurstPolicy.SampleSeconds) return;
            _last = now;

            // A capped program that came to the front, or that ended, is released.
            foreach (var pid in _capped.Keys.ToList())
            {
                var c = _capped[pid];
                bool alive = _n.CpuTime(pid, out long cr, out _) && cr == c.Created;
                if (!alive || pid == foreground)
                {
                    _n.Lift(c.Handle); _n.Close(c.Handle); _capped.Remove(pid);
                    if (alive) Log(L.F("Burst guard: {0} is in front, its CPU cap is lifted", c.Name), AppLog.Sev.Info);
                }
            }

            var seen = new HashSet<uint>();
            foreach (var (pid, name) in candidates)
            {
                if (pid == game || pid == foreground || BurstPolicy.Never.Contains(name)) continue;
                if (!_n.CpuTime(pid, out long created, out long cpu)) continue;
                seen.Add(pid);
                if (!_watch.TryGetValue(pid, out var w) || w.Created != created)
                {
                    _watch[pid] = new Watch { Created = created, Cpu = cpu, At = now, Name = name };
                    continue;
                }
                double cores = BurstPolicy.Cores(cpu - w.Cpu, (now - w.At).TotalSeconds);
                w.Cpu = cpu; w.At = now; w.Peak = Math.Max(w.Peak, cores);
                w.Streak = BurstPolicy.NextStreak(w.Streak, cores);
                if (w.Streak >= BurstPolicy.HeavySamples && !_capped.ContainsKey(pid))
                {
                    string job = $"DomainOS.BurstGuard.{pid}.{created}";
                    uint rate = BurstPolicy.CpuRate(BurstPolicy.CapCores, Logical);
                    if (_n.Cap(pid, job, rate, Soft ? BurstPolicy.SoftWeight : 0, out IntPtr h, out int err))
                    {
                        _capped[pid] = new Capped { Created = created, Name = name, Job = job, Handle = h, Peak = w.Peak };
                        Log(Soft ? L.F("Burst guard: {0} used {1:0.0} processors in the background during the game; it now gives way to the game (lowest share)", name, w.Peak)
                                 : L.F("Burst guard: {0} used {1:0.0} processors in the background during the game; capped at {2:0.0}", name, w.Peak, BurstPolicy.CapCores), AppLog.Sev.Info);
                        Save();
                    }
                    else
                    {
                        w.Streak = int.MinValue / 2;   // refused (protected, or a job that forbids it): not tried again
                        Log(L.F("Burst guard: {0} could not be capped (error {1})", name, err), AppLog.Sev.Info);
                    }
                }
                if (_capped.TryGetValue(pid, out var cp)) cp.Peak = Math.Max(cp.Peak, cores);
            }
            foreach (var pid in _watch.Keys.Where(p => !seen.Contains(p)).ToList()) _watch.Remove(pid);

            // Memory: the game's current working set becomes a hard minimum only under memory pressure.
            if (touchGame && !_wsSet && _n.Memory(out ulong avail, out ulong total) && BurstPolicy.MemoryPressure(avail, total))
            {
                _pressureSeen = true;
                if (_n.WorkingSet(game, out ulong ws, out ulong min0, out ulong max0))
                {
                    ulong min = BurstPolicy.GameMinimum(ws, total);
                    if (min > 0 && _n.SetWorkingSetMin(game, min, Math.Max(max0, min + (256UL << 20)), hard: true))
                    {
                        _wsSet = true; _wsMin0 = min0; _wsMax0 = max0;
                        Log(L.F("Burst guard: little free memory ({0} MB); the game keeps its {1} MB in memory", avail >> 20, min >> 20), AppLog.Sev.Info);
                    }
                }
            }
        }
    }

    /// <summary>The game ended, Auto stopped or Toolbox exits: every cap is lifted, the game's minimum is put back.</summary>
    public void Release() { lock (_gate) ReleaseLocked(true); }

    void ReleaseLocked(bool summary)
    {
        foreach (var c in _capped.Values) { _n.Lift(c.Handle); _n.Close(c.Handle); _history.Add((c.Name, c.Peak)); }
        bool had = _capped.Count > 0;
        _capped.Clear();
        if (_wsSet && _game != 0 && _n.CpuTime(_game, out long cr, out _) && cr == _gameCreated)
            _n.SetWorkingSetMin(_game, _wsMin0, _wsMax0, hard: false);
        if (summary && (_history.Count > 0 || _pressureSeen))
        {
            string list = string.Join(", ", _history.GroupBy(h => h.Name, StringComparer.OrdinalIgnoreCase)
                .Select(g => $"{g.Key} ({g.Max(x => x.Peak):0.0})"));
            Log(_history.Count > 0
                ? L.F("Burst guard after the game: capped in the background: {0}; little free memory: {1}", list, _pressureSeen ? L.T("yes") : L.T("no"))
                : L.T("Burst guard after the game: nothing was capped; free memory was low at times"), AppLog.Sev.Info);
        }
        _history.Clear();
        _wsSet = false; _pressureSeen = false;
        _watch.Clear();
        _game = 0; _gameCreated = 0; _last = default;
        if (had || StatePath != null) Save();
    }

    void Save()
    {
        if (StatePath == null) return;
        try
        {
            if (_capped.Count == 0) { if (File.Exists(StatePath)) File.Delete(StatePath); return; }
            var s = new SavedState(_capped.Select(kv => new SavedCap(kv.Key, kv.Value.Created, kv.Value.Job)).ToList());
            File.WriteAllText(StatePath, JsonSerializer.Serialize(s));
        }
        catch (Exception) { }
    }

    /// <summary>At start: caps a crashed Toolbox left behind are lifted (by the saved job names).</summary>
    public int RecoverAfterCrash()
    {
        if (StatePath == null || !File.Exists(StatePath)) return 0;
        int n = 0;
        try
        {
            var s = JsonSerializer.Deserialize<SavedState>(File.ReadAllText(StatePath));
            foreach (var c in s?.Caps ?? new List<SavedCap>())
                if (c.Job.StartsWith("DomainOS.BurstGuard.", StringComparison.Ordinal) && _n.LiftByName(c.Job)) n++;
        }
        catch (Exception) { }
        try { File.Delete(StatePath); } catch (Exception) { }
        if (n > 0) Log(L.F("Burst guard: {0} CPU caps left from the last run were lifted", n), AppLog.Sev.Info);
        return n;
    }
}

/// <summary>The real Windows calls of the burst guard.</summary>
public sealed class WinBurstNative : IBurstNative
{
    const uint PROCESS_SET_QUOTA = 0x0100, PROCESS_TERMINATE = 0x0001, PROCESS_QUERY_LIMITED = 0x1000;
    const uint JOB_ALL = 0x1F001F;
    const int JobObjectExtendedLimitInformation = 9, JobObjectCpuRateControlInformation = 15;
    const uint JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK = 0x1000;
    const uint CPU_RATE_ENABLE = 0x1, CPU_RATE_WEIGHT_BASED = 0x2, CPU_RATE_HARD_CAP = 0x4;
    const uint QUOTA_LIMITS_HARDWS_MIN_ENABLE = 0x1, QUOTA_LIMITS_HARDWS_MIN_DISABLE = 0x2, QUOTA_LIMITS_HARDWS_MAX_DISABLE = 0x8;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern IntPtr CreateJobObjectW(IntPtr sa, string name);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern IntPtr OpenJobObjectW(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetInformationJobObject(IntPtr job, int cls, byte[] info, int len);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetProcessTimes(IntPtr p, out long create, out long exit, out long kernel, out long user);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetProcessWorkingSetSizeEx(IntPtr p, out UIntPtr min, out UIntPtr max, out uint flags);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetProcessWorkingSetSizeEx(IntPtr p, UIntPtr min, UIntPtr max, uint flags);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool K32GetProcessMemoryInfo(IntPtr p, byte[] counters, int cb);
    [StructLayout(LayoutKind.Sequential)]
    struct MEMORYSTATUSEX { public uint Length, Load; public ulong TotalPhys, AvailPhys, TotalPage, AvailPage, TotalVirtual, AvailVirtual, AvailExtended; }
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX m);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr p, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool LookupPrivilegeValueW(string sys, string name, out long luid);
    [StructLayout(LayoutKind.Sequential, Pack = 4)] struct TP { public uint Count; public long Luid; public uint Attr; }
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool AdjustTokenPrivileges(IntPtr t, bool all, ref TP s, uint len, IntPtr prev, IntPtr ret);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();

    static bool _priv;
    static void Privileges()
    {
        if (_priv) return;
        _priv = true;
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out IntPtr t)) return;
        try
        {
            foreach (var name in new[] { "SeIncreaseWorkingSetPrivilege", "SeIncreaseQuotaPrivilege" })
                if (LookupPrivilegeValueW(null, name, out long luid))
                {
                    var tp = new TP { Count = 1, Luid = luid, Attr = 2 };
                    AdjustTokenPrivileges(t, false, ref tp, 16, IntPtr.Zero, IntPtr.Zero);
                }
        }
        finally { CloseHandle(t); }
    }

    public bool CpuTime(uint pid, out long created, out long cpu100ns)
    {
        created = 0; cpu100ns = 0;
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED, false, pid);
        if (h == IntPtr.Zero) return false;
        try
        {
            if (!GetProcessTimes(h, out created, out _, out long k, out long u)) return false;
            cpu100ns = k + u;
            return true;
        }
        finally { CloseHandle(h); }
    }

    static byte[] CpuRateInfo(uint flags, uint rate)
    {
        var b = new byte[8];
        BitConverter.GetBytes(flags).CopyTo(b, 0);
        BitConverter.GetBytes(rate).CopyTo(b, 4);
        return b;
    }

    public bool Cap(uint pid, string jobName, uint cpuRate, int weight, out IntPtr job, out int error)
    {
        job = IntPtr.Zero; error = 0;
        IntPtr p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid);   // the two rights AssignProcessToJobObject needs
        if (p == IntPtr.Zero) { error = Marshal.GetLastWin32Error(); return false; }
        try
        {
            IntPtr j = CreateJobObjectW(IntPtr.Zero, jobName);
            if (j == IntPtr.Zero) { error = Marshal.GetLastWin32Error(); return false; }
            // JOBOBJECT_EXTENDED_LIMIT_INFORMATION (x64, 144 bytes): BasicLimitInformation.LimitFlags at offset 16.
            var ext = new byte[144];
            BitConverter.GetBytes(JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK).CopyTo(ext, 16);
            // The union after ControlFlags holds CpuRate (hard cap) or Weight (weight-based), both a DWORD at offset 4.
            byte[] rate = weight > 0 ? CpuRateInfo(CPU_RATE_ENABLE | CPU_RATE_WEIGHT_BASED, (uint)Math.Clamp(weight, 1, 9))
                                     : CpuRateInfo(CPU_RATE_ENABLE | CPU_RATE_HARD_CAP, cpuRate);
            if (!SetInformationJobObject(j, JobObjectExtendedLimitInformation, ext, ext.Length)
                || !SetInformationJobObject(j, JobObjectCpuRateControlInformation, rate, rate.Length)
                || !AssignProcessToJobObject(j, p))
            {
                error = Marshal.GetLastWin32Error();
                Lift(j); CloseHandle(j);
                return false;
            }
            job = j;
            return true;
        }
        finally { CloseHandle(p); }
    }

    public void Lift(IntPtr job)
    {
        if (job == IntPtr.Zero) return;
        byte[] off = CpuRateInfo(0, 0);
        SetInformationJobObject(job, JobObjectCpuRateControlInformation, off, off.Length);
    }

    public bool LiftByName(string jobName)
    {
        IntPtr j = OpenJobObjectW(JOB_ALL, false, jobName);
        if (j == IntPtr.Zero) return false;
        Lift(j); CloseHandle(j);
        return true;
    }

    public void Close(IntPtr job) { if (job != IntPtr.Zero) CloseHandle(job); }

    public bool Memory(out ulong avail, out ulong total)
    {
        var m = new MEMORYSTATUSEX { Length = (uint)Marshal.SizeOf<MEMORYSTATUSEX>() };
        bool ok = GlobalMemoryStatusEx(ref m);
        avail = m.AvailPhys; total = m.TotalPhys;
        return ok;
    }

    public bool WorkingSet(uint pid, out ulong ws, out ulong min, out ulong max)
    {
        ws = min = max = 0;
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED, false, pid);
        if (h == IntPtr.Zero) return false;
        try
        {
            // PROCESS_MEMORY_COUNTERS (x64): WorkingSetSize at offset 16.
            var c = new byte[72];
            BitConverter.GetBytes(72).CopyTo(c, 0);
            if (!K32GetProcessMemoryInfo(h, c, c.Length)) return false;
            ws = BitConverter.ToUInt64(c, 16);
            if (!GetProcessWorkingSetSizeEx(h, out UIntPtr mn, out UIntPtr mx, out _)) return false;
            min = (ulong)mn; max = (ulong)mx;
            return true;
        }
        finally { CloseHandle(h); }
    }

    public bool SetWorkingSetMin(uint pid, ulong min, ulong max, bool hard)
    {
        Privileges();
        IntPtr h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED, false, pid);
        if (h == IntPtr.Zero) return false;
        try
        {
            uint flags = (hard ? QUOTA_LIMITS_HARDWS_MIN_ENABLE : QUOTA_LIMITS_HARDWS_MIN_DISABLE) | QUOTA_LIMITS_HARDWS_MAX_DISABLE;
            return SetProcessWorkingSetSizeEx(h, (UIntPtr)min, (UIntPtr)Math.Max(max, min), flags);
        }
        finally { CloseHandle(h); }
    }
}
