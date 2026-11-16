using Microsoft.Diagnostics.Tracing.Parsers;
using Microsoft.Diagnostics.Tracing.Session;

namespace DomainScheduler;

/// <summary>
/// Live DPC/ISR time per processor from ETW (section 22.9, optional, off by default). A
/// processor is "noisy" when DPC + ISR took more than 2% of it over the last 10 seconds; the
/// mask refinement treats noisy processors like registry pins. Nothing is changed by this class.
/// </summary>
public sealed class EtwIrq : IDisposable
{
    public const int WindowSeconds = 10;
    public const double NoisyShare = 0.02;
    const string SessionName = "DomainScheduler-Irq";

    readonly object _gate = new();
    readonly double[,] _ms = new double[64, WindowSeconds];     // per processor, per second slot
    readonly long[] _slotSecond = new long[WindowSeconds];
    TraceEventSession _session;
    Thread _thread;
    public Func<DateTime> Now = () => DateTime.UtcNow;

    public bool Available { get; private set; }
    public string Error { get; private set; } = "";

    /// <summary>Starts the tracer; false (with one warning) when it cannot run: the registry pins are used alone.</summary>
    public bool Start()
    {
        if (Available) return true;
        if (Flags.Refuse(Flags.EtwIrq, L.T("Live DPC/ISR"))) return false;
        try
        {
            if (TraceEventSession.IsElevated() != true) throw new InvalidOperationException(L.T("no administrator rights"));
            _session = new TraceEventSession(SessionName) { StopOnDispose = true };
            _session.EnableKernelProvider(KernelTraceEventParser.Keywords.DeferedProcedureCalls | KernelTraceEventParser.Keywords.Interrupt);
            _session.Source.Kernel.PerfInfoDPC += d => Add(d.ProcessorNumber, d.ElapsedTimeMSec);
            _session.Source.Kernel.PerfInfoISR += d => Add(d.ProcessorNumber, d.ElapsedTimeMSec);
            var session = _session;
            _thread = new Thread(() =>
            {
                try { session.Source.Process(); }
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

    internal void Add(int cpu, double ms)
    {
        if (cpu < 0 || cpu >= 64 || ms <= 0) return;
        long sec = Now().Ticks / TimeSpan.TicksPerSecond;
        int slot = (int)(sec % WindowSeconds);
        lock (_gate)
        {
            if (_slotSecond[slot] != sec)
            {
                _slotSecond[slot] = sec;
                for (int c = 0; c < 64; c++) _ms[c, slot] = 0;
            }
            _ms[cpu, slot] += ms;
        }
    }

    /// <summary>Mask of the processors whose DPC + ISR share over the last 10 s is above 2%.</summary>
    public ulong Noisy()
    {
        if (!Available) return 0;
        long sec = Now().Ticks / TimeSpan.TicksPerSecond;
        ulong mask = 0;
        lock (_gate)
            for (int c = 0; c < 64; c++)
            {
                double total = 0;
                for (int s = 0; s < WindowSeconds; s++)
                    if (sec - _slotSecond[s] < WindowSeconds) total += _ms[c, s];
                if (total / (WindowSeconds * 1000.0) > NoisyShare) mask |= 1UL << c;
            }
        return mask;
    }

    internal void MarkAvailableForTests() => Available = true;

    void Stop(bool log)
    {
        try { _session?.Dispose(); } catch (Exception) { }
        _session = null;
        if (Available && log) AppLog.Add(L.T("Live DPC/ISR: off"));
        Available = false;
    }

    public void Dispose() => Stop(true);
}
