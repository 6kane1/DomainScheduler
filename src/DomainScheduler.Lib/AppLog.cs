using System.Globalization;
using System.Text;

namespace DomainScheduler;

/// <summary>
/// The program log (section 25): a ring buffer of about 2000 entries in memory,
/// flushed to a file on exit and on a crash. The Report reads from here.
/// </summary>
public static class AppLog
{
    public enum Sev { Info, Warn, Error }

    public readonly record struct Entry(DateTime Utc, Sev Sev, string Msg)
    {
        public override string ToString() =>
            Utc.ToLocalTime().ToString("HH:mm:ss", CultureInfo.InvariantCulture) + "  " + Sev switch { Sev.Warn => "WARN ", Sev.Error => "ERROR", _ => "INFO " } + "  " + Msg;
    }

    public const int Capacity = 2000;
    static readonly object _gate = new();
    static readonly Entry[] _ring = new Entry[Capacity];
    static int _next, _count;
    public static Func<DateTime> Now = () => DateTime.UtcNow;
    public static Func<string> PathHook;
    public static string FilePath => PathHook?.Invoke() ?? Path.Combine(Store.Dir, "log.txt");

    static long _version;
    /// <summary>Grows with every entry (the log panel redraws only when it changed).</summary>
    public static long Version => Interlocked.Read(ref _version);

    /// <summary>Raised for every error entry (outside the lock): the app shows it in a window, there is no on-screen journal.</summary>
    public static event Action<Entry> ErrorAdded;

    public static void Add(string msg, Sev sev = Sev.Info)
    {
        if (string.IsNullOrEmpty(msg)) return;
        Interlocked.Increment(ref _version);
        Entry e = new Entry(Now(), sev, msg);
        lock (_gate)
        {
            _ring[_next] = e;
            _next = (_next + 1) % Capacity;
            if (_count < Capacity) _count++;
        }
        if (sev == Sev.Error)
        {
            try { ErrorAdded?.Invoke(e); } catch (Exception) { }
        }
    }

    /// <summary>The last n entries, oldest first.</summary>
    public static IReadOnlyList<Entry> Tail(int n)
    {
        lock (_gate)
        {
            n = Math.Min(n, _count);
            var list = new List<Entry>(n);
            for (int i = n; i > 0; i--) list.Add(_ring[(_next - i + Capacity) % Capacity]);
            return list;
        }
    }

    public static void Clear()
    {
        lock (_gate) _next = _count = 0;
    }

    /// <summary>Appends the buffer to the log file (exit, crash). Never throws.</summary>
    public static void Flush()
    {
        try
        {
            var lines = Tail(Capacity).Select(e => e.Utc.ToString("o", CultureInfo.InvariantCulture) + " " + e.Sev + " " + e.Msg);
            Directory.CreateDirectory(Path.GetDirectoryName(FilePath)!);
            var sb = new StringBuilder();
            sb.AppendLine("---- " + DateTime.UtcNow.ToString("o", CultureInfo.InvariantCulture));
            foreach (var l in lines) sb.AppendLine(l);
            File.AppendAllText(FilePath, sb.ToString());
            var fi = new FileInfo(FilePath);
            if (fi.Length > 4 << 20)   // keep the file small: the last 2 MB
            {
                var all = File.ReadAllBytes(FilePath);
                File.WriteAllBytes(FilePath, all[^(2 << 20)..]);
            }
        }
        catch (Exception) { }
    }

    /// <summary>Type, message and inner exceptions (an AggregateException is unpacked) plus the first stack frame of
    /// this program, so an error with an empty message still says what failed and where.</summary>
    public static string Describe(Exception e)
    {
        if (e == null) return "";
        var parts = new List<string>();
        var list = e is AggregateException ag ? ag.Flatten().InnerExceptions.ToList() : new List<Exception> { e };
        foreach (var x in list.Take(3))
        {
            var cur = x;
            var chain = new List<string>();
            for (int d = 0; cur != null && d < 3; d++, cur = cur.InnerException)
                chain.Add(cur.GetType().Name + (string.IsNullOrWhiteSpace(cur.Message) ? "" : ": " + cur.Message));
            string at = (x.StackTrace ?? "").Split('\n').Select(l => l.Trim()).FirstOrDefault(l => l.Contains("DomainScheduler")) ?? "";
            parts.Add(string.Join(" ← ", chain) + (at.Length > 0 ? " [" + at + "]" : ""));
        }
        return string.Join(" | ", parts);
    }
}
