using System.Globalization;
using System.Text;

namespace DomainScheduler;

/// <summary>What the Auto mode needs to know about running programs.</summary>
public interface IGameProbe
{
    uint SelfPid { get; }
    uint ForegroundPid();
    /// <summary>Processes owning a visible, unowned top-level window (a main window).</summary>
    HashSet<uint> WindowPids();
    /// <summary>d3d11/d3d12/vulkan-1/opengl32 is loaded (opens the process for reading).</summary>
    bool HasGraphicsModule(uint pid);
    bool CpuTime(uint pid, out long created, out long cpu100ns);
    uint SessionOf(uint pid);
}

/// <summary>Frame times of one process (PresentMon or ETW); Available = false when none can be measured.</summary>
public interface IFrameSource
{
    bool Available { get; }
    string Name { get; }
    string Error { get; }
    void Watch(uint pid);
    /// <summary>Frame intervals in ms of frames that ended between from and to (UTC).</summary>
    List<double> Intervals(DateTime from, DateTime to);
}

public sealed class NoFrames : IFrameSource
{
    public bool Available => false;
    public string Name => "";
    public string Error { get; init; } = "";
    public void Watch(uint pid) { }
    public List<double> Intervals(DateTime from, DateTime to) => new();
}

public enum AutoState { Idle, Armed, Active, Reverting, Faulted }

/// <summary>
/// The one-button mode (sections 12 and 17): the button arms the tool, a game is detected
/// with hysteresis, laid out, watched, and everything is reverted on the button, exit or a
/// watchdog trigger. All transitions run under one lock and never re-enter.
/// </summary>
public sealed class AutoController
{
    static readonly HashSet<string> SystemDeny = new(StringComparer.OrdinalIgnoreCase)
    {
        "System", "Registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "dwm.exe",
        "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "SearchIndexer.exe", "spoolsv.exe", "taskhostw.exe",
    };

    public static bool IsSystem(string exe) => SystemDeny.Contains(exe ?? "");

    static readonly HashSet<string> LauncherDeny = new(StringComparer.OrdinalIgnoreCase)
    {
        "steam.exe", "steamwebhelper.exe", "EpicGamesLauncher.exe", "EpicWebHelper.exe", "Battle.net.exe", "RiotClientServices.exe",
        "RiotClientUx.exe", "EADesktop.exe", "Origin.exe", "upc.exe", "UbisoftConnect.exe", "GalaxyClient.exe", "explorer.exe",
        "ShellExperienceHost.exe", "StartMenuExperienceHost.exe", "SearchHost.exe", "TextInputHost.exe", "ApplicationFrameHost.exe",
    };

    readonly object _lock = new();
    readonly Engine _e;
    readonly IGameProbe _probe;
    readonly IFrameSource _frames;
    readonly Func<DateTime> _now;
    bool _busy;

    public AutoState State { get; private set; } = AutoState.Idle;
    public string Status { get; private set; } = L.T("Off");
    public event Action Changed;
    /// <summary>Session file writes (crash safety); replaced in tests.</summary>
    public Action<SessionData> SaveSession = SessionFile.Write;
    /// <summary>The autostart line of the Report (the Task Scheduler is read only when the Report is built).</summary>
    public Func<string> AutostartLine = Autostart.ReportLine;
    /// <summary>Problems SelfTest found at start (empty = OK).</summary>
    public List<string> SelfTestProblems = new();
    /// <summary>Test hook: lets a test alter the mask plan (e.g. make it unsatisfiable).</summary>
    internal Func<AutoLogic.MaskPlan, AutoLogic.MaskPlan> MaskFilter;

    // detection
    readonly Dictionary<uint, (long created, long cpu, DateTime at)> _cpu = new();
    readonly Dictionary<uint, double> _share = new();
    uint _candidate;
    DateTime _candidateSince;
    DateTime _armingStart;
    string _blocked = "";
    bool _unrecognized;
    string _notWatched = "";
    // Section 22: the roadmap switches. They are handed over by the settings; Auto reads no setting itself.
    /// <summary>22.4: pinned threads are also limited to the CPU Set of their processor.</summary>
    public bool ThreadSets;
    /// <summary>22.5: the fine timer resolution is requested while a game is active.</summary>
    public bool TimerRes;
    /// <summary>22.6: the heat policy of the game's own preset (HotFirst, HotOthers); null = none.</summary>
    public Func<string, (string First, string Others)> Heat;
    /// <summary>22.3: soft closing of the user's list when a game becomes active.</summary>
    public SoftClose Soft;
    bool _bgStandalone;
    DateTime _top5MetaAt;
    /// <summary>The dynamic Top-5 mode: instead of the role plan, five slots are re-ranked every second.</summary>
    public bool Top5Mode { get; private set; }
    public Top5 Dynamic { get; private set; }

    /// <summary>Switches between the role plan and the dynamic Top-5; an active game is laid out anew.</summary>
    public void SetTop5(bool on)
    {
        lock (_lock)
        {
            if (Top5Mode == on) return;
            Top5Mode = on;
            Note(on ? L.T("Thread mode: dynamic Top-5") : L.T("Thread mode: role plan"));
            if (State != AutoState.Active || _busy) return;
            _busy = true;
            try
            {
                Dynamic.Stop(false);
                _e.Revert(_game);
                _plan = new();
                _planTids = new();
                _pins = false;
                _confidence = 0;
                _e.BeginPass();
                try { Reapply(_now(), first: true); }
                finally { _e.EndPass(); }
            }
            catch (Exception ex) { FaultLocked(ex); }
            finally { _busy = false; }
        }
    }

    /// <summary>The role plan as it is now (for the threads page): role, thread, name, processor.</summary>
    public List<(ThreadRole Role, uint Tid, string Name, uint LP, bool Pinned)> PlanView()
    {
        lock (_lock)
            return _plan.Select(c => (c.Role, c.Row.Tid, c.Row.Desc.Length > 0 ? c.Row.Desc : Engine.ModuleOf(c.Row.Sym), c.LP, _pins)).ToList();
    }

    public double Confidence { get { lock (_lock) return _confidence; } }
    /// <summary>Why the foreground game is not handled ("" = nothing blocks).</summary>
    public string Blocked { get { lock (_lock) return _blocked.Length > 0 ? _blocked : _notWatched.Length > 0 ? L.F("{0} is not on the watchlist", _notWatched) : ""; } }

    /// <summary>The watchlist (section 10): only a process on it may become the game. Null = no list (tests).</summary>
    public Func<ICollection<string>> WatchList;
    /// <summary>The game Auto is laying out now (0 = none): the preset layer leaves it alone.</summary>
    public uint GamePid { get { lock (_lock) return State == AutoState.Active ? _game : 0; } }
    public string GameExe { get { lock (_lock) return _gameExe; } }

    // active game
    uint _game;
    long _gameCreated;
    string _gameExe = "";
    DateTime _absentSince;
    MaskPlan _mask;
    List<Classified> _plan = new();
    HashSet<uint> _planTids = new();
    double _confidence;
    bool _pins;
    readonly HashSet<(uint pid, long created)> _games = new();   // processes that were the game: never background
    /// <summary>Threads with a measured rate in the snapshot the plan was built from.</summary>
    public int WarmRates { get; private set; }
    int _lastThreadCount;
    int _applyErrors;
    int _tick;
    DateTime _sessionStart;
    // watchdog
    double _baselineP99;
    int _baselineFrames;
    DateTime _appliedAt, _lastWindowCheck;
    int _badWindows;
    double _onP99;            // p99 with the changes, over the bad windows
    DateTime _verifyFrom;     // != default: the A/B check is running (changes taken off)
    string _lastRevertReason = "";

    public sealed class MaskPlan
    {
        public AutoLogic.MaskPlan Plan;
        public ulong Mask => Plan?.Mask ?? 0;
    }

    public AutoController(Engine engine, IGameProbe probe, IFrameSource frames, Func<DateTime> now = null)
    {
        _e = engine;
        _probe = probe;
        _frames = frames ?? new NoFrames();
        _now = now ?? (() => DateTime.UtcNow);
        _e.BeforeMutate = WriteSession;
        Dynamic = new Top5(_e);
    }

    public bool ButtonEnabled
    {
        get { lock (_lock) return State is AutoState.Idle or AutoState.Armed or AutoState.Active; }
    }

    public bool On
    {
        get { lock (_lock) return State is AutoState.Armed or AutoState.Active; }
    }

    void SetStatus(string s)
    {
        Status = s;
        Changed?.Invoke();
    }

    static void Note(string s, AppLog.Sev sev = AppLog.Sev.Info) => AppLog.Add(s, sev);

    void WriteSession()
    {
        try
        {
            var s = _e.Export();
            s.StartedUtc = _sessionStart == default ? _now() : _sessionStart;
            s.CleanShutdown = false;
            SaveSession?.Invoke(s);
        }
        catch (Exception ex) { Note(L.F("Session file: {0}", ex.Message)); }
    }

    /// <summary>Restores what a crashed run left (session.json with cleanShutdown = false).</summary>
    public int RecoverCrashedSession(SessionData s)
    {
        if (s == null || s.CleanShutdown) return 0;
        int n = _e.RestoreSession(s);
        Note(L.F("Previous run did not finish: {0} values restored", n));
        return n;
    }

    // ---- button --------------------------------------------------------------------------------

    public void Click()
    {
        lock (_lock)
        {
            if (_busy) return;
            _busy = true;
            try
            {
                if (State == AutoState.Idle)
                {
                    State = AutoState.Armed;
                    _bgStandalone = false;       // from here the background belongs to Auto (BgOn is forced)
                    _sessionStart = _now();
                    _candidate = 0;
                    _armingStart = _now();
                    _tick = 0;
                    Note(L.T("Armed"));
                    SetStatus(L.T("Waiting for the game…"));
                    PassBackground();
                }
                else if (State is AutoState.Armed or AutoState.Active)
                    RevertAllLocked(L.T("Button"));
            }
            catch (Exception ex) { FaultLocked(ex); }
            finally { _busy = false; }
        }
    }

    /// <summary>Window closed, exit, Windows session ending: everything back.</summary>
    public void Shutdown()
    {
        lock (_lock)
        {
            if (State == AutoState.Idle) return;
            RevertAllLocked(L.T("Exit"));
        }
    }

    /// <summary>Program exit or a "revert everything" request: also what the preset layer changed goes back.</summary>
    public int RevertEverything(string why)
    {
        lock (_lock)
        {
            if (State != AutoState.Idle)
            {
                RevertAllLocked(why);
                return 0;
            }
            _e.TimerRelease();
            _bgStandalone = false;
            int n = _e.RevertAll();
            if (n > 0) Note(L.F("Reverted ({0}): {1} values", why, n));
            try
            {
                var s = _e.Export();
                s.CleanShutdown = true;
                SaveSession?.Invoke(s);
            }
            catch (Exception ex) { Note(L.F("Session file: {0}", ex.Message)); }
            _lastRevertReason = why;
            return n;
        }
    }

    void RevertAllLocked(string why)
    {
        State = AutoState.Reverting;
        SetStatus(L.T("Reverting…"));
        _e.TimerRelease();
        _bgStandalone = false;
        int n = _e.RevertAll();
        _frames.Watch(0);
        Note(L.F("Reverted ({0}): {1} values", why, n));
        try
        {
            var s = _e.Export();
            s.StartedUtc = _sessionStart;
            s.CleanShutdown = true;
            SaveSession?.Invoke(s);
        }
        catch (Exception ex) { Note(L.F("Session file: {0}", ex.Message)); }
        ClearGame();
        _lastRevertReason = why;
        State = AutoState.Idle;
        SetStatus(L.T("Off"));
    }

    void FaultLocked(Exception ex)
    {
        State = AutoState.Faulted;
        Note(L.F("Error: {0}", ex.Message), AppLog.Sev.Error);
        try { _e.TimerRelease(); } catch (Exception) { }
        try { _e.RevertAll(); } catch (Exception e2) { Note(L.F("Revert error: {0}", e2.Message)); }
        try
        {
            var s = _e.Export();
            s.CleanShutdown = true;
            SaveSession?.Invoke(s);
        }
        catch (Exception) { }
        _frames.Watch(0);
        ClearGame();
        State = AutoState.Idle;
        SetStatus(L.T("Error. Everything was put back. Open the Report."));
    }

    void ClearGame()
    {
        Dynamic.Stop(false);      // the handles are closed; the values were put back by the engine's revert
        _game = 0;
        _gameCreated = 0;
        _gameExe = "";
        _plan = new();
        _planTids = new();
        _mask = null;
        _pins = false;
        _confidence = 0;
        _applyErrors = 0;
        _lastThreadCount = 0;
        _baselineP99 = 0;
        _baselineFrames = 0;
        _badWindows = 0;
        _onP99 = 0;
        _verifyFrom = default;
    }

    // ---- tick ------------------------------------------------------------------------------------

    /// <summary>Every second (the Optimizer's timer).</summary>
    public void Tick()
    {
        lock (_lock)
        {
            if (_busy || State is AutoState.Idle or AutoState.Reverting or AutoState.Faulted) return;
            _busy = true;
            try
            {
                _tick++;
                Soft?.Tick(pid => _probe.CpuTime(pid, out _, out _));
                _e.BeginPass();
                try { TickLocked(); }
                finally { _e.EndPass(); }
                if (_tick % 60 == 0) _e.Prune();
            }
            catch (Exception ex) { FaultLocked(ex); }
            finally { _busy = false; }
        }
    }

    bool ReapplyTick => _tick % Math.Clamp(AutoPolicy.ReapplyMs / 1000, 1, 1000) == 0;

    void TickLocked()
    {
        DateTime now = _now();
        uint target = Detect(now);

        if (State == AutoState.Armed)
        {
            if (target != 0)
            {
                if (target != _candidate)
                {
                    _candidate = target;
                    _candidateSince = now;
                    _armingStart = now;
                    _frames.Watch(target);   // the frametime baseline is collected before the first change
                    SetStatus(L.T("Detecting the game…"));
                }
                else if ((now - _candidateSince).TotalMilliseconds >= AutoPolicy.EnterStableMs)
                {
                    if ((now - _armingStart).TotalMilliseconds > AutoPolicy.ArmingTimeoutMs + AutoPolicy.EnterStableMs)
                    {
                        _candidate = 0;   // the attempt took too long: stay armed and try again
                        SetStatus(L.T("Waiting for the game…"));
                    }
                    else Activate(target, now);
                }
            }
            else
            {
                _candidate = 0;
                SetStatus(_blocked.Length > 0 ? L.T("Anti-cheat: the game is not changed, the background is muted")
                    : _notWatched.Length > 0 ? L.F("{0} is not on the watchlist: tick it on the Processes page", _notWatched)
                    : _unrecognized ? L.T("Game not recognized. Background muted.") : L.T("Waiting for the game…"));
            }
            if (ReapplyTick) PassBackground();
            return;
        }

        // Active.
        if (!ProcessAlive(_game, _gameCreated))
        {
            Note(L.F("{0} ended", _gameExe));
            LeaveGame(revertGame: false);
            return;
        }
        if (target == _game) _absentSince = default;
        else
        {
            if (_absentSince == default) _absentSince = now;
            if ((now - _absentSince).TotalMilliseconds > AutoPolicy.ExitStableMs)
            {
                Note(L.F("{0} not in front for {1} s: reverted", _gameExe, AutoPolicy.ExitStableMs / 1000));
                LeaveGame(revertGame: true);
                return;
            }
        }
        if (_verifyFrom != default)
        {
            Verify(now);
            return;
        }
        if (Top5Mode && Dynamic.Running) Dynamic.Tick(now, _e.ThreadsNow(_game));
        if (ReapplyTick)
        {
            if (!Reapply(now)) return;
            PassBackground();
        }
        CheckFrames(now);
    }

    // ---- detection (12.4) ------------------------------------------------------------------------

    bool ProcessAlive(uint pid, long created) => pid != 0 && _probe.CpuTime(pid, out long c, out _) && c == created;

    double CpuShare(uint pid, DateTime now)
    {
        if (!_probe.CpuTime(pid, out long created, out long cpu)) return 0;
        double share = _share.GetValueOrDefault(pid);
        if (_cpu.TryGetValue(pid, out var prev) && prev.created == created)
        {
            double dt = (now - prev.at).TotalSeconds;
            if (dt > 0.2) share = Math.Max(0, (cpu - prev.cpu) / 1e7 / dt);
            else return share;
        }
        _cpu[pid] = (created, cpu, now);
        _share[pid] = share;
        return share;
    }

    IEnumerable<string> RunningExes(Dictionary<uint, (string Name, uint Parent)> procs) => procs.Values.Select(p => p.Name);

    bool LooksLikeGame(uint pid, string exe, Dictionary<uint, (string Name, uint Parent)> procs, HashSet<uint> windows, DateTime now)
    {
        if (pid is 0 or 4 || pid == _probe.SelfPid) return false;
        if (SystemDeny.Contains(exe) || LauncherDeny.Contains(exe)) return false;
        if (Guard.Gate(exe, Guard.Op.Process, false, RunningExes(procs)) != null) return false;
        if (!windows.Contains(pid)) return false;
        if (!_probe.HasGraphicsModule(pid)) return false;
        if (CpuShare(pid, now) <= AutoPolicy.GameCpuShare) return false;
        var watch = WatchList?.Invoke();
        if (watch != null && !watch.Contains(exe))
        {
            _notWatched = exe;       // looks like a game, but the user did not put it on the watchlist
            return false;
        }
        return true;
    }

    uint Detect(DateTime now)
    {
        var procs = _e.ProcessesNow();
        var windows = _probe.WindowPids();
        uint fg = _probe.ForegroundPid();
        string fgExe = procs.TryGetValue(fg, out var fp) ? fp.Name : "";
        _blocked = fg != 0 && fgExe.Length > 0 ? Guard.Gate(fgExe, Guard.Op.Process, false, RunningExes(procs)) ?? "" : "";
        _unrecognized = false;
        _notWatched = "";
        foreach (uint pid in windows.ToList())
            if (pid != fg && !procs.ContainsKey(pid)) windows.Remove(pid);
        // CPU shares of window owners only (cheap: one GetProcessTimes each).
        foreach (uint pid in windows) CpuShare(pid, now);
        foreach (uint pid in _cpu.Keys.Where(p => !windows.Contains(p)).ToList())
        {
            _cpu.Remove(pid);
            _share.Remove(pid);
        }
        if (_blocked.Length > 0) return 0;
        if (fg != 0 && LooksLikeGame(fg, fgExe, procs, windows, now)) return fg;
        // The only other case that may pick a game: nothing or this program in front (the button was just pressed).
        if (fg == 0 || fg == _probe.SelfPid)
        {
            var eligible = windows.Where(p => procs.TryGetValue(p, out var pr) && LooksLikeGame(p, pr.Name, procs, windows, now)).ToList();
            if (eligible.Count == 1) return eligible[0];
        }
        if (fg != 0 && fgExe.Length > 0 && windows.Contains(fg) && !SystemDeny.Contains(fgExe) && !LauncherDeny.Contains(fgExe) && _probe.HasGraphicsModule(fg))
            _unrecognized = (now - _armingStart).TotalMilliseconds > AutoPolicy.ArmingTimeoutMs;
        return 0;
    }

    // ---- activation and re-apply -----------------------------------------------------------------

    void Activate(uint pid, DateTime now)
    {
        var procs = _e.ProcessesNow();
        _game = pid;
        _probe.CpuTime(pid, out _gameCreated, out _);
        _games.Add((pid, _gameCreated));
        _gameExe = procs.TryGetValue(pid, out var p) ? p.Name : "";
        _absentSince = default;
        // Frametime baseline: the frames of the detection period, before any change.
        if (_frames.Available)
        {
            var iv = _frames.Intervals(now.AddMilliseconds(-AutoPolicy.BaselineMs), now);
            _baselineFrames = iv.Count;
            _baselineP99 = P99(iv);
        }
        State = AutoState.Active;
        Note(L.F("Game: {0} (PID {1})", _gameExe, pid));
        if (TimerRes) _e.TimerRequest();
        Soft?.Begin(procs, new[] { pid }, WatchList?.Invoke(), _probe.SelfPid);
        if (!Reapply(now, first: true)) return;
        PassBackground();
        _appliedAt = _lastWindowCheck = _now();
    }

    void LeaveGame(bool revertGame)
    {
        _e.TimerRelease();
        if (revertGame && _game != 0) _e.Revert(_game);
        _frames.Watch(0);
        ClearGame();
        _candidate = 0;
        _armingStart = _now();
        State = AutoState.Armed;
        SetStatus(L.T("Waiting for the game…"));
    }

    /// <summary>One application to the game; false when a watchdog trigger reverted everything.</summary>
    bool Reapply(DateTime now, bool first = false)
    {
        var t = TopoBuilder.Live();
        try
        {
            // Mask (12.5).
            var mp = AutoLogic.BuildMask(t, Reservation.Reserved());
            if (MaskFilter != null) mp = MaskFilter(mp);
            if (!first && _mask?.Plan != null && _mask.Plan.Satisfied && !mp.Satisfied)
                return Watchdog(L.T("The mask became unsatisfiable"));
            if (!mp.Satisfied)
                foreach (var n in mp.Notes) Note(L.F("Mask not applied: {0}", n));
            _mask = new MaskPlan { Plan = mp };
            ulong mask = mp.Satisfied ? mp.Mask : 0;

            if (Top5Mode) return ReapplyTop5(t, mask, now, first);

            // Threads: warm-up on the first application, plan only when needed (17.2).
            ThreadSnap snap;
            if (first)
            {
                _e.Snapshot(_game);
                snap = null;
                for (int i = 0; i < AutoPolicy.WarmupRounds; i++)
                {
                    _e.Sleep(AutoPolicy.WarmupMs);
                    snap = _e.Snapshot(_game);
                    if (snap.Rows.Count(r => r.Rate > 0) >= 2) break;
                }
            }
            else snap = _e.Snapshot(_game);
            if (!snap.Ok) return Watchdog(L.T("The game process cannot be opened"));

            if (!first && _lastThreadCount > 0 && snap.Rows.Count * AutoPolicy.ThreadDropFactor < _lastThreadCount)
                return Watchdog(L.F("The game's thread count fell from {0} to {1}", _lastThreadCount, snap.Rows.Count));
            _lastThreadCount = snap.Rows.Count;

            var nowTids = snap.Rows.Where(r => r.Rate > AutoLogic.HotThreshold(snap)).Select(r => r.Tid).ToList();
            bool rebuild = first || _plan.Count == 0 || AutoLogic.CompositionChange(_planTids, nowTids) > AutoPolicy.PlanChangeFraction;
            if (rebuild)
            {
                var prev = _plan.ToDictionary(c => c.Row.Tid, c => c.LP);
                var plan = AutoLogic.Plan(t, snap, mask, prev);
                WarmRates = snap.Rows.Count(r => r.Rate > 0);
                double conf = AutoLogic.Confidence(snap, plan);
                bool pins = conf >= AutoPolicy.ConfidenceFloor;
                bool same = !first && pins == _pins && plan.Count == prev.Count && plan.All(c => prev.TryGetValue(c.Row.Tid, out uint lp) && lp == c.LP);
                _plan = plan;
                _planTids = nowTids.ToHashSet();
                _confidence = conf;
                _pins = pins;
                if (!same)   // the log line only when the threads or their cores really changed
                    Note(L.F("Plan: {0} threads, confidence {1:0.00}{2}", plan.Count, conf, _pins ? "" : " — " + L.T("below the floor, no pins")));
            }
            else
            {
                double conf = AutoLogic.Confidence(snap, _plan);
                if (_pins && conf < AutoPolicy.ConfidenceFloor)
                {
                    _pins = false;
                    _e.DropPins(_game);
                    Note(L.F("Confidence fell to {0:0.00}: pins removed, the mask stays", conf));
                }
                _confidence = conf;
            }
            var heat = Heat?.Invoke(_gameExe) ?? ("none", "none");
            var preset = AutoLogic.BuildAutoPreset(_gameExe, _plan, _pins, ThreadSets, heat.Item1, heat.Item2);
            var o = new ApplyOptions { DoMask = mask != 0, DoThreads = true, Warmup = false, Game = true };
            var rep = _e.ApplyAuto(_game, preset, mask, o);
            if (rep.Failures.Count > 0 && rep.HandleLevel < 0) return Watchdog(L.T("The game process cannot be opened"));
            if (_e.LastDeniedShare > AutoPolicy.DeniedShareLimit)
                return Watchdog(L.F("Access denied for {0:0}% of the game's threads", _e.LastDeniedShare * 100));
            _applyErrors = 0;
            SetStatus(_pins && mask != 0 ? L.F("Game: {0} · boost on", _gameExe) : L.F("Game: {0} · boost on (basic)", _gameExe));
            return true;
        }
        catch (Exception ex)
        {
            _applyErrors++;
            Note(L.F("Apply error {0}/{1}: {2}", _applyErrors, AutoPolicy.MaxApplyErrors, ex.Message));
            if (_applyErrors >= AutoPolicy.MaxApplyErrors) throw;
            return true;
        }
    }

    /// <summary>
    /// The dynamic mode's part of an application: class and mask as always, no thread rules; the
    /// threads belong to the Top-5 runner. A full snapshot (it opens every thread) is taken only
    /// at the start and when the names are older than MetaRefreshMs or new threads appeared.
    /// </summary>
    bool ReapplyTop5(Topo t, ulong mask, DateTime now, bool first)
    {
        ThreadSnap snap = null;
        bool meta = first || !Dynamic.Running || Dynamic.NeedsMeta && (now - _top5MetaAt).TotalMilliseconds >= AutoPolicy.ReapplyMs
                    || (now - _top5MetaAt).TotalMilliseconds >= Top5Policy.MetaRefreshMs;
        if (meta)
        {
            if (first)
            {
                _e.Snapshot(_game);
                _e.Sleep(AutoPolicy.WarmupMs);
            }
            snap = _e.Snapshot(_game);
            if (!snap.Ok) return Watchdog(L.T("The game process cannot be opened"));
            if (!first && _lastThreadCount > 0 && snap.Rows.Count * AutoPolicy.ThreadDropFactor < _lastThreadCount)
                return Watchdog(L.F("The game's thread count fell from {0} to {1}", _lastThreadCount, snap.Rows.Count));
            _lastThreadCount = snap.Rows.Count;
            _top5MetaAt = now;
        }
        _plan = new();
        _planTids = new();
        _pins = false;
        var preset = AutoLogic.BuildAutoPreset(_gameExe, _plan, false);
        var rep = _e.ApplyAuto(_game, preset, mask, new ApplyOptions { DoMask = mask != 0, DoThreads = false, Warmup = false, Game = true });
        if (rep.Failures.Count > 0 && rep.HandleLevel < 0) return Watchdog(L.T("The game process cannot be opened"));
        if (!Dynamic.Running)
        {
            if (!Dynamic.Start(_game, _gameExe, t, mask)) Note(L.T("Dynamic Top-5 could not start for this game: only the mask and the priority class are set"), AppLog.Sev.Warn);
            else Note(L.F("Dynamic Top-5: {0} slots on processors of the game mask", Math.Min(Top5Policy.Slots, Masks.IdealCandidates(t, true, mask).Count)));
        }
        if (snap != null) Dynamic.SetMeta(snap);
        _applyErrors = 0;
        SetStatus(L.F("Game: {0} · dynamic Top-5", _gameExe));
        return true;
    }

    bool Watchdog(string why)
    {
        Note(L.F("Watchdog: {0}", why), AppLog.Sev.Warn);
        RevertAllLocked(L.F("watchdog: {0}", why));
        SetStatus(L.F("Reverted: {0}. Open the Report.", why));
        return false;
    }

    // ---- background (12.8) -----------------------------------------------------------------------

    /// <summary>
    /// Section 22.1: the background layer on its own (BgOn and Auto off). While a watched program
    /// runs, the background set is lowered by the same rules as in Auto; when the switch goes off
    /// or the game ends, BgRelease puts every background baseline back.
    /// </summary>
    public int StandaloneBackground(bool on, ulong gameMask)
    {
        lock (_lock)
        {
            if (_busy || State != AutoState.Idle) return 0;
            var procs = _e.ProcessesNow();
            var watch = WatchList?.Invoke();
            var games = watch == null ? new HashSet<uint>() : procs.Where(p => watch.Contains(p.Value.Name)).Select(p => p.Key).ToHashSet();
            Soft?.Tick(pid => _e.ProcessesNow().ContainsKey(pid));
            if (!on || games.Count == 0)
            {
                if (!_bgStandalone) return 0;
                _bgStandalone = false;
                int n = _e.RevertBackground();
                Note(L.F("BgRelease ({0}): {1} background values put back", on ? L.T("the game ended") : L.T("switched off"), n));
                return n;
            }
            if (!_bgStandalone)
            {
                _bgStandalone = true;
                Note(L.T("Background layer: on (a watched program is running)"));
                Soft?.Begin(procs, games, watch, _probe.SelfPid);
            }
            PassBackground(games, gameMask);
            return _e.BackgroundCount;
        }
    }

    void PassBackground(HashSet<uint> games = null, ulong? maskOverride = null)
    {
        var t = TopoBuilder.Live();
        var procs = _e.ProcessesNow();
        uint self = _probe.SelfPid, mySession = _probe.SessionOf(self);
        uint fg = _probe.ForegroundPid();
        var selfTree = new HashSet<uint> { self };
        bool grew = true;
        while (grew)
        {
            grew = false;
            foreach (var (pid, p) in procs)
                if (!selfTree.Contains(pid) && selfTree.Contains(p.Parent)) grew = selfTree.Add(pid);
        }
        var running = RunningExes(procs).ToList();
        var list = new List<uint>();
        foreach (var (pid, p) in procs)
        {
            if (pid is 0 or 4 || pid == _game || selfTree.Contains(pid)) continue;
            if (games != null && games.Contains(pid)) continue;
            if (_games.Contains((pid, CreatedOf(pid)))) continue;
            // A game the user did not put on the watchlist is nobody's background: it is left alone.
            if (_notWatched.Length > 0 && p.Name.Equals(_notWatched, StringComparison.OrdinalIgnoreCase)) continue;
            if (pid == fg) continue;      // what the user is working with right now is not lowered
            if (SystemDeny.Contains(p.Name) || Guard.IsAntiCheatProcess(p.Name)) continue;
            if (Guard.Gate(p.Name, Guard.Op.Process, false, running) != null) continue;
            if (_probe.SessionOf(pid) != mySession) continue;
            list.Add(pid);
        }
        ulong gameMask = maskOverride ?? _mask?.Mask ?? 0;
        ulong bg = t != null ? Masks.BgMask(t, gameMask) : 0;
        if (t != null && bg == t.FullMask()) bg = 0;   // nothing to separate from
        _e.ApplyBackground(list, bg, true);
    }

    long CreatedOf(uint pid) => _probe.CpuTime(pid, out long c, out _) ? c : 0;

    // ---- frametime watchdog (12.11, 17.4) --------------------------------------------------------

    static double P99(List<double> iv)
    {
        if (iv.Count == 0) return 0;
        var s = iv.OrderBy(x => x).ToList();
        return s[Math.Min(s.Count - 1, (int)Math.Ceiling(s.Count * 0.99) - 1)];
    }

    /// <summary>
    /// The baseline only raises a suspicion: it was measured in another moment of the game (menu,
    /// loading, another scene). The verdict is an A/B check right away: the changes are taken off,
    /// the frames are measured, and only a real difference keeps the revert.
    /// </summary>
    void CheckFrames(DateTime now)
    {
        if (!_frames.Available || _baselineFrames < AutoPolicy.MinFramesForWatchdog || _probe.ForegroundPid() != _game) return;
        DateTime from = now.AddMilliseconds(-AutoPolicy.WatchdogWindowMs);
        if (from < _appliedAt.AddMilliseconds(AutoPolicy.WatchdogWarmupMs)) return;
        if ((now - _lastWindowCheck).TotalMilliseconds < AutoPolicy.WatchdogWindowMs) return;
        var iv = _frames.Intervals(from, now);
        _lastWindowCheck = now;
        if (iv.Count < AutoPolicy.MinFramesForWatchdog) return;
        double p99 = P99(iv);
        if (!(_baselineP99 > 0 && p99 > _baselineP99 * (1 + AutoPolicy.WatchdogRegression)))
        {
            _badWindows = 0;
            return;
        }
        if (++_badWindows < AutoPolicy.WatchdogConfirmWindows) return;
        _onP99 = P99(_frames.Intervals(now.AddMilliseconds(-AutoPolicy.WatchdogWindowMs * AutoPolicy.WatchdogConfirmWindows), now));
        Note(L.F("Frame time p99 {0:0.00} ms against {1:0.00} ms before: checking without the changes", _onP99, _baselineP99), AppLog.Sev.Warn);
        int n = _e.Revert(_game) + _e.RevertBackground();
        Note(L.F("Changes taken off for the check: {0} values", n));
        _verifyFrom = _now();
        SetStatus(L.F("Game: {0} · checking the frame time without the changes…", _gameExe));
    }

    void Verify(DateTime now)
    {
        double elapsed = (now - _verifyFrom).TotalMilliseconds;
        if (elapsed < AutoPolicy.VerifySettleMs + AutoPolicy.WatchdogWindowMs) return;
        bool front = _probe.ForegroundPid() == _game;
        var iv = front ? _frames.Intervals(now.AddMilliseconds(-AutoPolicy.WatchdogWindowMs), now) : new List<double>();
        if (iv.Count < AutoPolicy.MinFramesForWatchdog)
        {
            if (elapsed < AutoPolicy.VerifyMaxMs) return;
            Note(L.T("The check got too few frames: the changes are returned"));
            Resume(now, _baselineP99);
            return;
        }
        double off = P99(iv);
        if (_onP99 > off * (1 + AutoPolicy.WatchdogRegression))
        {
            _verifyFrom = default;
            Watchdog(L.F("frame time p99 {0:0.00} ms with the changes against {1:0.00} ms without", _onP99, off));
            return;
        }
        Note(L.F("Without the changes p99 {0:0.00} ms, with them {1:0.00} ms: the game itself got heavier, the changes are returned", off, _onP99));
        Resume(now, off);
    }

    void Resume(DateTime now, double baseline)
    {
        _verifyFrom = default;
        _badWindows = 0;
        _baselineP99 = baseline;
        if (!Reapply(now)) return;
        PassBackground();
        _appliedAt = _lastWindowCheck = _now();
    }

    // ---- report (12.12, 16) ----------------------------------------------------------------------

    public string Report()
    {
        lock (_lock)
        {
            var t = TopoBuilder.Live();
            bool measured = _frames.Available && (_game == 0 || _baselineFrames >= AutoPolicy.MinFramesForWatchdog);
            var sb = new StringBuilder();
            void Line(string s) => sb.AppendLine(s);
            Line(L.T("DomainScheduler — report") + "  " + _now().ToLocalTime().ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture));
            Line("");
            if (t != null)
            {
                Line(L.F("CPU: {0}", t.Name));
                Line(L.F("Topology: {0} logical, {1} physical, SMT {2}, groups {3}, L3 domains {4}", t.LogicalCount, t.PhysicalCount, t.SMT ? L.T("on") : L.T("off"), t.Groups, t.LLCGroups.Count));
                foreach (var l in t.LLCGroups)
                    Line("  " + L.F("L3 domain {0}: {1} MB, processors {2}", l, t.L3ByLlc.GetValueOrDefault(l) >> 20, Masks.Ranges(TopoBuilder.Mask(t.Sets.Where(s => s.LLC == l)))) + (t.BigCacheLlc == l ? " (V-Cache)" : ""));
                Line(L.F("Split: {0}", t.Split.CanSplit ? t.Split.Label + " — " + L.F("game {0}, non-game {1}", Masks.Ranges(t.Split.Game), Masks.Ranges(t.Split.NonGame)) : t.Split.Why()));
            }
            else Line(L.T("CPU: the topology could not be read"));
            Line(L.F("Auto: {0}", StateText()));
            Line(L.F("Status: {0}", Status));
            Line("");
            var mp = _mask?.Plan;
            if (mp != null)
            {
                Line(mp.Satisfied
                    ? L.F("Game mask: {0} → {1} ({2})", mp.Spec, "0x" + mp.Mask.ToString("x", CultureInfo.InvariantCulture), Masks.Ranges(mp.Mask))
                    : L.F("Game mask: not applied ({0})", string.Join("; ", mp.Notes)));
                if (mp.Irq != null)
                {
                    Line(mp.Irq.IrqDrop != 0 ? L.F("Cores excluded for IRQ: {0}", Masks.Ranges(mp.Irq.IrqDrop))
                        : mp.Irq.IrqIgnored ? L.T("Cores excluded for IRQ: none (the exclusion would cut more than half)") : L.T("Cores excluded for IRQ: none"));
                    foreach (var d in mp.Irq.IrqDevices) Line("  " + L.F("{0}: processors {1}", d.Key, Masks.Ranges(d.Value)));
                    foreach (var d in mp.Irq.IrqWide) Line("  " + L.F("{0}: processors {1} (several cores, not excluded)", d.Key, Masks.Ranges(d.Value)));
                }
            }
            if (_game != 0)
            {
                var snap = _e.LastSnap;
                int named = snap?.Rows.Count(r => r.Desc.Length > 0) ?? 0;
                Line(L.F("Game: {0}, PID {1}, threads {2}, named {3}, confidence {4:0.00}", _gameExe, _game, snap?.Rows.Count ?? 0, named, _confidence));
                if (_pins)
                    foreach (var c in _plan)
                        Line("  " + L.F("{0} {1} ({2}) → processor {3}, core {4}", c.Role, c.Row.Tid, c.Row.Desc.Length > 0 ? c.Row.Desc : Engine.ModuleOf(c.Row.Sym), c.LP, c.Core));
                else Line("  " + L.T("Pins: none (confidence below the floor or no plan)"));
                Line(L.F("Priority: {0}, EcoQoS: off", L.T("Above normal")));
            }
            else Line(L.T("Game: not active"));
            Line(L.F("Background processes muted: {0}", _e.BackgroundCount));
            ulong reserved = Reservation.Reserved();
            Line(reserved == 0 ? L.T("Core reservation: not set") : L.F("Core reservation: processors {0}", Masks.Ranges(reserved)));
            var watch = WatchList?.Invoke();
            if (watch != null) Line(L.F("Watchlist: {0}", watch.Count == 0 ? L.T("empty") : string.Join(", ", watch)));
            string auto;
            try { auto = AutostartLine?.Invoke() ?? ""; }
            catch (Exception ex) { auto = L.F("Autostart: unknown ({0})", ex.Message); }
            if (auto.Length > 0) Line(auto);
            Line(SelfTestProblems.Count == 0 ? L.T("SelfTest: OK") : L.F("SelfTest: {0} problems ({1})", SelfTestProblems.Count, string.Join("; ", SelfTestProblems)));
            Line(measured ? L.T("Frametime: measured") : L.T("Frametime: not measured (the performance auto-revert is off)"));
            Line(L.T("Manual API: present, not active in Auto"));
            Line(Top5Mode ? L.F("Thread mode: dynamic Top-5, {0} moves so far", Dynamic.Moves) : L.T("Thread mode: role plan"));
            if (Top5Mode)
                foreach (var s in Dynamic.View())
                    Line("  " + L.F("slot {0}: {1} ({2}) on processor {3}, priority {4}, {5:0}% of the game's load", s.Index + 1, s.Name, s.Tid, s.LP, s.Prio.HasValue ? "+" + s.Prio.Value : L.T("own"), s.Share * 100));
            if (t != null && t.Split.Family == "x3d")
                Line(L.F("Big-cache domain: {0} half of the processor numbers ({1})", t.Split.UpperHalf ? L.T("upper") : L.T("lower"), Masks.Ranges(t.Split.Game)));
            Line(L.F("Thread CPU Sets: {0}", _e.ThreadPins.Count > 0 ? L.F("{0} threads limited to their processor", _e.ThreadPins.Count) : ThreadSets ? L.T("on, none now") : L.T("off")));
            Line(L.F("Timer resolution: {0}", _e.TimerRequested ? L.T("requested (released on revert)") : TimerRes ? L.T("on, not requested now") : L.T("off")));
            Line(L.F("Soft close: {0}", Soft != null && Soft.On ? L.F("on, {0} programs asked to close; closing cannot be undone and is not part of the crash recovery", Soft.Asked) : L.T("off")));
            Line(L.T("IRQ devices: not changed"));
            Line(measured ? (_game != 0 && _baselineP99 > 0 ? L.F("Frame time: measured ({0}); baseline p99 {1:0.00} ms", _frames.Name, _baselineP99) : L.F("Frame time: measured ({0})", _frames.Name))
                : L.F("Frame time is not measured — the performance auto-revert is off{0}", _frames.Error.Length > 0 ? " (" + _frames.Error + ")" : _frames.Available ? " (" + L.T("the game sends no DXGI Present") + ")" : ""));
            Line(L.F("Baseline: {0}", _e.Export().Procs.Count > 0 ? L.F("saved for {0} processes", _e.Export().Procs.Count) : L.T("nothing changed")));
            if (_lastRevertReason.Length > 0) Line(L.F("Last revert: {0}", _lastRevertReason));
            Line("");
            Line(L.T("Honest limits:"));
            Line("  " + L.T("The ideal processor is a hint: the scheduler may ignore it."));
            Line("  " + L.T("Thread classification is a heuristic; games without thread names are recognized worse and fall back to the mask."));
            Line("  " + L.T("On X3D the V-Cache die is a statistical choice, not the truth for every game."));
            Line("  " + L.T("On an ordinary Intel there is nothing to split."));
            Line("  " + L.T("The goal is an even p99 frame time, not the average FPS; no average FPS gain is guaranteed."));
            var tail = AppLog.Tail(80);
            if (tail.Count > 0)
            {
                Line("");
                Line(L.T("Log:"));
                foreach (var e in tail) Line("  " + e);
            }
            return sb.ToString();
        }
    }

    /// <summary>One line for the Quick page: what is changed right now.</summary>
    public string Summary()
    {
        lock (_lock)
        {
            if (State is AutoState.Idle or AutoState.Reverting or AutoState.Faulted)
                return _lastRevertReason.Length > 0 ? L.F("Nothing is changed. Last revert: {0}", _lastRevertReason) : L.T("Nothing is changed.");
            var parts = new List<string>();
            if (_game != 0)
            {
                var mp = _mask?.Plan;
                parts.Add(mp != null && mp.Satisfied ? L.F("{0}: processors {1}", _gameExe, Masks.Ranges(mp.Mask)) : L.F("{0}: no mask", _gameExe));
                parts.Add(_pins ? L.F("{0} threads placed", _plan.Count) : L.T("threads not placed"));
            }
            parts.Add(L.F("background processes lowered: {0}", _e.BackgroundCount));
            return string.Join(" · ", parts);
        }
    }

    string StateText() => State switch
    {
        AutoState.Idle => L.T("off"),
        AutoState.Armed => L.T("armed"),
        AutoState.Active => L.T("active"),
        AutoState.Reverting => L.T("reverting"),
        _ => L.T("fault"),
    };
}
