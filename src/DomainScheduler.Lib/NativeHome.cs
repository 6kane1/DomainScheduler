using System.Diagnostics;
using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>
/// Where the program's own native libraries are unpacked (1.15.0 Test 1.3).
///
/// Toolbox is one exe. Its native libraries (the WPF ones, the trace helpers) travel inside it and the .NET host
/// unpacks them on start, by default into %TEMP%\.net\DomainScheduler\&lt;build id&gt;. The .NET 8 host reuses a folder
/// that already exists there and only checks that each file is present — not what is in it — and %TEMP% belongs to
/// the user: a program running without administrator rights could put its own DLL there under the expected name, and
/// the next start with administrator rights would load it.
///
/// The host takes the folder from DOTNET_BUNDLE_EXTRACT_BASE_DIR. So the first thing the program does (before any
/// unpacked library is loaded: no WPF, no trace) is: if that variable does not point to "native" in the admin-only
/// state folder, start itself again with it set (and without the variables that make .NET load a profiler or a
/// startup hook), wait until the new copy has reached its own start, and end. For a start that has to report back
/// (update hand-over, --revert-devices, --uninstall, --factory-reset) it waits for the new copy to finish and returns
/// its exit code. If the new copy cannot start (the host could not unpack there), this copy goes on by itself as
/// before — the program is never left unable to start.
///
/// Honest limit: this first copy was itself started with the default folder; the host unpacked (or found) the files
/// there, but this copy does not load any of them before it hands over. Variables that make .NET load code at the
/// very start (a profiler set in the user's environment) still apply to this first copy.
/// </summary>
public static class NativeHome
{
    public const string ExtractEnv = "DOTNET_BUNDLE_EXTRACT_BASE_DIR";
    /// <summary>Set for the second copy: the PID of the first one (its "reached the start" event carries that number).</summary>
    public const string MarkerEnv = "DOMAINOS_TOOLBOX_PARENT";

    static string ReadyName(string parentPid) => "Local\\DomainScheduler.Relaunched." + parentPid;

    /// <summary>The folder for the unpacked libraries (in the checked state folder; throws when that cannot be used).</summary>
    public static string Dir => Path.Combine(Store.Dir, "native");

    /// <summary>Whether to start again with the folder set (pure).</summary>
    public static bool ShouldRelaunch(bool windows, bool bundled, string current, string wanted, string marker) =>
        windows && bundled && !string.IsNullOrEmpty(wanted) && string.IsNullOrEmpty(marker) && !SamePath(current, wanted);

    static bool SamePath(string a, string b) =>
        !string.IsNullOrEmpty(a) && !string.IsNullOrEmpty(b)
        && string.Equals(a.Trim().TrimEnd('\\', '/'), b.Trim().TrimEnd('\\', '/'), StringComparison.OrdinalIgnoreCase);

    /// <summary>Starts whose caller waits for the result: the first copy waits for the second one and returns its exit code.</summary>
    public static bool WaitsForChild(IEnumerable<string> args) =>
        args.Any(a => a is "--after-update" or "--revert-devices" or "--uninstall" or "--factory-reset");

    [DllImport("user32.dll")] static extern bool AllowSetForegroundWindow(int pid);

    /// <summary>
    /// Called first in Main. Null: go on in this process. A number: this process is done, exit with it.
    /// </summary>
    public static int? RelaunchIfNeeded(string[] args)
    {
        string marker = Environment.GetEnvironmentVariable(MarkerEnv);
        if (!string.IsNullOrEmpty(marker))
        {
            // The second copy: tell the first one it got here, and keep the marker away from our own children.
            try { if (EventWaitHandle.TryOpenExisting(ReadyName(marker), out var ev)) using (ev) ev.Set(); } catch (Exception) { }
            Environment.SetEnvironmentVariable(MarkerEnv, null);
            return null;
        }
        if (!OperatingSystem.IsWindows()) return null;
        bool bundled = string.IsNullOrEmpty(typeof(NativeHome).Assembly.Location);
        string wanted;
        try
        {
            if (!bundled) return null;
            wanted = Dir;
            Directory.CreateDirectory(wanted);
        }
        catch (Exception) { return null; }      // no safe folder: the default one, as before
        foreach (var n in StateDir.TakeNotes()) AppLog.Add(n, AppLog.Sev.Warn);
        if (!ShouldRelaunch(true, bundled, Environment.GetEnvironmentVariable(ExtractEnv), wanted, marker)) return null;
        try
        {
            string self = Environment.ProcessPath;
            if (string.IsNullOrEmpty(self)) return null;
            var psi = new ProcessStartInfo(self) { UseShellExecute = false, WorkingDirectory = Environment.CurrentDirectory };
            foreach (var a in args) psi.ArgumentList.Add(a);
            foreach (var k in psi.Environment.Keys.Where(SysExe.Unsafe).ToList()) psi.Environment.Remove(k);
            string pid = Environment.ProcessId.ToString(System.Globalization.CultureInfo.InvariantCulture);
            psi.Environment[ExtractEnv] = wanted;
            psi.Environment[MarkerEnv] = pid;
            using var ready = new EventWaitHandle(false, EventResetMode.ManualReset, ReadyName(pid));
            using var p = Process.Start(psi);
            if (p == null) return null;
            try { AllowSetForegroundWindow(p.Id); } catch (Exception) { }
            bool reached = false;
            for (int i = 0; i < 300 && !reached; i++)
            {
                reached = ready.WaitOne(200);
                if (!reached && p.HasExited) { reached = ready.WaitOne(0); break; }
            }
            if (!reached && p.HasExited)
            {
                if (p.ExitCode == 0) return 0;
                // The host could not start it there: go on here, with the default folder.
                AppLog.Add(L.F("The program's libraries could not be unpacked into the state folder (exit code {0}); the default temporary folder is used this time", p.ExitCode), AppLog.Sev.Warn);
                return null;
            }
            if (!WaitsForChild(args)) return 0;
            p.WaitForExit();
            return p.ExitCode;
        }
        catch (Exception) { return null; }
    }

    /// <summary>
    /// Removes unpacked folders of other builds (after an update the old one is no longer needed). The folder in use is
    /// the one a loaded module of this process comes from; when none is found nothing is removed.
    /// </summary>
    public static void CleanOld()
    {
        try
        {
            string exe = Path.GetFileNameWithoutExtension(Environment.ProcessPath ?? "");
            string root = Path.Combine(Dir, exe);
            if (exe.Length == 0 || !Directory.Exists(root)) return;
            string inUse = null;
            foreach (ProcessModule m in Process.GetCurrentProcess().Modules)
            {
                string f = m.FileName ?? "";
                if (!f.StartsWith(root + "\\", StringComparison.OrdinalIgnoreCase)) continue;
                inUse = f[(root.Length + 1)..].Split('\\')[0];
                break;
            }
            if (inUse == null) return;
            foreach (var d in Directory.GetDirectories(root))
                if (!Path.GetFileName(d).Equals(inUse, StringComparison.OrdinalIgnoreCase))
                    try { Directory.Delete(d, true); } catch (Exception) { }
        }
        catch (Exception) { }
    }
}
