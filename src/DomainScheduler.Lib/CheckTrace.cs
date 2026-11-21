using System.Diagnostics;
using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>
/// The measurement behind the "Check" page (1.16.0 Test 1), the same arithmetic as Domain SchedTest: Windows' own kernel
/// trace gives every ready-thread wait (ReadyThread → the context switch that puts the thread on a processor); a second
/// session gives the game's frames (DXGI "Present start", event 42). Both are
/// private real-time sessions on the QPC clock that stop with the measurement. Nothing is opened in the game and
/// nothing in the system is changed. Kept only between Begin and End.
/// </summary>
public sealed class CheckTrace : IDisposable
{
    public string Error { get; private set; } = "";
    public bool Running { get; private set; }
    /// <summary>Waits of the watched process's threads, in QPC ticks.</summary>
    public readonly List<long> GameWaits = new(1 << 18);
    /// <summary>Present timestamps (QPC) of the watched process.</summary>
    public readonly List<long> Presents = new(1 << 15);
    public long First, Last;
    public uint Lost;

    readonly uint _pid;
    readonly HashSet<uint> _gameTids = new();
    readonly Dictionary<uint, long> _ready = new();
    readonly object _gate = new();
    volatile bool _collect;

    const string KernelName = "DomainScheduler-Check";
    const string FramesName = "DomainScheduler-CheckFrames";
    static readonly Guid KernelGuid = new("6e2a8c31-4b7d-4f0e-9a55-1d3c7b2e9f40");
    static readonly Guid FramesGuid = new("a9d40e72-1c3b-46f8-8e21-7b5f0c9d3a16");
    static readonly Guid ThreadGuid = new("3d6fa8d1-fe05-11d0-9dda-00c04fd7ba7c");
    static readonly Guid Dxgi = new("CA11C036-0102-4A2D-A6AD-F03CFED5D3C9");
    readonly byte[] _thread = ThreadGuid.ToByteArray();
    readonly byte[] _dxgi = Dxgi.ToByteArray();
    // PROCESS | THREAD (thread → process rundown) | CSWITCH | DISPATCHER (ReadyThread)
    const uint Flags = 0x1 | 0x2 | 0x10 | 0x800;

    ulong _kSession, _kTrace, _fSession, _fTrace;
    Thread _kThread, _fThread, _flusher;
    EventRecordCallback _kCallback, _fCallback;

    public CheckTrace(uint pid) { _pid = pid; }

    public static double TicksPerSecond => Stopwatch.Frequency;

    public bool Start()
    {
        try
        {
            Stop(KernelName);
            Stop(FramesName);
            uint rc = StartSession(KernelName, KernelGuid, 0x100 /* REAL_TIME */ | 0x02000000 /* SYSTEM_LOGGER */, 1024, 64, 512, Flags, out _kSession);
            if (rc == 5) { Error = L.T("no administrator rights"); return false; }
            if (rc != 0) { Error = L.F("the kernel trace did not start (error {0})", rc); return false; }
            IntPtr fl = Marshal.AllocHGlobal(4);
            Marshal.WriteInt32(fl, (int)Flags);
            uint r2 = TraceSetInformation(_kSession, 4 /* TraceSystemTraceEnableFlagsInfo */, fl, 4);
            Marshal.FreeHGlobal(fl);
            if (r2 != 0) { Stop(KernelName); Error = L.F("the kernel trace did not start (error {0})", r2); return false; }
            _kCallback = OnKernel;
            _kTrace = Open(KernelName, _kCallback);
            if (_kTrace == ulong.MaxValue) { Stop(KernelName); Error = L.T("the session could not be opened"); return false; }
            ulong kt = _kTrace;
            _kThread = new Thread(() => { try { ProcessTrace(new[] { kt }, 1, IntPtr.Zero, IntPtr.Zero); } catch (Exception) { } }) { IsBackground = true, Name = "check-kernel", Priority = ThreadPriority.AboveNormal };
            _kThread.Start();
            Running = true;

            // Frames: optional; without them the waits are still measured.
            if (StartSession(FramesName, FramesGuid, 0x100, 64, 16, 128, 0, out _fSession) == 0)
            {
                var g = Dxgi;
                if (EnableTraceEx2(_fSession, ref g, 1, 4, ulong.MaxValue, 0, 0, IntPtr.Zero) == 0)
                {
                    _fCallback = OnFrames;
                    _fTrace = Open(FramesName, _fCallback);
                    if (_fTrace != ulong.MaxValue)
                    {
                        ulong ft = _fTrace;
                        _fThread = new Thread(() => { try { ProcessTrace(new[] { ft }, 1, IntPtr.Zero, IntPtr.Zero); } catch (Exception) { } }) { IsBackground = true, Name = "check-frames" };
                        _fThread.Start();
                        _flusher = new Thread(() => { while (Running) { Thread.Sleep(500); try { Flush(FramesName); } catch (Exception) { } } }) { IsBackground = true, Name = "check-flush" };
                        _flusher.Start();
                    }
                }
            }
            return true;
        }
        catch (Exception e) { Error = e.Message; return false; }
    }

    /// <summary>The first seconds of a session carry the rundown; the measurement starts here.</summary>
    public void Begin()
    {
        lock (_gate) { _ready.Clear(); GameWaits.Clear(); Presents.Clear(); First = Last = 0; }
        _collect = true;
    }

    public void End()
    {
        _collect = false;
        Lost = LostNow();
    }

    unsafe void OnKernel(IntPtr record)
    {
        byte* r = (byte*)record;
        fixed (byte* g = _thread) for (int i = 0; i < 16; i++) if (r[24 + i] != g[i]) return;
        byte opcode = r[45];
        ushort len = *(ushort*)(r + 86);
        byte* u = *(byte**)(r + 96);
        if (u == null) return;
        long ts = *(long*)(r + 16);
        ushort flags = *(ushort*)(r + 4);
        int cpu = (flags & 0x200) != 0 ? *(ushort*)(r + 80) : *(r + 80);
        lock (_gate)
            switch (opcode)
            {
                case 1: case 3:                                    // Thread Start / DCStart: ProcessId, TThreadId
                    if (len >= 8 && *(uint*)u == _pid) _gameTids.Add(*(uint*)(u + 4));
                    return;
                case 50:                                           // ReadyThread: TThreadId
                    if (!_collect || len < 4) return;
                    uint t = *(uint*)u;
                    if (_gameTids.Contains(t)) _ready.TryAdd(t, ts);
                    return;
                case 36:                                           // CSwitch: NewThreadId, OldThreadId
                    if (!_collect || len < 8) return;
                    if (First == 0) First = ts;
                    Last = ts;
                    uint nt = *(uint*)u;
                    if (nt != 0 && _ready.Remove(nt, out long readyAt) && ts >= readyAt) GameWaits.Add(ts - readyAt);
                    return;
            }
    }

    unsafe void OnFrames(IntPtr record)
    {
        if (!_collect) return;
        byte* r = (byte*)record;
        fixed (byte* g = _dxgi) for (int i = 0; i < 16; i++) if (r[24 + i] != g[i]) return;
        if (*(ushort*)(r + 40) != 42 || *(uint*)(r + 12) != _pid) return;
        lock (_gate) Presents.Add(*(long*)(r + 16));
    }

    /// <summary>A copy of what was collected (the sessions keep running until Dispose).</summary>
    public (long[] Waits, long[] Presents, long First, long Last) Snapshot()
    {
        lock (_gate) return (GameWaits.ToArray(), Presents.OrderBy(x => x).ToArray(), First, Last);
    }

    static ulong Open(string name, EventRecordCallback cb)
    {
        var lf = new EVENT_TRACE_LOGFILEW
        {
            LoggerName = Marshal.StringToHGlobalUni(name),
            ProcessTraceMode = 0x100 /* REAL_TIME */ | 0x10000000 /* EVENT_RECORD */ | 0x1000 /* RAW_TIMESTAMP */,
            EventRecordCallback = Marshal.GetFunctionPointerForDelegate(cb),
        };
        return OpenTraceW(ref lf);
    }

    static IntPtr Props(int size)
    {
        IntPtr p = Marshal.AllocHGlobal(size);
        for (int i = 0; i < size; i++) Marshal.WriteByte(p, i, 0);
        Marshal.WriteInt32(p, 0, size);
        Marshal.WriteInt32(p, 116, 120);
        return p;
    }

    static uint StartSession(string name, Guid guid, uint mode, int bufferKb, int minBuffers, int maxBuffers, uint flags, out ulong session)
    {
        int size = 120 + 2048;
        IntPtr p = Props(size);
        try
        {
            Marshal.Copy(guid.ToByteArray(), 0, p + 24, 16);
            Marshal.WriteInt32(p, 40, 1);                           // QPC timestamps
            Marshal.WriteInt32(p, 44, 0x20000);                     // WNODE_FLAG_TRACED_GUID
            Marshal.WriteInt32(p, 48, bufferKb);
            Marshal.WriteInt32(p, 52, minBuffers);
            Marshal.WriteInt32(p, 56, maxBuffers);
            Marshal.WriteInt32(p, 64, (int)mode);
            Marshal.WriteInt32(p, 68, 1);                           // FlushTimer (s)
            Marshal.WriteInt32(p, 72, (int)flags);
            return StartTraceW(out session, name, p);
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    uint LostNow()
    {
        IntPtr p = Props(120 + 2048);
        try { return ControlTraceW(0, KernelName, p, 0) == 0 ? (uint)Marshal.ReadInt32(p, 88) + (uint)Marshal.ReadInt32(p, 100) : 0; }
        finally { Marshal.FreeHGlobal(p); }
    }

    static void Flush(string name) { IntPtr p = Props(120 + 2048); try { ControlTraceW(0, name, p, 3 /* FLUSH */); } finally { Marshal.FreeHGlobal(p); } }
    static void Stop(string name) { IntPtr p = Props(120 + 2048); try { ControlTraceW(0, name, p, 1 /* STOP */); } finally { Marshal.FreeHGlobal(p); } }

    public void Dispose()
    {
        if (!Running) return;
        Running = false;
        try { Flush(FramesName); } catch (Exception) { }
        Stop(KernelName);
        Stop(FramesName);
        CloseTrace(_kTrace);
        if (_fTrace != 0 && _fTrace != ulong.MaxValue) CloseTrace(_fTrace);
        _kThread?.Join(5000);
        _fThread?.Join(3000);
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
    [DllImport("advapi32.dll")] static extern uint EnableTraceEx2(ulong handle, ref Guid provider, uint control, byte level, ulong any, ulong all, uint timeout, IntPtr parameters);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern ulong OpenTraceW(ref EVENT_TRACE_LOGFILEW logfile);
    [DllImport("advapi32.dll")] static extern uint ProcessTrace(ulong[] handles, uint count, IntPtr start, IntPtr end);
    [DllImport("advapi32.dll")] static extern uint CloseTrace(ulong handle);
}
