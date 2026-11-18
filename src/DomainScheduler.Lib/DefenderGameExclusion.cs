using System.Diagnostics;

namespace DomainScheduler;

/// <summary>
/// Clean-up only. Earlier test builds could exclude the game's exe from Microsoft Defender scanning while it ran and
/// kept the paths they added in defender-exclusions.txt. The feature is gone; RemoveAll still runs at start and at
/// exit so an exclusion left behind by such a build (Toolbox killed, PC crashed) is taken off. Nothing is ever added.
/// </summary>
public static class DefenderGameExclusion
{
    static string File_ => Path.Combine(Store.Dir, "defender-exclusions.txt");
    static readonly object Gate = new();

    /// <summary>Takes off every exclusion an earlier build added (listed in defender-exclusions.txt).</summary>
    public static string RemoveAll()
    {
        lock (Gate)
        {
            List<string> paths;
            try { paths = File.Exists(File_) ? File.ReadAllLines(File_).Where(l => l.Trim().Length > 0).Distinct(StringComparer.OrdinalIgnoreCase).ToList() : new(); }
            catch (Exception) { return ""; }
            if (paths.Count == 0) return "";
            var left = new List<string>();
            foreach (var p in paths)
            {
                var (ok, _) = Ps($"Remove-MpPreference -ExclusionProcess '{Quote(p)}'");
                if (!ok) left.Add(p);
            }
            try { if (left.Count == 0) File.Delete(File_); else File.WriteAllLines(File_, left); } catch (Exception) { }
            return left.Count == 0 ? L.F("Defender: exclusion taken off ({0})", string.Join(", ", paths.Select(Path.GetFileName)))
                                   : L.F("Defender: could not take off {0}; it is retried at the next start", string.Join(", ", left));
        }
    }

    static string Quote(string s) => s.Replace("'", "''");

    static (bool Ok, string Text) Ps(string command)
    {
        try
        {
            var psi = new ProcessStartInfo(Path.Combine(Environment.SystemDirectory, @"WindowsPowerShell\v1.0\powershell.exe"),
                "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command \"& { $ErrorActionPreference='Stop'; " + command.Replace("\"", "\\\"") + " }\"")
            { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
            using var p = Process.Start(psi);
            string o = p.StandardOutput.ReadToEnd(), e = p.StandardError.ReadToEnd();
            if (!p.WaitForExit(15000)) return (false, "timeout");
            return p.ExitCode == 0 ? (true, o) : (false, (e.Length > 0 ? e : o).Trim().Split('\n')[0]);
        }
        catch (Exception ex) { return (false, ex.Message); }
    }
}
