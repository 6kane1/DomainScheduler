using System.Globalization;
using System.Text;

namespace DomainScheduler;

/// <summary>
/// The program log (section 25): a ring buffer of about 2000 entries in memory (the Report reads from here) and the
/// file log.txt in the state folder. A warning or an error is written to the file at once (it is there after a
/// blue screen too); other entries go with it, or at the latest a minute later, and at exit. Each entry is written
/// once: a flush appends only what came since the last one.
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
    /// <summary>Where the file is, for display (never throws).</summary>
    public static string ShownPath => PathHook?.Invoke() ?? Path.Combine(Store.DirShown, "log.txt");
    /// <summary>Entries added since the last flush (not yet in the file).</summary>
    static int _unwritten;
    static DateTime _lastFlush = DateTime.MinValue;
    static bool _headerDone;
    static readonly object _fileGate = new();
    public static readonly TimeSpan FlushEvery = TimeSpan.FromMinutes(1);
    /// <summary>The file is written on Windows, and in tests only when PathHook points somewhere.</summary>
    static bool FileOn => PathHook != null || OperatingSystem.IsWindows();

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
        bool flush;
        lock (_gate)
        {
            _ring[_next] = e;
            _next = (_next + 1) % Capacity;
            if (_count < Capacity) _count++;
            if (_unwritten < Capacity) _unwritten++;
            flush = sev != Sev.Info || e.Utc - _lastFlush >= FlushEvery;
        }
        if (flush) Flush();
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
        lock (_gate) _next = _count = _unwritten = 0;
    }

    /// <summary>Appends the entries that are not in the file yet (each one once). Never throws.</summary>
    public static void Flush()
    {
        if (!FileOn) { lock (_gate) { _unwritten = 0; _lastFlush = Now(); } return; }
        lock (_fileGate)
        {
            List<Entry> fresh;
            lock (_gate)
            {
                int n = Math.Min(_unwritten, _count);
                fresh = new List<Entry>(n);
                for (int i = n; i > 0; i--) fresh.Add(_ring[(_next - i + Capacity) % Capacity]);
                _unwritten = 0;
                _lastFlush = Now();
            }
            if (fresh.Count == 0 && _headerDone) return;
            try
            {
                string path = FilePath;
                Directory.CreateDirectory(Path.GetDirectoryName(path)!);
                var sb = new StringBuilder();
                if (!_headerDone) sb.AppendLine("---- " + DateTime.UtcNow.ToString("o", CultureInfo.InvariantCulture));
                foreach (var e in fresh) sb.AppendLine(e.Utc.ToString("o", CultureInfo.InvariantCulture) + " " + e.Sev + " " + e.Msg);
                File.AppendAllText(path, sb.ToString());
                _headerDone = true;
                var fi = new FileInfo(path);
                if (fi.Length > 4 << 20)   // keep the file small: the last 2 MB
                {
                    var all = File.ReadAllBytes(path);
                    File.WriteAllBytes(path, all[^(2 << 20)..]);
                }
            }
            catch (Exception) { }
        }
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
