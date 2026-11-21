using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>
/// Own load per thread (1.16.0 Test 1): the program's long-lived threads register themselves by name, and once a minute
/// the processor time each one used is read with GetThreadTimes (query rights on our own threads only). The About page
/// and the log then say where the program's own load goes (window, worker, DPC/ISR trace, frame trace), so it is cut
/// where it is measured, not guessed.
/// </summary>
public static class SelfThreads
{
    static readonly object Gate = new();
    static readonly Dictionary<string, uint> Tids = new(StringComparer.Ordinal);
    static readonly Dictionary<string, (uint Tid, long Time)> Last = new(StringComparer.Ordinal);

    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();

    /// <summary>Call on the thread itself; a new thread under the same name replaces the old one.</summary>
    public static void Register(string name)
    {
        uint tid = GetCurrentThreadId();
        lock (Gate) Tids[name] = tid;
    }

    /// <summary>Processor time (100 ns units, kernel + user) of each registered thread now; a thread that ended is left out.</summary>
    public static Dictionary<string, (uint Tid, long Time)> Now()
    {
        KeyValuePair<string, uint>[] list;
        lock (Gate) list = Tids.ToArray();
        var now = new Dictionary<string, (uint, long)>(StringComparer.Ordinal);
        foreach (var (name, tid) in list)
        {
            IntPtr h = Native.OpenThread(0x0800 /* THREAD_QUERY_LIMITED_INFORMATION */, false, tid);
            if (h == IntPtr.Zero) continue;
            try { if (Native.GetThreadTimes(h, out _, out _, out long k, out long u)) now[name] = (tid, k + u); }
            finally { Native.CloseHandle(h); }
        }
        return now;
    }

    /// <summary>
    /// Share of one processor each thread used since the previous call (0..1), from two readings over seconds.
    /// A thread that was replaced (another id under the same name) counts from zero: it started within the window.
    /// </summary>
    public static Dictionary<string, double> Shares(Dictionary<string, (uint Tid, long Time)> now, double seconds)
    {
        var shares = new Dictionary<string, double>(StringComparer.Ordinal);
        lock (Gate)
        {
            if (seconds > 0)
                foreach (var (name, v) in now)
                {
                    long before = Last.TryGetValue(name, out var l) && l.Tid == v.Tid ? l.Time : 0;
                    if (Last.Count > 0 || before > 0) shares[name] = Math.Max(0, v.Time - before) / 1e7 / seconds;
                }
            Last.Clear();
            foreach (var kv in now) Last[kv.Key] = kv.Value;
        }
        return shares;
    }
}
