using System.Diagnostics;

namespace DomainScheduler;

/// <summary>
/// Soft closing of background programs (section 22.3). Only programs the user listed himself
/// are asked to close, with Process.CloseMainWindow; a program that is still there after
/// 5 seconds is left alone with a warning. TerminateProcess is never called. Closing cannot be
/// undone, so it is not part of the crash-safe session.
/// </summary>
public sealed class SoftClose
{
    public const int WaitSeconds = 5;
    readonly List<(uint Pid, string Exe, DateTime Deadline)> _pending = new();
    readonly object _gate = new();
    readonly HashSet<uint> _askedPids = new();      // a program is asked once, not at every game start

    /// <summary>Asks a process to close its main window; false when it has none.</summary>
    public Func<uint, bool> CloseWindow = pid =>
    {
        using var p = Process.GetProcessById((int)pid);
        return p.CloseMainWindow();
    };
    public Func<DateTime> Now = () => DateTime.UtcNow;
    /// <summary>The switch SoftCloseOn and the user's ClosableList (set by the settings).</summary>
    public bool On;
    public HashSet<string> List = new(StringComparer.OrdinalIgnoreCase);
    public int Asked { get; private set; }

    /// <summary>
    /// A game became active: every listed program is asked once. The denylist, system processes,
    /// the game (and everything on the watchlist) and this program are excluded whatever the list says.
    /// </summary>
    public int Begin(Dictionary<uint, (string Name, uint Parent)> procs, ICollection<uint> games, ICollection<string> watch, uint self)
    {
        lock (_gate)
        {
            _askedPids.RemoveWhere(p => !procs.ContainsKey(p));
            if (!On || List.Count == 0) return 0;
            if (Flags.Refuse(Flags.SoftClose, L.T("Soft close"))) return 0;
            int n = 0;
            foreach (var (pid, p) in procs)
            {
                if (!List.Contains(p.Name)) continue;
                if (pid is 0 or 4 || pid == self || (games != null && games.Contains(pid))) continue;
                if (Guard.IsDenied(p.Name) || AutoController.IsSystem(p.Name) || (watch != null && watch.Contains(p.Name))) continue;
                if (!_askedPids.Add(pid)) continue;
                bool asked;
                try { asked = CloseWindow(pid); }
                catch (Exception e)
                {
                    AppLog.Add(L.F("Soft close: {0} (PID {1}) could not be asked: {2}", p.Name, pid, e.Message), AppLog.Sev.Warn);
                    continue;
                }
                if (!asked)
                {
                    AppLog.Add(L.F("Soft close: {0} (PID {1}) has no window to close, left running", p.Name, pid), AppLog.Sev.Warn);
                    continue;
                }
                AppLog.Add(L.F("Soft close: {0} (PID {1}) was asked to close", p.Name, pid));
                _pending.Add((pid, p.Name, Now().AddSeconds(WaitSeconds)));
                n++;
            }
            Asked += n;
            return n;
        }
    }

    /// <summary>Every second: after 5 s a program that is still running is left alone (never forced).</summary>
    public void Tick(Func<uint, bool> alive)
    {
        lock (_gate)
        {
            if (_pending.Count == 0) return;
            DateTime now = Now();
            foreach (var x in _pending.ToList())
            {
                bool lives = alive(x.Pid);
                if (lives && now < x.Deadline) continue;
                _pending.Remove(x);
                if (lives) AppLog.Add(L.F("Soft close: {0} (PID {1}) is still running after {2} s, left alone", x.Exe, x.Pid, WaitSeconds), AppLog.Sev.Warn);
                else AppLog.Add(L.F("Soft close: {0} (PID {1}) closed", x.Exe, x.Pid));
            }
        }
    }
}
