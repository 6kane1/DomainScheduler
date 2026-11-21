using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// Taking over the data of versions before 1.15.0 Test 1.3 (1.15.0 Test 1.3). They kept everything in the user's own
/// folder (%LOCALAPPDATA%\DomainScheduler), which any program of that user can change without administrator rights.
/// Some of it is still needed: the backups that put back the device and registry changes made then.
///
/// At every start, each file still there is read once: only entries that pass the same checks the restores use
/// (exactly what the program itself could have written) are copied into the admin-only state folder, the rest is named
/// in the log and dropped; then the old file is renamed "*.migrated" so it is never read again. A backup that already
/// exists in the state folder wins (it holds the values from before the first change). The GPU profile marked "apply
/// at start" is taken over without that mark: the program asks once, in its own window, whether to keep applying it
/// (GpuProfiles.Confirm). Nothing is written to the
/// registry or to Windows here; this only moves files.
/// </summary>
public static class LegacyImport
{
    /// <summary>The importer of one file: (old content, content already in the state folder or null, dropped) → new content, or null for nothing.</summary>
    delegate string Importer(string text, string existing, List<string> dropped);

    static readonly (string Name, Importer Import)[] Files =
    {
        (IrqAuto.BackupName, (t, e, d) => IrqAuto.ImportBackup(t, e, d)),
        (Devices.BackupName, (t, e, d) => Devices.ImportBackup(t, e, d)),
        (Tweaks.BackupName, (t, e, d) => Tweaks.ImportBackup(t, e, d)),
        (Reservation.BackupName, (t, e, d) => e != null ? null : Reservation.ImportBackup(t, d)),
        (GamePower.FileName, (t, e, d) => e != null ? null : GamePower.ImportFile(t, d)),
        (DefenderGameExclusion.FileName, ImportExclusions),
        (BootGuard.FileName, (t, e, d) => e != null ? null : JsonSerializer.Serialize(JsonSerializer.Deserialize<BootGuard.State>(t) ?? throw new InvalidDataException("empty"))),
        ("session.json", ImportSession),
        (GpuProfiles.FileName, ImportGpu),
        ("presets.json", ImportPresets),
        ("tables.json", (t, e, d) => { if (e != null) return null; Tables.Merge(new Tables.Data(), t); return t; }),
        ("devices-state.json", (t, e, d) => e != null ? null : JsonSerializer.Serialize(JsonSerializer.Deserialize<Dictionary<string, DateTime>>(t) ?? new())),
    };

    /// <summary>
    /// Runs the take-over from legacyDir into the state folder (Store.Dir). Returns lines for the log. account only
    /// names where the files came from in the log.
    /// </summary>
    public static List<(string Text, AppLog.Sev Sev)> Run(string legacyDir, string account = "")
    {
        var log = new List<(string, AppLog.Sev)>();
        if (string.IsNullOrEmpty(legacyDir) || !Directory.Exists(legacyDir)) return log;
        if ((File.GetAttributes(legacyDir) & FileAttributes.ReparsePoint) != 0)
        {
            log.Add((L.F("The old data folder {0} is a link: nothing was taken over from it", legacyDir), AppLog.Sev.Warn));
            return log;
        }
        string dest = Store.Dir;
        if (string.Equals(Path.GetFullPath(dest).TrimEnd('\\', '/'), Path.GetFullPath(legacyDir).TrimEnd('\\', '/'), StringComparison.OrdinalIgnoreCase)) return log;
        foreach (var (name, import) in Files) One(legacyDir, dest, name, name, import, log);
        string profiles = Path.Combine(legacyDir, "profiles");
        if (Directory.Exists(profiles) && (File.GetAttributes(profiles) & FileAttributes.ReparsePoint) == 0)
            foreach (var f in Directory.GetFiles(profiles, "*.json"))
                One(legacyDir, dest, Path.Combine("profiles", Path.GetFileName(f)), Path.Combine("profiles", Store.Sanitize(Path.GetFileNameWithoutExtension(f)) + ".json"), ImportPresets, log);
        return log;
    }

    const long MaxBytes = 4 << 20;

    static void One(string from, string to, string name, string destName, Importer import, List<(string, AppLog.Sev)> log)
    {
        string src = Path.Combine(from, name), dst = Path.Combine(to, destName);
        if (!File.Exists(src)) return;
        var dropped = new List<string>();
        try
        {
            var fi = new FileInfo(src);
            if ((fi.Attributes & FileAttributes.ReparsePoint) != 0) throw new InvalidDataException(L.T("it is a link"));
            if (fi.Length > MaxBytes) throw new InvalidDataException(L.T("it is too large"));
            string text = File.ReadAllText(src);
            string existing = File.Exists(dst) ? File.ReadAllText(dst) : null;
            string result = import(text, existing, dropped);
            if (result != null)
            {
                Directory.CreateDirectory(Path.GetDirectoryName(dst)!);
                File.WriteAllText(dst + ".tmp", result);
                File.Move(dst + ".tmp", dst, true);
                log.Add((L.F("Taken over from the old data folder: {0}", name), AppLog.Sev.Info));
            }
            else if (existing != null) log.Add((L.F("Not taken over from the old data folder (the state folder already has it): {0}", name), AppLog.Sev.Info));
            if (dropped.Count > 0)
                log.Add((L.F("{0}: entries that are not values the program writes were dropped: {1}", name, string.Join(", ", dropped.Take(8)) + (dropped.Count > 8 ? " …" : "")), AppLog.Sev.Warn));
        }
        catch (Exception e)
        {
            log.Add((L.F("{0} from the old data folder was not taken over: {1}", name, e.Message), AppLog.Sev.Warn));
        }
        // Renamed in any case (read once): an unreadable or refused file is never tried again either.
        try
        {
            string target = src + ".migrated";
            if (File.Exists(target)) target = src + "." + DateTime.UtcNow.ToString("yyyyMMddHHmmss", System.Globalization.CultureInfo.InvariantCulture) + ".migrated";
            File.Move(src, target);
        }
        catch (Exception e) { log.Add((L.F("{0}: the old file could not be renamed ({1}); it is read again at the next start", name, e.Message), AppLog.Sev.Warn)); }
    }

    static string ImportExclusions(string text, string existing, List<string> dropped)
    {
        var lines = (existing ?? "").Split('\n').Concat(text.Split('\n')).Select(l => l.Trim()).Where(l => l.Length > 0).ToList();
        var keep = new List<string>();
        foreach (var l in lines)
            if (DefenderGameExclusion.IsSafePath(l)) { if (!keep.Contains(l, StringComparer.OrdinalIgnoreCase)) keep.Add(l); }
            else dropped.Add(l.Length > 80 ? l[..80] + "…" : l);
        return keep.Count == 0 ? null : string.Join(Environment.NewLine, keep) + Environment.NewLine;
    }

    /// <summary>A session of a run that did not finish: taken over so the start restores it (each value is checked again there).</summary>
    static string ImportSession(string text, string existing, List<string> dropped)
    {
        if (existing != null) return null;
        var s = JsonSerializer.Deserialize<SessionData>(text);
        if (s == null || s.CleanShutdown) return null;
        int procs = s.Procs?.Count ?? 0, threads = s.Threads?.Count ?? 0;
        s.Procs = (s.Procs ?? new()).Where(Engine.SessionProcValid).ToList();
        s.Threads = (s.Threads ?? new()).Where(Engine.SessionThreadValid).ToList();
        if (s.Procs.Count < procs) dropped.Add(L.F("{0} processes", procs - s.Procs.Count));
        if (s.Threads.Count < threads) dropped.Add(L.F("{0} threads", threads - s.Threads.Count));
        return JsonSerializer.Serialize(s);
    }

    /// <summary>A scratch file inside the state folder (never in %TEMP%, which the user's programs can change).</summary>
    static string Scratch() => Path.Combine(Store.Dir, "import-" + Guid.NewGuid().ToString("N") + ".tmp");

    static string ImportGpu(string text, string existing, List<string> dropped)
    {
        if (existing != null) return null;
        string tmp = Scratch();
        GpuProfiles g;
        try
        {
            File.WriteAllText(tmp, text);
            g = GpuProfiles.Load(tmp);       // names cleaned, voltage clamped, duplicates dropped
        }
        finally { try { File.Delete(tmp); } catch (Exception) { } }
        if (g.Profiles.Count == 0) return null;
        if (g.AtStart.Length > 0)
        {
            g.Confirm = g.AtStart;       // not applied until the user says yes in the program's window
            g.AtStart = "";
        }
        return JsonSerializer.Serialize(g, new JsonSerializerOptions { WriteIndented = true });
    }

    static string ImportPresets(string text, string existing, List<string> dropped)
    {
        if (existing != null) return null;
        string tmp = Scratch();
        try
        {
            File.WriteAllText(tmp, text);
            Store.Load(out var presets, out var bg, tmp);     // parsed with the same rules as a normal load
            if (presets.Count == 0 && bg.Count == 0) return null;
            Store.Save(presets, bg, tmp);
            return File.ReadAllText(tmp);
        }
        finally { try { File.Delete(tmp); } catch (Exception) { } try { File.Delete(tmp + ".tmp"); } catch (Exception) { } }
    }
}
