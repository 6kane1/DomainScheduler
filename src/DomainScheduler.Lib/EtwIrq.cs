using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>
/// Live DPC/ISR time per processor from Windows' own kernel trace, run only while a game is handled. Two uses:
///  - busy interrupt cores: a physical core whose processor spends more than 4% of its time in DPC/ISR (over the
///    last 10 s) counts as busy until it falls under 2%, and a new set is accepted only after it held 20 s, so the
///    game mask does not flap. Auto leaves busy cores (both SMT threads) out of the game mask — with the narrow rule
///    (default) only the busiest of them, next to the one strongest graphics-driver core;
///  - the latency watchdog: DPCs and interrupts of 0.5 ms and longer are counted per driver and logged once a minute.
/// Nothing is changed by this class; the session stops when the game ends and on exit.
/// </summary>
public sealed class EtwIrq : IDisposable
{
    public const int WindowSeconds = 10;
    public const double NoisyShare = 0.02;
    public const double BusyEnter = 0.04, BusyLeave = 0.02;
    public const int BusyHoldSeconds = 20;
    public const double LongMs = 0.5, VeryLongMs = 1.0;
    /// <summary>
    /// Graphics-driver cores: a core whose processors ran DPCs/ISRs of the graphics driver for at least 0.2% of the
    /// last 3 s. A game thread woken by that driver cannot start on that core until the DPC ends (~80 µs measured), so
    /// the game keeps off it — and from the third second of a game, not after the 30 s of the busy-core rule.
    /// </summary>
    public const double GpuShare = 0.002;
    public const int GpuSeconds = 3;
    static readonly string[] GpuDrivers = { "nvlddmkm", "amdkmdag", "atikmdag", "igdkmd", "dxgkrnl", "dxgmms" };
    const string SessionName = "DomainScheduler-Irq";

    readonly object _gate = new();
    readonly double[,] _ms = new double[64, WindowSeconds];     // per processor, per second slot
    readonly double[,] _gpuMs = new double[64, WindowSeconds];  // the graphics driver's part of it
    readonly Dictionary<ulong, bool> _gpuRoutine = new();
    readonly long[] _slotSecond = new long[WindowSeconds];
    Thread _thread;
    public Func<DateTime> Now = () => DateTime.UtcNow;

    public bool Available { get; private set; }
    public string Error { get; private set; } = "";

    /// <summary>Starts the tracer; false (with one warning) when it cannot run: the registry pins are used alone.</summary>
    public bool Start()
    {
        if (Available) return true;
        try
        {
            LoadDrivers();
            _usPerTick = 1e3 / System.Diagnostics.Stopwatch.Frequency;     // ms per QPC tick
            StopByName(SessionName);
            // A private system-logger session (Windows 8+): the shared "NT Kernel Logger" stays free for other tools.
            uint rc = StartSession();
            if (rc == 5) throw new InvalidOperationException(L.T("no administrator rights"));
            if (rc != 0) throw new InvalidOperationException(L.F("error {0}", rc));
            IntPtr fl = Marshal.AllocHGlobal(4);
            Marshal.WriteInt32(fl, (int)Flags);
            rc = TraceSetInformation(_handle, 4 /* TraceSystemTraceEnableFlagsInfo */, fl, 4);
            Marshal.FreeHGlobal(fl);
            if (rc != 0) { StopByName(SessionName); throw new InvalidOperationException(L.F("error {0}", rc)); }
            _callback = OnEvent;
            var lf = new EVENT_TRACE_LOGFILEW
            {
                LoggerName = Marshal.StringToHGlobalUni(SessionName),
                // REAL_TIME | EVENT_RECORD | RAW_TIMESTAMP: the header time stays in QPC ticks like InitialTime.
                ProcessTraceMode = 0x100 | 0x10000000 | 0x1000,
                EventRecordCallback = Marshal.GetFunctionPointerForDelegate(_callback),
            };
            _trace = OpenTraceW(ref lf);
            if (_trace == ulong.MaxValue) { StopByName(SessionName); throw new InvalidOperationException(L.T("the session could not be opened")); }
            ulong trace = _trace;
            _thread = new Thread(() =>
            {
                try { ProcessTrace(new[] { trace }, 1, IntPtr.Zero, IntPtr.Zero); }
                catch (Exception) { }
            }) { IsBackground = true, Name = "etw-irq", Priority = ThreadPriority.BelowNormal };
            _thread.Start();
            Available = true;
            AppLog.Add(L.T("Live DPC/ISR: on"));
            return true;
        }
        catch (Exception e)
        {
            Error = e.Message;
            Stop(false);
            AppLog.Add(L.F("Live DPC/ISR is not available ({0}): only the registry pins are used", e.Message), AppLog.Sev.Warn);
            return false;
        }
    }

    // ---- raw ETW: one pointer read per field, no event objects -------------------------------------------------

    const uint Flags = 0x20 /* DPC */ | 0x40 /* INTERRUPT */;
    static readonly Guid SessionGuid = new("6b0f3c2e-1d4a-4c8e-9f21-7a5e3b9d4c18");
    static readonly byte[] PerfInfo = new Guid("ce1dbfb4-137e-4da6-87b0-3f59aa102cbc").ToByteArray();
    ulong _handle, _trace;
    double _usPerTick;
    EventRecordCallback _callback;

    unsafe void OnEvent(IntPtr record)
    {
        byte* r = (byte*)record;
        fixed (byte* g = PerfInfo)
            for (int i = 0; i < 16; i++) if (r[24 + i] != g[i]) return;          // EVENT_HEADER.ProviderId
        byte op = r[45];                                                       // EVENT_DESCRIPTOR.Opcode
        if (op != 66 && op != 67 && op != 68 && op != 69 && op != 70 && op != 50) return;   // DPC kinds, ISR, ISR-MSI
        ushort len = *(ushort*)(r + 86);
        byte* user = *(byte**)(r + 96);
        if (user == null || len < 16) return;
        long initial = *(long*)user;                                           // InitialTime (QPC)
        ulong routine = *(ulong*)(user + 8);
        long stamp = *(long*)(r + 16);                                         // EVENT_HEADER.TimeStamp (QPC)
        ushort flags = *(ushort*)(r + 4);
        int cpu = (flags & 0x200) != 0 ? *(ushort*)(r + 80) : *(r + 80);
        Add(cpu, (stamp - initial) * _usPerTick, routine);
    }

    uint StartSession()
    {
        int size = 120 + 2048;
        IntPtr p = Marshal.AllocHGlobal(size);
        try
        {
            for (int i = 0; i < size; i++) Marshal.WriteByte(p, i, 0);
            Marshal.WriteInt32(p, 0, size);                         // Wnode.BufferSize
            Marshal.Copy(SessionGuid.ToByteArray(), 0, p + 24, 16); // Wnode.Guid
            Marshal.WriteInt32(p, 40, 1);                           // Wnode.ClientContext: QPC
            Marshal.WriteInt32(p, 44, 0x20000);                     // WNODE_FLAG_TRACED_GUID
            Marshal.WriteInt32(p, 48, 256);                         // BufferSize (KB)
            Marshal.WriteInt32(p, 52, 16);                          // MinimumBuffers
            Marshal.WriteInt32(p, 56, 128);                         // MaximumBuffers
            Marshal.WriteInt32(p, 64, 0x100 | 0x02000000);          // REAL_TIME | SYSTEM_LOGGER
            Marshal.WriteInt32(p, 68, 1);                           // FlushTimer (s)
            Marshal.WriteInt32(p, 72, (int)Flags);                  // EnableFlags
            Marshal.WriteInt32(p, 116, 120);                        // LoggerNameOffset
            return StartTraceW(out _handle, SessionName, p);
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    static void StopByName(string name)
    {
        int size = 120 + 2048;
        IntPtr p = Marshal.AllocHGlobal(size);
        try
        {
            for (int i = 0; i < size; i++) Marshal.WriteByte(p, i, 0);
            Marshal.WriteInt32(p, 0, size);
            Marshal.WriteInt32(p, 116, 120);
            ControlTraceW(0, name, p, 1 /* STOP */);
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate void EventRecordCallback(IntPtr record);

    [StructLayout(LayoutKind.Explicit, Size = 448)]
    struct EVENT_TRACE_LOGFILEW
    {
        [FieldOffset(8)] public IntPtr LoggerName;
        [FieldOffset(28)] public uint ProcessTraceMode;
        [FieldOffset(424)] public IntPtr EventRecordCallback;
    }

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern uint StartTraceW(out ulong handle, string name, IntPtr props);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern uint ControlTraceW(ulong handle, string name, IntPtr props, uint code);
    [DllImport("advapi32.dll")] static extern uint TraceSetInformation(ulong handle, int cls, IntPtr info, uint len);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern ulong OpenTraceW(ref EVENT_TRACE_LOGFILEW logfile);
    [DllImport("advapi32.dll")] static extern uint ProcessTrace(ulong[] handles, uint count, IntPtr start, IntPtr end);
    [DllImport("advapi32.dll")] static extern uint CloseTrace(ulong handle);

    internal void Add(int cpu, double ms) => Add(cpu, ms, 0);

    internal void Add(int cpu, double ms, ulong routine)
    {
        if (cpu < 0 || cpu >= 64 || ms <= 0) return;
        long sec = Now().Ticks / TimeSpan.TicksPerSecond;
        int slot = (int)(sec % WindowSeconds);
        lock (_gate)
        {
            if (_slotSecond[slot] != sec)
            {
                _slotSecond[slot] = sec;
                for (int c = 0; c < 64; c++) { _ms[c, slot] = 0; _gpuMs[c, slot] = 0; }
            }
            _ms[cpu, slot] += ms;
            if (routine != 0)
            {
                if (!_gpuRoutine.TryGetValue(routine, out bool gpu))
                {
                    string drv = DriverOf(routine);
                    _gpuRoutine[routine] = gpu = GpuDrivers.Any(g => drv.StartsWith(g, StringComparison.OrdinalIgnoreCase));
                }
                if (gpu) _gpuMs[cpu, slot] += ms;
            }
            if (ms >= LongMs && ms < 1000)
            {
                if (!_long.TryGetValue(routine, out var s)) _long[routine] = s = new LongStat();
                s.Count++;
                if (ms >= VeryLongMs) s.VeryLong++;
                if (ms > s.MaxMs) s.MaxMs = ms;
            }
        }
    }

    /// <summary>DPC + ISR share of every processor over the last window (0..1).</summary>
    public double[] Shares()
    {
        var shares = new double[64];
        if (!Available) return shares;
        long sec = Now().Ticks / TimeSpan.TicksPerSecond;
        lock (_gate)
            for (int c = 0; c < 64; c++)
            {
                double total = 0;
                for (int s = 0; s < WindowSeconds; s++)
                    if (sec - _slotSecond[s] < WindowSeconds) total += _ms[c, s];
                shares[c] = total / (WindowSeconds * 1000.0);
            }
        return shares;
    }

    /// <summary>Mask of the processors whose DPC + ISR share over the last 10 s is above 2%.</summary>
    public ulong Noisy()
    {
        var sh = Shares();
        ulong mask = 0;
        for (int c = 0; c < 64; c++) if (sh[c] > NoisyShare) mask |= 1UL << c;
        return mask;
    }

    // ---- busy interrupt cores (hysteresis + hold) --------------------------------------------------------------

    ulong _busy, _candidate;
    DateTime _candidateSince, _startedAt;
    bool _ready;

    ulong _gpuLogged = ulong.MaxValue;


    /// <summary>
    /// Cores the game keeps off: the busy interrupt cores once their set has held, plus the graphics-driver cores as
    /// soon as 3 s of trace exist. Null while
    /// neither is known (callers keep their old rule: the registry pins).
    /// </summary>
    public ulong? BusyCoresIfReady(Topo t)
    {
        ulong extra = BusyCores(t);
        ulong gpu = GpuCores(t);
        lock (_gate)
        {
            bool gpuReady = _startedAt != default && (Now() - _startedAt).TotalSeconds >= GpuSeconds;
            if (gpuReady && gpu != _gpuLogged)
            {
                _gpuLogged = gpu;
                AppLog.Add(L.F("Graphics driver cores (game keeps off them): {0}", gpu == 0 ? L.T("none") : Masks.Ranges(gpu)));
            }
            if (_ready) return extra | gpu;
            return gpuReady && gpu != 0 ? gpu : null;
        }
    }

    /// <summary>The graphics driver's DPC/ISR share of every processor over the last 3 s (0..1).</summary>
    double[] GpuShares()
    {
        var share = new double[64];
        if (!Available) return share;
        long sec = Now().Ticks / TimeSpan.TicksPerSecond;
        lock (_gate)
            for (int c = 0; c < 64; c++)
            {
                double total = 0;
                for (int sl = 0; sl < WindowSeconds; sl++)
                    if (sec - _slotSecond[sl] < GpuSeconds) total += _gpuMs[c, sl];
                share[c] = total / (GpuSeconds * 1000.0);
            }
        return share;
    }

    /// <summary>Whole physical cores whose processors ran the graphics driver's DPCs/ISRs ≥ 0.2% of the last 3 s.</summary>
    public ulong GpuCores(Topo t)
    {
        if (!Available || t?.Sets == null) return 0;
        var share = GpuShares();
        ulong m = 0;
        foreach (var core in t.Sets.Where(x => x.Group == 0 && x.LP < 64).GroupBy(x => x.Core))
            if (core.Max(x => share[x.LP]) >= GpuShare) m |= TopoBuilder.Mask(core);
        return m;
    }

    /// <summary>Tests: driver load addresses without Windows.</summary>
    internal void SetDrivers(List<(ulong Base, string Name)> drivers) { _drivers = drivers; _gpuRoutine.Clear(); }


    /// <summary>
    /// Whole physical cores (all their processors) that are busy with interrupts: enter above 4%, leave under 2%,
    /// a changed set is accepted after it held 20 s. 0 while the tracer is off or has no full window yet.
    /// </summary>
    public ulong BusyCores(Topo t)
    {
        if (!Available || t?.Sets == null) return 0;
        var sh = Shares();
        ulong next = 0;
        foreach (var core in t.Sets.Where(s => s.Group == 0 && s.LP < 64).GroupBy(s => s.Core))
        {
            ulong coreMask = TopoBuilder.Mask(core);
            double max = core.Max(s => sh[s.LP]);
            bool was = (_busy & coreMask) != 0;
            if (max >= BusyEnter || (was && max >= BusyLeave)) next |= coreMask;
        }
        var now = Now();
        lock (_gate)
        {
            if (_startedAt == default) _startedAt = now;
            if (!_ready && (now - _startedAt).TotalSeconds >= WindowSeconds + BusyHoldSeconds && next == _candidate && (now - _candidateSince).TotalSeconds >= BusyHoldSeconds)
            {
                _ready = true;
                _busy = next;
                AppLog.Add(L.F("Interrupt cores busy (game keeps off them): {0}", next == 0 ? L.T("none") : Masks.Ranges(next)));
                return _busy;
            }
            if (next == _busy && _ready) { _candidate = next; return _busy; }
            if (next != _candidate || _candidateSince == default) { _candidate = next; _candidateSince = now; return _busy; }
            if (_ready && (now - _candidateSince).TotalSeconds >= BusyHoldSeconds)
            {
                ulong old = _busy;
                _busy = next;
                if (old != next) AppLog.Add(L.F("Interrupt cores busy (game keeps off them): {0}", next == 0 ? L.T("none") : Masks.Ranges(next)));
            }
            return _busy;
        }
    }

    // ---- latency watchdog -----------------------------------------------------------------------------------------

    sealed class LongStat { public long Count, VeryLong; public double MaxMs; }
    readonly Dictionary<ulong, LongStat> _long = new();
    List<(ulong Base, string Name)> _drivers = new();

    void LoadDrivers()
    {
        try { _drivers = Drivers(); } catch (Exception) { _drivers = new(); }
    }

    /// <summary>Long DPCs/ISRs since the last call, per driver: (driver, ≥0.5 ms, ≥1 ms, longest ms), longest first.</summary>
    public List<(string Driver, long Long, long VeryLong, double MaxMs)> TakeLong()
    {
        Dictionary<ulong, LongStat> copy;
        lock (_gate)
        {
            copy = new Dictionary<ulong, LongStat>(_long);
            _long.Clear();
        }
        return copy.GroupBy(kv => DriverOf(kv.Key))
            .Select(g => (g.Key, g.Sum(x => x.Value.Count), g.Sum(x => x.Value.VeryLong), g.Max(x => x.Value.MaxMs)))
            .OrderByDescending(x => x.Item4).ToList();
    }

    string DriverOf(ulong addr)
    {
        var list = _drivers;
        string best = "?";
        ulong bestBase = 0;
        foreach (var (b, n) in list)
            if (b <= addr && b >= bestBase) { bestBase = b; best = n; }
        return addr - bestBase > 0x8000000 ? "?" : best;
    }

    static List<(ulong, string)> Drivers()
    {
        var list = new List<(ulong, string)>();
        var arr = new IntPtr[4096];
        if (!EnumDeviceDrivers(arr, (uint)(arr.Length * IntPtr.Size), out uint needed)) return list;
        int n = (int)Math.Min(needed / (uint)IntPtr.Size, (uint)arr.Length);
        var buf = new char[260];
        for (int i = 0; i < n; i++)
        {
            if (arr[i] == IntPtr.Zero) continue;
            uint len = GetDeviceDriverBaseNameW(arr[i], buf, (uint)buf.Length);
            list.Add(((ulong)arr[i].ToInt64(), len > 0 ? new string(buf, 0, (int)len) : "?"));
        }
        return list;
    }

    [DllImport("psapi.dll", SetLastError = true)] static extern bool EnumDeviceDrivers([Out] IntPtr[] images, uint cb, out uint needed);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode)] static extern uint GetDeviceDriverBaseNameW(IntPtr image, [Out] char[] name, uint size);

    internal void MarkAvailableForTests() => Available = true;

    void Stop(bool log)
    {
        if (Available || _trace != 0)
        {
            StopByName(SessionName);
            try { if (_trace != 0 && _trace != ulong.MaxValue) CloseTrace(_trace); } catch (Exception) { }
            _thread?.Join(2000);
        }
        _trace = 0;
        if (Available && log) AppLog.Add(L.T("Live DPC/ISR: off"));
        Available = false;
        lock (_gate)
        {
            _busy = _candidate = 0;
            _ready = false;
            _startedAt = _candidateSince = default;
            _long.Clear();
            for (int c = 0; c < 64; c++) for (int s = 0; s < WindowSeconds; s++) { _ms[c, s] = 0; _gpuMs[c, s] = 0; }
            _gpuLogged = ulong.MaxValue;
        }
    }

    /// <summary>A session left by a killed copy of the program is stopped (called once at start).</summary>
    public static void StopOrphan() => StopByName(SessionName);

    /// <summary>Stops the scheduler trace an earlier test build (Domain Engine, removed) may have left running after a crash.</summary>
    public static void StopOrphanSched() => StopByName("DomainScheduler-Sched");

    /// <summary>Stops the session (the game ended); Start can be called again.</summary>
    public void Pause() => Stop(true);

    public void Dispose() => Stop(true);
}
