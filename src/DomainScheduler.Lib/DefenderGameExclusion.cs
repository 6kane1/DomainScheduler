using System.Diagnostics;
using System.Text.RegularExpressions;

namespace DomainScheduler;

/// <summary>
/// Clean-up only. Earlier test builds could exclude the game's exe from Microsoft Defender scanning while it ran and
/// kept the paths they added in defender-exclusions.txt. The feature is gone; RemoveAll still runs at start and at
/// exit so an exclusion left behind by such a build (Toolbox killed, PC crashed) is taken off. Nothing is ever added.
///
/// The list lives in the admin-only state folder (an older version's list is taken over once, see
/// <see cref="LegacyImport"/>), and every line is checked again before use: only an absolute "X:\…\name.exe" path
/// made of letters, digits and a few plain punctuation marks passes — no quote of any kind (PowerShell also takes
/// ‘ ’ ‚ ‛ “ ” „ as quotes), no $, backtick, ;, |, &amp;, &lt;, &gt; or line break. The path never becomes part of a
/// command: a fixed PowerShell command reads it from an environment variable.
/// </summary>
public static class DefenderGameExclusion
{
    internal const string FileName = "defender-exclusions.txt";
    static string File_ => Path.Combine(Store.Dir, FileName);
    static readonly object Gate = new();

    static readonly Regex Safe = new(@"^[A-Za-z]:\\(?:[\p{Lu}\p{Ll}\p{Lt}\p{Lo}\p{Nd} _.()\[\]+,!#@%=~-]+\\)*[\p{Lu}\p{Ll}\p{Lt}\p{Lo}\p{Nd} _.()\[\]+,!#@%=~-]+\.exe$",
        RegexOptions.CultureInvariant | RegexOptions.IgnoreCase);

    /// <summary>A line of the list may be used (pure): see the class summary.</summary>
    public static bool IsSafePath(string line) =>
        line != null && line.Length <= 260 && Safe.IsMatch(line) && !line.Split('\\').Any(p => p is "." or "..");

    /// <summary>The fixed command: the path comes only from $env:DOMAINOS_EXCLUSION.</summary>
    internal const string Command = "$ErrorActionPreference='Stop'; Remove-MpPreference -ExclusionProcess $env:DOMAINOS_EXCLUSION";

    /// <summary>Takes off every exclusion an earlier build added (listed in defender-exclusions.txt).</summary>
    public static string RemoveAll()
    {
        lock (Gate)
        {
            List<string> paths, refused;
            try
            {
                if (!File.Exists(File_)) return "";
                var all = File.ReadAllLines(File_).Select(l => l.Trim()).Where(l => l.Length > 0).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
                paths = all.Where(IsSafePath).ToList();
                refused = all.Where(l => !IsSafePath(l)).ToList();
            }
            catch (Exception) { return ""; }
            if (refused.Count > 0)
                AppLog.Add(L.F("Defender: lines of the exclusion list that are not plain program paths were not used (remove such an exclusion by hand in Windows Security if it is there): {0}",
                    string.Join(" | ", refused.Take(5).Select(r => r.Length > 120 ? r[..120] + "…" : r))), AppLog.Sev.Warn);
            if (paths.Count == 0) { try { File.Delete(File_); } catch (Exception) { } return ""; }
            var left = new List<string>();
            foreach (var p in paths)
            {
                var (ok, _) = Ps(p);
                if (!ok) left.Add(p);
            }
            try { if (left.Count == 0) File.Delete(File_); else File.WriteAllLines(File_, left); } catch (Exception) { }
            return left.Count == 0 ? L.F("Defender: exclusion taken off ({0})", string.Join(", ", paths.Select(Path.GetFileName)))
                                   : L.F("Defender: could not take off {0}; it is retried at the next start", string.Join(", ", left));
        }
    }

    static (bool Ok, string Text) Ps(string path)
    {
        try
        {
            var psi = SysExe.PowerShell(Command, new Dictionary<string, string> { ["DOMAINOS_EXCLUSION"] = path });
            using var p = Process.Start(psi);
            var err = p.StandardError.ReadToEndAsync();
            string o = p.StandardOutput.ReadToEnd();
            if (!p.WaitForExit(15000)) return (false, "timeout");
            string e = err.Result;
            return p.ExitCode == 0 ? (true, o) : (false, (e.Length > 0 ? e : o).Trim().Split('\n')[0]);
        }
        catch (Exception ex) { return (false, ex.Message); }
    }
}
