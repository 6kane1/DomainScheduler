using System.Globalization;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// The boot guard of the device interrupt changes (they take effect at the next start of Windows, and a bad one can
/// stop Windows from starting). Before anything is written, a "pending" marker is saved with the time and the current
/// boot. A later boot confirms the changes once the program has run in it for <see cref="ConfirmUptime"/> of uptime.
/// At the start of the program the changes are reverted when
///  - Windows went down unexpectedly since they were written (System log: Kernel-Power 41 or EventLog 6008), or
///  - a boot after the change ended before the program could confirm it (it saw that boot, never confirmed it).
/// "--revert-devices" reverts them without the window (also usable from Safe Mode).
/// </summary>
public static class BootGuard
{
    public static readonly TimeSpan ConfirmUptime = TimeSpan.FromMinutes(2);
    /// <summary>Boot times are computed from the uptime; two readings of one boot differ by milliseconds.</summary>
    static readonly TimeSpan SameBootTolerance = TimeSpan.FromSeconds(90);

    public sealed class State
    {
        public bool Pending { get; set; }
        public DateTime AppliedUtc { get; set; }
        public DateTime AppliedBootUtc { get; set; }
        /// <summary>A later boot in which the program ran but has not confirmed yet (default = none).</summary>
        public DateTime SeenBootUtc { get; set; }
    }

    public enum Verdict { Nothing, Wait, Revert }

    public static Func<string> PathHook;
    internal const string FileName = "device-boot-guard.json";
    public static string FilePath => PathHook?.Invoke() ?? Path.Combine(Store.Dir, FileName);
    /// <summary>Tests: the unexpected-shutdown check (applied time → seen).</summary>
    public static Func<DateTime, bool> CrashHook;

    public static bool SameBoot(DateTime a, DateTime b) => a != default && b != default && (a - b).Duration() <= SameBootTolerance;

    /// <summary>The decision at the start of the program (pure).</summary>
    public static Verdict Decide(State s, DateTime bootUtc, bool crashSinceApply)
    {
        if (s == null || !s.Pending) return Verdict.Nothing;
        if (crashSinceApply) return Verdict.Revert;
        // A boot after the change in which the program ran, but which ended before the confirmation.
        if (s.SeenBootUtc != default && !SameBoot(s.SeenBootUtc, bootUtc) && !SameBoot(s.SeenBootUtc, s.AppliedBootUtc)) return Verdict.Revert;
        return Verdict.Wait;
    }

    /// <summary>The changes are confirmed in a boot after the one they were written in, after 2 minutes of uptime (pure).</summary>
    public static bool CanConfirm(State s, DateTime bootUtc, TimeSpan uptime) =>
        s != null && s.Pending && !SameBoot(bootUtc, s.AppliedBootUtc) && uptime >= ConfirmUptime;

    /// <summary>Device changes were written in this boot: they start working after a restart of Windows.</summary>
    public static bool RestartPending()
    {
        var s = Load();
        return s != null && s.Pending && SameBoot(Reservation.BootUtc(), s.AppliedBootUtc);
    }

    /// <summary>What the home screen's "Optimize devices" button shows.</summary>
    public enum HomeDevices { Offer, BlockedByGame, AppliedRestartNeeded, AppliedActive }

    /// <summary>
    /// The home button's state (pure). applied: the device layout's backup exists. appliedUtc: when the last device change
    /// was written (the boot guard's marker; default = no unconfirmed change). Windows has restarted since the change
    /// when the current boot started after it — the same boot time the boot guard compares.
    /// </summary>
    public static HomeDevices HomeState(bool applied, bool gameRunning, DateTime appliedUtc, DateTime bootUtc)
    {
        if (!applied) return gameRunning ? HomeDevices.BlockedByGame : HomeDevices.Offer;
        return appliedUtc != default && appliedUtc > bootUtc ? HomeDevices.AppliedRestartNeeded : HomeDevices.AppliedActive;
    }

    /// <summary>When the unconfirmed device change was written (default when there is none).</summary>
    public static DateTime PendingAppliedUtc()
    {
        var s = Load();
        return s != null && s.Pending ? s.AppliedUtc : default;
    }

    public static State Load()
    {
        try { return File.Exists(FilePath) ? JsonSerializer.Deserialize<State>(File.ReadAllText(FilePath)) : null; }
        catch (Exception) { return null; }
    }

    static void Save(State s)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(FilePath)!);
        string tmp = FilePath + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(s));
        File.Move(tmp, FilePath, true);
    }

    public static void Clear()
    {
        try { File.Delete(FilePath); } catch (Exception) { }
    }

    /// <summary>Called before any device interrupt value is written. A failure to save the marker stops the write.</summary>
    public static void MarkPending()
    {
        var s = Load();
        if (s != null && s.Pending)
        {
            // Another change on top of an unconfirmed one: the new time counts, the boot stays the current one.
            s.AppliedUtc = DateTime.UtcNow;
            s.AppliedBootUtc = Reservation.BootUtc();
            s.SeenBootUtc = default;
        }
        else s = new State { Pending = true, AppliedUtc = DateTime.UtcNow, AppliedBootUtc = Reservation.BootUtc() };
        Save(s);
    }

    /// <summary>
    /// At the start of the program: reverts the device changes when the decision says so, else remembers this boot.
    /// revert does the work (Devices.RevertAll); returns lines for the log.
    /// </summary>
    public static List<string> OnStart(Func<string> revert)
    {
        var log = new List<string>();
        var s = Load();
        if (s == null || !s.Pending) return log;
        DateTime boot = Reservation.BootUtc();
        bool crash = UnexpectedShutdownSince(s.AppliedUtc);
        var v = Decide(s, boot, crash);
        if (v == Verdict.Revert)
        {
            string err = revert();
            log.Add(crash
                ? L.T("Windows went down unexpectedly after the device interrupt changes: they were reverted. Restart Windows.")
                : L.T("Windows restarted before the device interrupt changes were confirmed: they were reverted. Restart Windows."));
            if (err != null) log.Add(L.F("Devices: {0}", err));
            Clear();
            return log;
        }
        if (v == Verdict.Wait && !SameBoot(boot, s.AppliedBootUtc) && !SameBoot(boot, s.SeenBootUtc))
        {
            s.SeenBootUtc = boot;
            try { Save(s); } catch (Exception) { }
        }
        return log;
    }

    /// <summary>Clears the marker when the change has been confirmed in this boot. Returns a log line or null.</summary>
    public static string ConfirmIfDue(TimeSpan uptime)
    {
        var s = Load();
        if (!CanConfirm(s, Reservation.BootUtc(), uptime)) return null;
        Clear();
        return L.T("Windows started fine with the device interrupt changes: they are confirmed");
    }

    /// <summary>Kernel-Power 41, EventLog 6008 or a bug check (WER 1001, blue screen) in the System log after the given time.</summary>
    public static bool UnexpectedShutdownSince(DateTime utc)
    {
        if (CrashHook != null) return CrashHook(utc);
        try
        {
            string since = utc.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture);
            string q = "*[System[((Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41) or (Provider[@Name='EventLog'] and EventID=6008) or (Provider[@Name='Microsoft-Windows-WER-SystemErrorReporting'] and EventID=1001))"
                     + " and TimeCreated[@SystemTime>='" + since + "']]]";
            var psi = SysExe.Start("wevtutil.exe", redirect: false);
            psi.RedirectStandardOutput = true;
            psi.Arguments = $"qe System /q:\"{q}\" /c:1 /f:xml";
            using var p = System.Diagnostics.Process.Start(psi)!;
            var text = p.StandardOutput.ReadToEndAsync();
            if (!p.WaitForExit(15000)) return false;
            return text.Result.Contains("<Event", StringComparison.Ordinal);
        }
        catch (Exception) { return false; }
    }
}
