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

    public static void Add(string msg, Sev sev = Sev.Info)
    {
        if (string.IsNullOrEmpty(msg)) return;
        Interlocked.Increment(ref _version);
        lock (_gate)
        {
            _ring[_next] = new Entry(Now(), sev, msg);
            _next = (_next + 1) % Capacity;
            if (_count < Capacity) _count++;
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
}
