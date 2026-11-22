using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// The black box (1.16.1 Test 1): a flight recorder for the game. While Auto handles a game, Windows' own kernel trace
/// (context switches, ready threads, DPC/ISR) and the DXGI present events are kept in memory rings for the last
/// ~30 s; nothing is written to disk until a stutter happens. A stutter is a frame longer than 3x the median and
/// longer than two refreshes of the monitor. Then the frames and thread waits around it are explained in plain words,
/// with the same rules as Domain SchedTest's episode causes:
///   other program - a main game thread (priority 8+) waited &gt; 100 us for a core held by another program;
///   driver        - a DPC/ISR &gt; 100 us ran in the frame on a processor a main game thread used in that frame;
///   cores         - a main game thread waited &gt; 100 us for a core held by the game's own thread;
///   game or GPU   - none of the above: the processor side was clean, the time went to the engine or the graphics card.
/// The report goes to the Black box page and to a small JSON file (last 50 kept). Nothing in the game or the system is
/// changed: the trace only reads.
/// </summary>
public static class BlackBoxPolicy
{
    public const double HitchFactor = 3.0;
    public const double MainPriority = 8, MainWaitUs = 100, DpcUs = 100, KeepDpcUs = 50;
    public const int MinGapMs = 3000;          // at most one report per 3 s (a burst of bad frames is one stutter)
    public const int AfterMs = 300;            // the analysis waits this long so the waits that end after the frame are in
    public const int KeepReports = 50;
    /// <summary>A frame this long is a load, a map change or the game window losing focus, not a stutter in play (Test 2).</summary>
    public const double LoadingMs = 100;
    /// <summary>Windows' own processes: never "close it" (closing them restarts or breaks Windows).</summary>
    public static readonly HashSet<string> SystemProcesses = new(StringComparer.OrdinalIgnoreCase)
    {
        "System", "Registry", "Secure System", "Memory Compression", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe",
        "lsass.exe", "lsaiso.exe", "svchost.exe", "dwm.exe", "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "NisSrv.exe",
        "MpDefenderCoreService.exe", "SecurityHealthService.exe", "SearchIndexer.exe", "spoolsv.exe", "taskhostw.exe", "WmiPrvSE.exe",
        "TiWorker.exe", "TrustedInstaller.exe", "conhost.exe", "sihost.exe", "ctfmon.exe", "explorer.exe", "RuntimeBroker.exe", "dllhost.exe",
    };

    /// <summary>A frame counts as a stutter: longer than 3x the median and than two refreshes (or 8 ms if the refresh is unknown).</summary>
    public static bool IsStutter(double frameMs, double medianMs, double refreshHz) =>
        medianMs > 0 && frameMs > HitchFactor * medianMs && frameMs > (refreshHz > 0 ? 2000.0 / refreshHz : 8.0);
}

public readonly record struct BbWait(long Run, long Ticks, ushort Cpu, uint Tid, byte Priority, uint OldTid, uint OldPid);
public readonly record struct BbDpc(ushort Cpu, long Start, long End, ulong Routine);

public enum BbCause { Game, Cores, Driver, Other, Loading }

/// <summary>One explained stutter.</summary>
public sealed record BbReport(DateTime When, string Game, double FrameMs, double MedianMs, int BadFrames, BbCause Cause,
    string Who, double WaitUs, double DpcUs, int MainWaits, string TextRu, string TextEn, string AdviceRu, string AdviceEn);

/// <summary>A fixed-size ring (overwrites the oldest). Not thread-safe by itself: the trace holds one lock.</summary>
public sealed class Ring<T> where T : struct
{
    readonly T[] _a; int _next; bool _full;
    public Ring(int capacity) { _a = new T[capacity]; }
    public int Count => _full ? _a.Length : _next;
    public void Add(T x) { _a[_next] = x; if (++_next == _a.Length) { _next = 0; _full = true; } }
    public void Clear() { _next = 0; _full = false; }
    /// <summary>The items in the order they were added.</summary>
    public List<T> ToList()
    {
        var l = new List<T>(Count);
        if (_full) for (int i = _next; i < _a.Length; i++) l.Add(_a[i]);
        for (int i = 0; i < _next; i++) l.Add(_a[i]);
        return l;
    }
}

/// <summary>Finds stutters in the present times and explains them (pure: tests drive it with synthetic data).</summary>
public static class BlackBoxAnalyzer
{
    public static double Median(IReadOnlyList<double> ms)
    {
        if (ms.Count == 0) return 0;
        var s = ms.OrderBy(x => x).ToArray();
        return s[s.Length / 2];
    }

    /// <summary>
    /// Explains the stutter that ends at present index <paramref name="hitch"/>: frames are the present times (ticks),
    /// waits the game's thread waits, dpcs the long DPC/ISRs, gamePids the game's process ids.
    /// </summary>
    public static BbReport Explain(string game, DateTime when, IReadOnlyList<long> presents, int hitch, double ticksPerMs,
        IReadOnlyList<BbWait> waits, IReadOnlyList<BbDpc> dpcs, ICollection<uint> gamePids, Func<uint, string> nameOf,
        Func<ulong, string> driverOf, double refreshHz)
    {
        var ms = new List<double>();
        for (int i = 1; i < presents.Count; i++) ms.Add((presents[i] - presents[i - 1]) / ticksPerMs);
        double median = Median(ms);
        // the bad frames: this one and the stutters within 100 ms before it
        var bad = new List<int>();
        for (int i = hitch; i >= 1; i--)
        {
            if ((presents[hitch] - presents[i]) / ticksPerMs > 100) break;
            if (BlackBoxPolicy.IsStutter(ms[i - 1], median, refreshHz) || i == hitch) bad.Add(i);
        }
        double worst = bad.Max(i => ms[i - 1]);
        bool other = false, driver = false, cores = false;
        string who = "", drv = "";
        double waitUs = 0, dpcUs = 0; int mainWaits = 0;
        double usPerTick = 1000.0 / ticksPerMs;
        var main = waits.Where(w => w.Priority >= BlackBoxPolicy.MainPriority).ToList();
        foreach (int i in bad)
        {
            long b = presents[i - 1], e = presents[i];
            var cpus = new HashSet<ushort>();
            foreach (var w in main)
            {
                if (w.Run >= b && w.Run <= e) cpus.Add(w.Cpu);
                double us = w.Ticks * usPerTick;
                if (us <= BlackBoxPolicy.MainWaitUs || w.Run - w.Ticks > e || w.Run < b) continue;
                mainWaits++;
                bool ownerGame = gamePids.Contains(w.OldPid);
                if (w.OldTid == 0 || w.OldPid == 0) continue;            // the core was idle: woken from sleep
                if (!ownerGame) { if (us > waitUs || !other) { who = nameOf(w.OldPid); } other = true; waitUs = Math.Max(waitUs, us); }
                else { cores = true; if (!other) waitUs = Math.Max(waitUs, us); }
            }
            foreach (var d in dpcs)
            {
                double us = (d.End - d.Start) * usPerTick;
                if (us <= BlackBoxPolicy.DpcUs || d.Start > e || d.End < b || !cpus.Contains(d.Cpu)) continue;
                if (us > dpcUs) { dpcUs = us; drv = driverOf(d.Routine); }
                driver = true;
            }
        }
        var cause = worst >= BlackBoxPolicy.LoadingMs ? BbCause.Loading : other ? BbCause.Other : driver ? BbCause.Driver : cores ? BbCause.Cores : BbCause.Game;
        bool sys = BlackBoxPolicy.SystemProcesses.Contains(who);
        var ci = CultureInfo.InvariantCulture;
        string head = string.Format(ci, "{0:0.0}", worst), med = string.Format(ci, "{0:0.0}", median);
        (string ru, string en, string aru, string aen) = cause switch
        {
            BbCause.Loading => (
                $"Кадр шёл {head} мс. Так долго игра стоит при загрузке, смене карты, подключении к серверу или когда окно игры теряет фокус: это не рывок в бою.",
                $"The frame took {head} ms. A game stops this long while loading, changing the map, connecting to a server or when its window loses focus: not a stutter in play.",
                "Ничего делать не нужно. Если такие паузы случаются посреди раунда, пришлите файл этого рывка.",
                "Nothing to do. If such pauses happen in the middle of a round, send the file of this stutter."),
            BbCause.Other when sys => (
                $"Кадр шёл {head} мс (обычно {med} мс). Главный поток игры {Us(waitUs)} ждал ядро, которое занимал системный процесс Windows: {who}.",
                $"The frame took {head} ms (usually {med} ms). A main game thread waited {UsEn(waitUs)} for a core held by a Windows process: {who}.",
                $"{who} это часть Windows, закрывать его нельзя. Разовый всплеск бывает при входе, проверке безопасности или обслуживании. Если он повторяется в каждой игре, пришлите файлы рывков: разберём, что Windows делала в это время.",
                $"{who} is part of Windows and must not be closed. A one-off burst happens at sign-in, a security check or maintenance. If it repeats in every game, send the stutter files and we will look at what Windows was doing."),
            BbCause.Other => (
                $"Кадр шёл {head} мс (обычно {med} мс). Главный поток игры {Us(waitUs)} ждал ядро, которое занимала другая программа: {who}.",
                $"The frame took {head} ms (usually {med} ms). A main game thread waited {UsEn(waitUs)} for a core held by another program: {who}.",
                $"Закройте {who} на время игры или проверьте, что он не делает работу в фоне (обновления, синхронизация, запись).",
                $"Close {who} while you play, or check that it does no background work (updates, sync, recording)."),
            BbCause.Driver => (
                $"Кадр шёл {head} мс (обычно {med} мс). На ядре главного потока игры драйвер {drv} работал {Us(dpcUs)} подряд, и игра ждала.",
                $"The frame took {head} ms (usually {med} ms). On a core of the game's main thread the driver {drv} ran for {UsEn(dpcUs)} in one go and the game waited.",
                DriverAdviceRu(drv), DriverAdviceEn(drv)),
            BbCause.Cores => (
                $"Кадр шёл {head} мс (обычно {med} мс). Главный поток игры {Us(waitUs)} ждал ядро, занятое другими потоками самой игры: в этот момент игре не хватило свободных ядер.",
                $"The frame took {head} ms (usually {med} ms). A main game thread waited {UsEn(waitUs)} for a core held by the game's own threads: the game ran out of free cores at that moment.",
                "Если у Toolbox включена маска ядер, проверьте замером вариант без неё (Настройки, маска auto или off): на 8 ядрах всплески задач игры могут упираться в маску.",
                "If Toolbox's core mask is on, measure the game without it (Settings, mask auto or off): on 8 cores the game's job bursts can hit the mask."),
            _ => (
                $"Кадр шёл {head} мс (обычно {med} мс). Со стороны процессора всё было чисто: главные потоки игры не ждали ядер, драйверы не мешали. Время ушло на работу самой игры или видеокарты.",
                $"The frame took {head} ms (usually {med} ms). The processor side was clean: the game's main threads did not wait for cores and no driver got in the way. The time went to the game itself or the graphics card.",
                "Это не система: так бывает при подгрузке, взрывах, дымах, смене сцены. Если такие рывки частые, проверьте температуру и частоты видеокарты и настройки графики.",
                "Not the system: this happens on loading, explosions, smokes, scene changes. If they are frequent, check the graphics card's temperature and clocks and the graphics settings."),
        };
        return new BbReport(when, game, worst, median, bad.Count, cause, cause == BbCause.Driver ? drv : who, waitUs, dpcUs, mainWaits, ru, en, aru, aen);

        static string Us(double us) => us >= 1000 ? string.Format(CultureInfo.InvariantCulture, "{0:0.0} мс", us / 1000) : string.Format(CultureInfo.InvariantCulture, "{0:0} мкс", us);
        static string UsEn(double us) => us >= 1000 ? string.Format(CultureInfo.InvariantCulture, "{0:0.0} ms", us / 1000) : string.Format(CultureInfo.InvariantCulture, "{0:0} µs", us);
    }

    static bool Gpu(string d) => d.StartsWith("nvlddmkm", StringComparison.OrdinalIgnoreCase) || d.StartsWith("amdkmdag", StringComparison.OrdinalIgnoreCase)
                                 || d.StartsWith("atikmdag", StringComparison.OrdinalIgnoreCase) || d.StartsWith("igdkmd", StringComparison.OrdinalIgnoreCase) || d.StartsWith("dxgkrnl", StringComparison.OrdinalIgnoreCase);
    static bool Net(string d) => d.StartsWith("ndis", StringComparison.OrdinalIgnoreCase) || d.StartsWith("tcpip", StringComparison.OrdinalIgnoreCase) || d.Contains("rt640", StringComparison.OrdinalIgnoreCase) || d.Contains("e1d", StringComparison.OrdinalIgnoreCase) || d.Contains("netwtw", StringComparison.OrdinalIgnoreCase);
    static bool Usb(string d) => d.StartsWith("usb", StringComparison.OrdinalIgnoreCase) || d.StartsWith("hidclass", StringComparison.OrdinalIgnoreCase) || d.StartsWith("Wdf01000", StringComparison.OrdinalIgnoreCase);
    static string DriverAdviceRu(string d) =>
        Gpu(d) ? "Драйвер видеокарты: поставьте чистую последнюю версию драйвера. Toolbox уже старается держать игру подальше от ядер этого драйвера (живые DPC/ISR в настройках)."
        : Net(d) ? "Сетевой драйвер: обновите драйвер сетевой карты с сайта производителя платы; на Wi-Fi такие задержки бывают чаще, чем по кабелю."
        : Usb(d) ? "Драйвер USB: подключите мышь в порт прямо на материнской плате, без хабов; уберите лишние USB-устройства, обновите драйвер чипсета."
        : "Обновите этот драйвер с сайта производителя устройства или платы; если устройство не нужно для игры, его можно отключить в диспетчере устройств.";
    static string DriverAdviceEn(string d) =>
        Gpu(d) ? "Graphics driver: install the latest driver cleanly. Toolbox already keeps the game away from this driver's cores (live DPC/ISR in Settings)."
        : Net(d) ? "Network driver: update the network card driver from the board maker's site; Wi-Fi has such delays more often than a cable."
        : Usb(d) ? "USB driver: plug the mouse straight into the motherboard, without hubs; remove extra USB devices, update the chipset driver."
        : "Update this driver from the device or board maker's site; if the device is not needed for the game, you can disable it in Device Manager.";
}

/// <summary>
/// The recorder: one private kernel session (process, thread, context switch, dispatcher, DPC, interrupt) and one DXGI
/// session, both read-only, on the QPC clock. Waits of the game's threads, long DPC/ISRs and the game's presents go to
/// rings; every present is checked for a stutter, and a found stutter is explained 300 ms later on the caller's tick.
/// </summary>
public sealed class BlackBox : IDisposable
{
    readonly object _gate = new();
    readonly Ring<long> _presents = new(1 << 16);            // ~30 s at 2000 FPS
    readonly Ring<BbWait> _waits = new(1 << 19);              // the game's thread waits
    readonly Ring<BbDpc> _dpcs = new(1 << 14);                // DPC/ISR >= 50 us, any processor
    readonly Dictionary<uint, uint> _tidPid = new();
    readonly HashSet<uint> _gameTids = new();
    readonly Dictionary<uint, long> _ready = new();
    readonly List<double> _recent = new();                    // recent frame times for the running median
    double _median; int _sinceMedian;
    long _lastPresent, _lastReport, _pendingAt; int _pending = -1;
    uint _pid; string _game = "";
    readonly double _tpm = Stopwatch.Frequency / 1000.0;
    List<(ulong Base, string Name)> _drivers = new();
    readonly List<BbReport> _reports = new();
    public double RefreshHz = 0;
    public string Folder;                                    // where report files go (null: not written)
    public bool Running { get; private set; }
    public string Error { get; private set; } = "";
    public uint GamePid => _pid;

    public IReadOnlyList<BbReport> Reports { get { lock (_gate) return _reports.ToList(); } }

    public void Start(uint pid, string game)
    {
        if (Running && pid == _pid) return;
        Stop();
        lock (_gate)
        {
            _pid = pid; _game = game; _presents.Clear(); _waits.Clear(); _dpcs.Clear(); _ready.Clear(); _gameTids.Clear(); _tidPid.Clear();
            _recent.Clear(); _median = 0; _lastPresent = 0; _pending = -1; _lastReport = 0;
        }
        try { _drivers = Drivers(); } catch (Exception) { _drivers = new(); }
        Running = StartSessions();
        if (Running) AppLog.Add(L.F("Black box: recording {0}", game));
    }

    public void Stop()
    {
        if (!Running) return;
        Running = false;
        StopSessions();
        AppLog.Add(L.T("Black box: stopped"));
    }

    public void Dispose() => Stop();

    /// <summary>The refresh rate of the main monitor (Hz), 0 when Windows does not say.</summary>
    public static double MonitorHz()
    {
        try
        {
            var dm = new byte[220];
            BitConverter.GetBytes((ushort)220).CopyTo(dm, 68);                  // DEVMODEW.dmSize
            if (!EnumDisplaySettingsW(null, -1 /* ENUM_CURRENT_SETTINGS */, dm)) return 0;
            uint hz = BitConverter.ToUInt32(dm, 184);                           // dmDisplayFrequency
            return hz is > 1 and < 1000 ? hz : 0;
        }
        catch (Exception) { return 0; }
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool EnumDisplaySettingsW(string device, int mode, [In, Out] byte[] devMode);

    // ---- events (called on the trace threads) ------------------------------------------------------------------
    internal void OnThread(uint pid, uint tid) { lock (_gate) { _tidPid[tid] = pid; if (pid == _pid) _gameTids.Add(tid); } }
    internal void OnReady(uint tid, long ts) { lock (_gate) if (_gameTids.Contains(tid)) _ready.TryAdd(tid, ts); }
    internal void OnSwitch(long ts, ushort cpu, uint newTid, byte newPrio, uint oldTid)
    {
        lock (_gate)
            if (newTid != 0 && _ready.Remove(newTid, out long at) && ts >= at)
                _waits.Add(new BbWait(ts, ts - at, cpu, newTid, newPrio, oldTid, oldTid == 0 ? 0 : _tidPid.GetValueOrDefault(oldTid)));
    }
    internal void OnDpc(ushort cpu, long start, long end, ulong routine)
    {
        if ((end - start) / _tpm * 1000 < BlackBoxPolicy.KeepDpcUs) return;
        lock (_gate) _dpcs.Add(new BbDpc(cpu, start, end, routine));
    }
    internal void OnPresent(uint pid, long ts)
    {
        lock (_gate)
        {
            if (pid != _pid) return;
            _presents.Add(ts);
            if (_lastPresent != 0)
            {
                double ms = (ts - _lastPresent) / _tpm;
                _recent.Add(ms); if (_recent.Count > 600) _recent.RemoveAt(0);
                if (++_sinceMedian >= 120 || _median == 0) { _median = BlackBoxAnalyzer.Median(_recent); _sinceMedian = 0; }
                if (_recent.Count >= 120 && _pending < 0 && BlackBoxPolicy.IsStutter(ms, _median, RefreshHz)
                    && (ts - _lastReport) / _tpm > BlackBoxPolicy.MinGapMs)
                { _pending = 1; _pendingAt = ts; _lastReport = ts; }
            }
            _lastPresent = ts;
        }
    }

    /// <summary>Called about every 100-1000 ms by the hub: explains a stutter once its trailing data is in.</summary>
    public BbReport Tick()
    {
        List<long> p; List<BbWait> w; List<BbDpc> d; long at; HashSet<uint> pids;
        lock (_gate)
        {
            if (_pending < 0 || (Stopwatch.GetTimestamp() - _pendingAt) / _tpm < BlackBoxPolicy.AfterMs) return null;
            _pending = -1; at = _pendingAt;
            long from = at - (long)(2000 * _tpm), to = at + (long)(500 * _tpm);
            p = _presents.ToList().Where(x => x >= from && x <= to).ToList();
            w = _waits.ToList().Where(x => x.Run >= from && x.Run <= to).ToList();
            d = _dpcs.ToList().Where(x => x.End >= from && x.Start <= to).ToList();
            pids = new HashSet<uint> { _pid };
        }
        int hitch = p.IndexOf(at);
        if (hitch < 1) return null;
        DateTime when = DateTime.Now - TimeSpan.FromMilliseconds((Stopwatch.GetTimestamp() - at) / _tpm);
        var r = BlackBoxAnalyzer.Explain(_game, when, p, hitch, _tpm, w, d, pids, NameOf, DriverOf, RefreshHz);
        lock (_gate) { _reports.Insert(0, r); if (_reports.Count > BlackBoxPolicy.KeepReports) _reports.RemoveAt(_reports.Count - 1); }
        Save(r, p, at);
        return r;
    }

    void Save(BbReport r, List<long> presents, long at)
    {
        if (Folder == null) return;
        try
        {
            Directory.CreateDirectory(Folder);
            var frames = new List<double>();
            for (int i = 1; i < presents.Count; i++) frames.Add(Math.Round((presents[i] - presents[i - 1]) / _tpm, 3));
            string f = Path.Combine(Folder, r.When.ToString("yyyyMMdd-HHmmss-fff", CultureInfo.InvariantCulture) + ".json");
            File.WriteAllText(f, JsonSerializer.Serialize(new { r.When, r.Game, r.FrameMs, r.MedianMs, r.BadFrames, Cause = r.Cause.ToString(), r.Who, r.WaitUs, r.DpcUs, r.MainWaits, r.TextRu, r.TextEn, r.AdviceRu, r.AdviceEn, FramesMs = frames }));
            foreach (var old in Directory.GetFiles(Folder, "*.json").OrderByDescending(x => x).Skip(BlackBoxPolicy.KeepReports)) File.Delete(old);
        }
        catch (Exception) { }
    }

    static readonly Dictionary<uint, string> _names = new();
    static string NameOf(uint pid)
    {
        lock (_names)
        {
            if (_names.TryGetValue(pid, out var n)) return n;
            try { using var p = Process.GetProcessById((int)pid); n = p.ProcessName + ".exe"; } catch (Exception) { n = pid == 4 ? "System" : $"PID {pid}"; }
            if (_names.Count > 4000) _names.Clear();
            return _names[pid] = n;
        }
    }

    string DriverOf(ulong addr)
    {
        string best = "?"; ulong bestBase = 0;
        foreach (var (b, n) in _drivers) if (b <= addr && b >= bestBase) { bestBase = b; best = n; }
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

    // ---- raw ETW --------------------------------------------------------------------------------------------------
    const string KernelName = "DomainScheduler-BlackBox", FramesName = "DomainScheduler-BlackBoxFrames";
    static readonly Guid KernelGuid = new("d4a1f6c2-58b3-4e7a-a0c9-2f6b8e1d7c35"), FramesGuid = new("7c2e9b14-a6d3-4f58-9e01-b3c84d2f6a97");
    static readonly byte[] ThreadG = new Guid("3d6fa8d1-fe05-11d0-9dda-00c04fd7ba7c").ToByteArray();
    static readonly byte[] PerfInfoG = new Guid("ce1dbfb4-137e-4da6-87b0-3f59aa102cbc").ToByteArray();
    static readonly byte[] DxgiG = new Guid("CA11C036-0102-4A2D-A6AD-F03CFED5D3C9").ToByteArray();
    // PROCESS | THREAD | CSWITCH | DPC | INTERRUPT | DISPATCHER
    const uint KFlags = 0x1 | 0x2 | 0x10 | 0x20 | 0x40 | 0x800;
    ulong _kS, _kT, _fS, _fT; Thread _kTh, _fTh, _flush; EventRecordCallback _kCb, _fCb;

    bool StartSessions()
    {
        try
        {
            StopByName(KernelName); StopByName(FramesName);
            uint rc = StartSession(KernelName, KernelGuid, 0x100 | 0x02000000, 1024, 64, 512, out _kS);
            if (rc != 0) { Error = rc == 5 ? L.T("no administrator rights") : L.F("the kernel trace did not start (error {0})", rc); return false; }
            IntPtr fl = Marshal.AllocHGlobal(4); Marshal.WriteInt32(fl, (int)KFlags);
            uint r2 = TraceSetInformation(_kS, 4, fl, 4); Marshal.FreeHGlobal(fl);
            if (r2 != 0) { StopByName(KernelName); Error = L.F("the kernel trace did not start (error {0})", r2); return false; }
            _kCb = OnKernel; _kT = Open(KernelName, _kCb);
            if (StartSession(FramesName, FramesGuid, 0x100, 64, 16, 128, out _fS) == 0)
            {
                var g = new Guid("CA11C036-0102-4A2D-A6AD-F03CFED5D3C9");
                EnableTraceEx2(_fS, ref g, 1, 4, ulong.MaxValue, 0, 0, IntPtr.Zero);
                _fCb = OnFrames; _fT = Open(FramesName, _fCb);
            }
            ulong kt = _kT, ft = _fT;
            _kTh = new Thread(() => { SelfThreads.Register(L.N("Black box trace")); try { ProcessTrace(new[] { kt }, 1, IntPtr.Zero, IntPtr.Zero); } catch (Exception) { } }) { IsBackground = true, Name = "blackbox-k", Priority = ThreadPriority.BelowNormal };
            _kTh.Start();
            if (ft != 0 && ft != ulong.MaxValue)
            {
                _fTh = new Thread(() => { try { ProcessTrace(new[] { ft }, 1, IntPtr.Zero, IntPtr.Zero); } catch (Exception) { } }) { IsBackground = true, Name = "blackbox-f", Priority = ThreadPriority.BelowNormal };
                _fTh.Start();
            }
            _flush = new Thread(() => { while (Running) { Thread.Sleep(250); try { Flush(FramesName); } catch (Exception) { } } }) { IsBackground = true, Name = "blackbox-flush" };
            Running = true; _flush.Start();
            return true;
        }
        catch (Exception e) { Error = e.Message; StopByName(KernelName); StopByName(FramesName); return false; }
    }

    void StopSessions()
    {
        StopByName(KernelName); StopByName(FramesName);
        try { if (_kT != 0 && _kT != ulong.MaxValue) CloseTrace(_kT); if (_fT != 0 && _fT != ulong.MaxValue) CloseTrace(_fT); } catch (Exception) { }
        _kTh?.Join(2000); _fTh?.Join(2000);
        _kT = _fT = 0;
    }

    /// <summary>A session left by a killed copy of the program is stopped (called once at start).</summary>
    public static void StopOrphan() { StopByName(KernelName); StopByName(FramesName); }

    unsafe void OnKernel(IntPtr record)
    {
        byte* r = (byte*)record;
        byte op = r[45]; ushort len = *(ushort*)(r + 86); byte* u = *(byte**)(r + 96);
        if (u == null) return;
        long ts = *(long*)(r + 16);
        ushort flags = *(ushort*)(r + 4);
        ushort cpu = (flags & 0x200) != 0 ? *(ushort*)(r + 80) : *(r + 80);
        bool thread = true, perf = true;
        for (int i = 0; i < 16; i++) { if (r[24 + i] != ThreadG[i]) thread = false; if (r[24 + i] != PerfInfoG[i]) perf = false; }
        if (thread)
        {
            switch (op)
            {
                case 1: case 3: if (len >= 8) OnThread(*(uint*)u, *(uint*)(u + 4)); return;                 // Start / DCStart: ProcessId, TThreadId
                case 50: if (len >= 4) OnReady(*(uint*)u, ts); return;                                       // ReadyThread
                case 36: if (len >= 10) OnSwitch(ts, cpu, *(uint*)u, *(u + 8), *(uint*)(u + 4)); return;    // CSwitch: New, Old, NewPrio
            }
            return;
        }
        if (perf && (op == 66 || op == 67 || op == 68 || op == 69 || op == 70 || op == 50) && len >= 16)
            OnDpc(cpu, *(long*)u, ts, *(ulong*)(u + 8));                                                  // InitialTime, Routine
    }

    unsafe void OnFrames(IntPtr record)
    {
        byte* r = (byte*)record;
        for (int i = 0; i < 16; i++) if (r[24 + i] != DxgiG[i]) return;
        if (*(ushort*)(r + 40) != 42) return;
        OnPresent(*(uint*)(r + 12), *(long*)(r + 16));
    }

    static uint StartSession(string name, Guid guid, int mode, int bufKb, int min, int max, out ulong handle)
    {
        int size = 120 + 2048; IntPtr p = Props(size);
        try
        {
            Marshal.Copy(guid.ToByteArray(), 0, p + 24, 16);
            Marshal.WriteInt32(p, 40, 1); Marshal.WriteInt32(p, 44, 0x20000);
            Marshal.WriteInt32(p, 48, bufKb); Marshal.WriteInt32(p, 52, min); Marshal.WriteInt32(p, 56, max);
            Marshal.WriteInt32(p, 64, mode); Marshal.WriteInt32(p, 68, 1);
            return StartTraceW(out handle, name, p);
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    static ulong Open(string name, EventRecordCallback cb)
    {
        var lf = new EVENT_TRACE_LOGFILEW { LoggerName = Marshal.StringToHGlobalUni(name), ProcessTraceMode = 0x100 | 0x10000000 | 0x1000, EventRecordCallback = Marshal.GetFunctionPointerForDelegate(cb) };
        return OpenTraceW(ref lf);
    }

    static IntPtr Props(int size)
    {
        IntPtr p = Marshal.AllocHGlobal(size);
        for (int i = 0; i < size; i++) Marshal.WriteByte(p, i, 0);
        Marshal.WriteInt32(p, 0, size); Marshal.WriteInt32(p, 116, 120);
        return p;
    }

    static void Flush(string name) { IntPtr p = Props(120 + 2048); try { ControlTraceW(0, name, p, 3); } finally { Marshal.FreeHGlobal(p); } }
    static void StopByName(string name) { IntPtr p = Props(120 + 2048); try { ControlTraceW(0, name, p, 1); } finally { Marshal.FreeHGlobal(p); } }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)] delegate void EventRecordCallback(IntPtr record);
    [StructLayout(LayoutKind.Explicit, Size = 448)]
    struct EVENT_TRACE_LOGFILEW { [FieldOffset(8)] public IntPtr LoggerName; [FieldOffset(28)] public uint ProcessTraceMode; [FieldOffset(424)] public IntPtr EventRecordCallback; }
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern uint StartTraceW(out ulong handle, string name, IntPtr props);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern uint ControlTraceW(ulong handle, string name, IntPtr props, uint code);
    [DllImport("advapi32.dll")] static extern uint TraceSetInformation(ulong handle, int cls, IntPtr info, uint len);
    [DllImport("advapi32.dll")] static extern uint EnableTraceEx2(ulong handle, ref Guid provider, uint control, byte level, ulong any, ulong all, uint timeout, IntPtr parameters);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)] static extern ulong OpenTraceW(ref EVENT_TRACE_LOGFILEW logfile);
    [DllImport("advapi32.dll")] static extern uint ProcessTrace(ulong[] handles, uint count, IntPtr start, IntPtr end);
    [DllImport("advapi32.dll")] static extern uint CloseTrace(ulong handle);
    [DllImport("psapi.dll", SetLastError = true)] static extern bool EnumDeviceDrivers([Out] IntPtr[] images, uint cb, out uint needed);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode)] static extern uint GetDeviceDriverBaseNameW(IntPtr image, [Out] char[] name, uint size);
}
