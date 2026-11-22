using System.Diagnostics;
using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>
/// The game's own pain, measured cheaply while Auto handles it (1.16.1 Test 2). One private kernel session runs with
/// only process and thread events (a few per second); every <see cref="ProbePolicy.PeriodMs"/> it switches on context
/// switches, ready threads and hard page faults for <see cref="ProbePolicy.WindowMs"/> and off again, so the heavy events
/// flow about 12% of the time. From each window:
///  - per game thread: the 99th percentile of its ready waits (ReadyThread -> the context switch that runs it), which
///    Dynamic Top-5 uses to rank and move threads by what they WAIT, not only by what they use;
///  - the game's hard page faults per second (a page read from disk because Windows had trimmed it), which the burst
///    guard uses to decide whether holding the game's memory helps.
/// A window that lost events is thrown away. Nothing is changed; the trace only reads.
/// </summary>
public static class ProbePolicy
{
    public const int WindowMs = 250, PeriodMs = 2000;
    public const int MinWaits = 3;                 // fewer waits in a window: no p99 for that thread
}

public sealed class ReadyProbe : IDisposable
{
    readonly object _gate = new();
    readonly Dictionary<uint, uint> _tidPid = new();
    readonly HashSet<uint> _gameTids = new();
    readonly Dictionary<uint, long> _ready = new();
    readonly Dictionary<uint, List<long>> _waits = new();
    int _faults; bool _window; long _windowStart;
    uint _pid;
    readonly double _tpm = Stopwatch.Frequency / 1000.0;
    Dictionary<uint, double> _p99 = new();
    double? _faultsPerSec;
    DateTime _nextWindow;
    public bool Running { get; private set; }
    public string Error { get; private set; } = "";
    public uint GamePid => _pid;
    public Func<DateTime> Now = () => DateTime.UtcNow;

    /// <summary>p99 ready wait (ms) of a game thread in the last good window; -1 when unknown.</summary>
    public double WaitP99Ms(uint tid) { lock (_gate) return _p99.TryGetValue(tid, out double v) ? v : -1; }

    /// <summary>The game's hard page faults per second in the last good window; null when unknown.</summary>
    public double? HardFaultsPerSec { get { lock (_gate) return _faultsPerSec; } }

    public void Start(uint pid)
    {
        if (Running && pid == _pid) return;
        Stop();
        lock (_gate) { _pid = pid; _tidPid.Clear(); _gameTids.Clear(); _ready.Clear(); _waits.Clear(); _p99 = new(); _faultsPerSec = null; _window = false; }
        Running = StartSession();
        _nextWindow = Now();
    }

    public void Stop()
    {
        if (!Running) return;
        Running = false;
        _closer?.Dispose(); _closer = null;
        StopByName(Name);
        try { if (_t != 0 && _t != ulong.MaxValue) CloseTrace(_t); } catch (Exception) { }
        _th?.Join(2000); _t = 0;
    }

    public void Dispose() => Stop();
    public static void StopOrphan() => StopByName(Name);

    /// <summary>About once a second: opens or closes a window and computes its results.</summary>
    public void Tick()
    {
        if (!Running) return;
        DateTime now = Now();
        if (!_window && now >= _nextWindow)
        {
            lock (_gate) { _ready.Clear(); _waits.Clear(); _faults = 0; _window = true; _windowStart = Stopwatch.GetTimestamp(); }
            SetFlags(BaseFlags | WindowFlags);
            _nextWindow = now.AddMilliseconds(ProbePolicy.PeriodMs);
            // The hub ticks once a second; a one-shot timer closes the window after WindowMs, so the heavy events flow
            // only ~250 ms of every 2 s.
            _closer?.Dispose();
            _closer = new Timer(_ => { try { Close(); } catch (Exception) { } }, null, ProbePolicy.WindowMs, Timeout.Infinite);
        }
    }

    Timer _closer;

    /// <summary>Ends the window: the heavy events go off and the window's results are computed.</summary>
    void Close()
    {
        if (!Running) return;
        SetFlags(BaseFlags);
        uint lost = Lost();
        lock (_gate)
        {
            _window = false;
            double sec = (Stopwatch.GetTimestamp() - _windowStart) / _tpm / 1000.0;
            if (lost != _lostSeen) { _lostSeen = lost; return; }       // events were lost: this window is not trusted
            _p99 = _waits.Where(kv => kv.Value.Count >= ProbePolicy.MinWaits)
                         .ToDictionary(kv => kv.Key, kv => P99(kv.Value) / _tpm);
            _faultsPerSec = sec > 0 ? _faults / sec : null;
        }
    }

    internal static double P99(List<long> v)
    {
        var s = v.OrderBy(x => x).ToArray();
        return s[Math.Clamp((int)Math.Ceiling(0.99 * s.Length) - 1, 0, s.Length - 1)];
    }

    // ---- events ---------------------------------------------------------------------------------------------------
    internal void OnThread(uint pid, uint tid) { lock (_gate) { _tidPid[tid] = pid; if (pid == _pid) _gameTids.Add(tid); } }
    internal void OnReady(uint tid, long ts) { lock (_gate) if (_window && _gameTids.Contains(tid)) _ready.TryAdd(tid, ts); }
    internal void OnSwitch(uint newTid, long ts)
    {
        lock (_gate)
            if (_window && newTid != 0 && _ready.Remove(newTid, out long at) && ts >= at)
            {
                if (!_waits.TryGetValue(newTid, out var l)) _waits[newTid] = l = new List<long>();
                l.Add(ts - at);
            }
    }
    internal void OnHardFault(uint tid) { lock (_gate) if (_window && _gameTids.Contains(tid)) _faults++; }

    // ---- raw ETW --------------------------------------------------------------------------------------------------
    const string Name = "DomainScheduler-Probe";
    static readonly Guid SessionGuid = new("5e8b2d71-c4a9-4f36-b1d0-8a7e3c6f2b94");
    static readonly byte[] ThreadG = new Guid("3d6fa8d1-fe05-11d0-9dda-00c04fd7ba7c").ToByteArray();
    static readonly byte[] FaultG = new Guid("3d6fa8d3-fe05-11d0-9dda-00c04fd7ba7c").ToByteArray();
    const uint BaseFlags = 0x1 | 0x2;                         // PROCESS | THREAD
    const uint WindowFlags = 0x10 | 0x800 | 0x2000;           // CSWITCH | DISPATCHER | MEMORY_HARD_FAULTS
    ulong _s, _t; Thread _th; EventRecordCallback _cb; uint _lostSeen;

    bool StartSession()
    {
        try
        {
            StopByName(Name);
            int size = 120 + 2048; IntPtr p = Props(size);
            uint rc;
            try
            {
                Marshal.Copy(SessionGuid.ToByteArray(), 0, p + 24, 16);
                Marshal.WriteInt32(p, 40, 1); Marshal.WriteInt32(p, 44, 0x20000);
                Marshal.WriteInt32(p, 48, 256); Marshal.WriteInt32(p, 52, 32); Marshal.WriteInt32(p, 56, 256);
                Marshal.WriteInt32(p, 64, 0x100 | 0x02000000); Marshal.WriteInt32(p, 68, 1);
                rc = StartTraceW(out _s, Name, p);
            }
            finally { Marshal.FreeHGlobal(p); }
            if (rc != 0) { Error = rc == 5 ? L.T("no administrator rights") : L.F("error {0}", rc); return false; }
            if (!SetFlags(BaseFlags)) { StopByName(Name); return false; }
            _cb = OnEvent;
            var lf = new EVENT_TRACE_LOGFILEW { LoggerName = Marshal.StringToHGlobalUni(Name), ProcessTraceMode = 0x100 | 0x10000000 | 0x1000, EventRecordCallback = Marshal.GetFunctionPointerForDelegate(_cb) };
            _t = OpenTraceW(ref lf);
            if (_t == ulong.MaxValue) { StopByName(Name); Error = L.T("the session could not be opened"); return false; }
            ulong t = _t;
            _th = new Thread(() => { SelfThreads.Register(L.N("Wait probe")); try { ProcessTrace(new[] { t }, 1, IntPtr.Zero, IntPtr.Zero); } catch (Exception) { } })
                { IsBackground = true, Name = "probe", Priority = ThreadPriority.BelowNormal };
            _th.Start();
            _lostSeen = Lost();
            return true;
        }
        catch (Exception e) { Error = e.Message; StopByName(Name); return false; }
    }

    bool SetFlags(uint flags)
    {
        IntPtr fl = Marshal.AllocHGlobal(4);
        try { Marshal.WriteInt32(fl, (int)flags); return TraceSetInformation(_s, 4, fl, 4) == 0; }
        finally { Marshal.FreeHGlobal(fl); }
    }

    uint Lost()
    {
        IntPtr p = Props(120 + 2048);
        try { return ControlTraceW(0, Name, p, 0) == 0 ? (uint)Marshal.ReadInt32(p, 88) + (uint)Marshal.ReadInt32(p, 100) : 0; }   // EventsLost + RealTimeBuffersLost
        finally { Marshal.FreeHGlobal(p); }
    }

    unsafe void OnEvent(IntPtr record)
    {
        byte* r = (byte*)record;
        byte op = r[45]; ushort len = *(ushort*)(r + 86); byte* u = *(byte**)(r + 96);
        if (u == null) return;
        bool thread = true, fault = true;
        for (int i = 0; i < 16; i++) { if (r[24 + i] != ThreadG[i]) thread = false; if (r[24 + i] != FaultG[i]) fault = false; }
        if (thread)
        {
            long ts = *(long*)(r + 16);
            switch (op)
            {
                case 1: case 3: if (len >= 8) OnThread(*(uint*)u, *(uint*)(u + 4)); return;   // Start / DCStart
                case 50: if (len >= 4) OnReady(*(uint*)u, ts); return;                         // ReadyThread
                case 36: if (len >= 4) OnSwitch(*(uint*)u, ts); return;                        // CSwitch: NewThreadId
            }
        }
        else if (fault && op == 32 && len >= 40) OnHardFault(*(uint*)(u + 32));             // HardFault: TThreadId at 32 (x64)
    }

    static IntPtr Props(int size)
    {
        IntPtr p = Marshal.AllocHGlobal(size);
        for (int i = 0; i < size; i++) Marshal.WriteByte(p, i, 0);
        Marshal.WriteInt32(p, 0, size); Marshal.WriteInt32(p, 116, 120);
        return p;
    }
    static void StopByName(string name) { IntPtr p = Props(120 + 2048); try { ControlTraceW(0, name, p, 1); } finally { Marshal.FreeHGlobal(p); } }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate void EventRecordCallback(IntPtr record);
    [StructLayout(LayoutKind.Explicit, Size = 448)]
    struct EVENT_TRACE_LOGFILEW { [FieldOffset(8)] public IntPtr LoggerName; [FieldOffset(28)] public uint ProcessTraceMode; [FieldOffset(424)] public IntPtr EventRecordCallback; }
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern uint StartTraceW(out ulong handle, string name, IntPtr props);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern uint ControlTraceW(ulong handle, string name, IntPtr props, uint code);
    [DllImport("advapi32.dll")] static extern uint TraceSetInformation(ulong handle, int cls, IntPtr info, uint len);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern ulong OpenTraceW(ref EVENT_TRACE_LOGFILEW logfile);
    [DllImport("advapi32.dll")] static extern uint ProcessTrace(ulong[] handles, uint count, IntPtr start, IntPtr end);
    [DllImport("advapi32.dll")] static extern uint CloseTrace(ulong handle);

    /// <summary>Tests: feed a window of synthetic events.</summary>
    internal void TestWindow(uint pid, Action<ReadyProbe> events, double seconds)
    {
        lock (_gate) { _pid = pid; _window = true; _windowStart = Stopwatch.GetTimestamp() - (long)(seconds * 1000 * _tpm); _ready.Clear(); _waits.Clear(); _faults = 0; }
        events(this);
        lock (_gate)
        {
            _window = false;
            _p99 = _waits.Where(kv => kv.Value.Count >= ProbePolicy.MinWaits).ToDictionary(kv => kv.Key, kv => P99(kv.Value) / _tpm);
            _faultsPerSec = _faults / seconds;
        }
    }
}
