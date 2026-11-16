using Microsoft.Diagnostics.Tracing;
using Microsoft.Diagnostics.Tracing.Session;

namespace DomainScheduler;

/// <summary>
/// Frame times from ETW (section 17.4): the Present_Start events (id 42) of the
/// Microsoft-Windows-DXGI provider for the watched process. When the session
/// cannot be started (no rights, old Windows), Available is false and the
/// Report says so; the Auto mode is not blocked.
/// </summary>
public sealed class EtwFrames : IFrameSource, IDisposable
{
    static readonly Guid Dxgi = new("CA11C036-0102-4A2D-A6AD-F03CFED5D3C9");
    const string SessionName = "DomainScheduler-Frames";
    const int PresentStart = 42, Keep = 20000;

    readonly object _gate = new();
    readonly Queue<DateTime> _frames = new();
    TraceEventSession _session;
    Thread _thread;
    volatile uint _pid;

    public bool Available { get; private set; }
    public string Name => "ETW DXGI Present";
    public string Error { get; private set; } = "";

    public void Start()
    {
        try
        {
            if (TraceEventSession.IsElevated() != true)
            {
                Error = L.T("no administrator rights");
                return;
            }
            _session = new TraceEventSession(SessionName) { StopOnDispose = true };
            _session.EnableProvider(Dxgi, TraceEventLevel.Informational, ulong.MaxValue);
            _session.Source.Dynamic.All += OnEvent;
            _thread = new Thread(() =>
            {
                try { _session.Source.Process(); }
                catch (Exception e) { Error = e.Message; Available = false; }
            }) { IsBackground = true, Name = "etw-frames", Priority = ThreadPriority.BelowNormal };
            _thread.Start();
            Available = true;
        }
        catch (Exception e)
        {
            Error = e.Message;
            Available = false;
            try { _session?.Dispose(); } catch (Exception) { }
            _session = null;
        }
    }

    void OnEvent(TraceEvent e)
    {
        if (_pid == 0 || e.ProcessID != _pid || (int)e.ID != PresentStart || e.ProviderGuid != Dxgi) return;
        lock (_gate)
        {
            _frames.Enqueue(e.TimeStamp.ToUniversalTime());
            while (_frames.Count > Keep) _frames.Dequeue();
        }
    }

    public void Watch(uint pid)
    {
        lock (_gate)
        {
            _pid = pid;
            _frames.Clear();
        }
    }

    public List<double> Intervals(DateTime from, DateTime to)
    {
        var list = new List<double>();
        lock (_gate)
        {
            DateTime prev = default;
            foreach (var t in _frames)
            {
                if (prev != default && t > from && t <= to) list.Add((t - prev).TotalMilliseconds);
                prev = t;
            }
        }
        return list;
    }

    public void Dispose()
    {
        try { _session?.Dispose(); }
        catch (Exception) { }
        _session = null;
        Available = false;
    }
}
