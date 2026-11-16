using System.Text.RegularExpressions;

namespace DomainScheduler;

/// <summary>One cache the cleaner knows: where it is, what Scan found, whether its owner runs.</summary>
public sealed class CleanItem
{
    public string Key = "", Name = "", Owner = "";     // Owner: the process that must not run while cleaning
    public List<string> Roots = new();
    public bool AskFirst, Running, Scanned;
    public long Size;
    public int Files;
    public string ScanNote = "";
    public DateTime Oldest = DateTime.MaxValue, Newest = DateTime.MinValue;
    internal List<string> Found = new();
}

/// <summary>
/// The cache cleaner (a separate module, never called from Auto). Rules: nothing is deleted
/// without a Scan, guarded names are never deleted, a cache whose owner runs is skipped,
/// AskFirst caches need a consent, reparse points are not entered, Steam: caches and
/// unfinished downloads only.
/// </summary>
public static class Cleaner
{
    /// <summary>Test hooks.</summary>
    public static Func<string, string> EnvHook;
    public static Func<HashSet<string>> RunningHook;

    static readonly string[] GuardNames =
    {
        "libraryfolders.vdf", "appmanifest_", ".acf", "userdata", "config", "common", "workshop", "prefs", "local-settings", "Users",
    };

    static string Env(string name) => EnvHook?.Invoke(name) ?? Environment.GetEnvironmentVariable(name) ?? "";

    /// <summary>Names the cleaner never deletes whatever list they are found in.</summary>
    public static bool Guard(string path)
    {
        string name = Path.GetFileName(path.TrimEnd('\\', '/'));
        return GuardNames.Any(g => name.StartsWith(g, StringComparison.OrdinalIgnoreCase) || name.EndsWith(g, StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>Steam library folders: the install folder and what libraryfolders.vdf lists.</summary>
    public static List<string> SteamLibraries()
    {
        var list = new List<string>();
        string pf = Env("ProgramFiles(x86)");
        if (pf.Length == 0) return list;
        string root = Path.Combine(pf, "Steam");
        if (!Directory.Exists(root)) return list;
        list.Add(root);
        try
        {
            string vdf = Path.Combine(root, "steamapps", "libraryfolders.vdf");
            if (File.Exists(vdf))
                foreach (Match m in Regex.Matches(File.ReadAllText(vdf), "\"path\"\\s+\"([^\"]+)\""))
                {
                    string p = m.Groups[1].Value.Replace(@"\\", @"\");
                    if (Directory.Exists(p) && !list.Contains(p, StringComparer.OrdinalIgnoreCase)) list.Add(p);
                }
        }
        catch (Exception) { }
        return list;
    }

    /// <summary>Spotify's own cache limit in MB from its prefs file (0 = not set). Read only.</summary>
    public static int SpotifyLimit()
    {
        try
        {
            string prefs = Path.Combine(Env("APPDATA"), "Spotify", "prefs");
            if (!File.Exists(prefs)) return 0;
            foreach (var line in File.ReadAllLines(prefs))
                if (line.StartsWith("storage.size=", StringComparison.Ordinal) && int.TryParse(line[13..].Trim(), out int mb)) return mb;
        }
        catch (Exception) { }
        return 0;
    }

    public static List<CleanItem> Items()
    {
        string local = Env("LOCALAPPDATA"), temp = Env("TEMP");
        var items = new List<CleanItem>();
        void Add(string key, string name, string owner, bool ask, params string[] roots)
        {
            var r = roots.Where(x => !string.IsNullOrWhiteSpace(x) && Path.IsPathRooted(x)).ToList();
            if (r.Count > 0) items.Add(new CleanItem { Key = key, Name = name, Owner = owner, AskFirst = ask, Roots = r });
        }
        if (temp.Length > 0) Add("temp", L.T("Temporary files"), "", false, temp);
        if (local.Length > 0)
        {
            Add("d3d", L.T("DirectX shader cache"), "", false, Path.Combine(local, "D3DSCache"));
            Add("nvidia", L.T("NVIDIA shader cache"), "", true, Path.Combine(local, "NVIDIA", "DXCache"), Path.Combine(local, "NVIDIA", "GLCache"));
            Add("amd", L.T("AMD shader cache"), "", true, Path.Combine(local, "AMD", "DxCache"), Path.Combine(local, "AMD", "DxcCache"), Path.Combine(local, "AMD", "VkCache"));
            Add("spotify", L.T("Spotify cache"), "Spotify.exe", false, Path.Combine(local, "Spotify", "Storage"), Path.Combine(local, "Spotify", "Data"));
        }
        var libs = SteamLibraries();
        if (libs.Count > 0)
        {
            Add("steam-shader", L.T("Steam shader cache"), "steam.exe", true, libs.Select(l => Path.Combine(l, "steamapps", "shadercache")).ToArray());
            Add("steam-download", L.T("Steam unfinished downloads"), "steam.exe", true, libs.Select(l => Path.Combine(l, "steamapps", "downloading")).ToArray());
        }
        return items;
    }

    static HashSet<string> RunningNow()
    {
        if (RunningHook != null) return RunningHook();
        try { return new HashSet<string>(Native.Processes().Values.Select(p => p.Name), StringComparer.OrdinalIgnoreCase); }
        catch (Exception) { return new HashSet<string>(StringComparer.OrdinalIgnoreCase); }
    }

    static bool Reparse(FileSystemInfo f) => (f.Attributes & FileAttributes.ReparsePoint) != 0;

    static void Walk(DirectoryInfo dir, CleanItem item, ref int denied)
    {
        FileSystemInfo[] entries;
        try { entries = dir.GetFileSystemInfos(); }
        catch (Exception) { denied++; return; }
        foreach (var e in entries)
        {
            try
            {
                if (Reparse(e) || Guard(e.FullName)) continue;
                if (e is DirectoryInfo d) Walk(d, item, ref denied);
                else if (e is FileInfo f)
                {
                    item.Found.Add(f.FullName);
                    item.Size += f.Length;
                    item.Files++;
                    if (f.LastWriteTimeUtc < item.Oldest) item.Oldest = f.LastWriteTimeUtc;
                    if (f.LastWriteTimeUtc > item.Newest) item.Newest = f.LastWriteTimeUtc;
                }
            }
            catch (Exception) { denied++; }
        }
    }

    /// <summary>Counts what an item holds; nothing is changed.</summary>
    public static void Scan(CleanItem item)
    {
        item.Found = new List<string>();
        item.Size = 0;
        item.Files = 0;
        item.Oldest = DateTime.MaxValue;
        item.Newest = DateTime.MinValue;
        item.Running = item.Owner.Length > 0 && RunningNow().Contains(item.Owner);
        int denied = 0, missing = 0;
        foreach (var root in item.Roots)
        {
            var d = new DirectoryInfo(root);
            if (!d.Exists || Reparse(d)) { missing++; continue; }
            Walk(d, item, ref denied);
        }
        item.Scanned = true;
        item.ScanNote = missing == item.Roots.Count ? L.T("not found")
            : item.Running ? L.F("{0} is running: skipped when cleaning", item.Owner)
            : denied > 0 ? L.F("{0} folders could not be read", denied) : "";
    }

    /// <summary>
    /// Deletes exactly the files the last Scan listed. Returns (deleted files, freed bytes, note).
    /// </summary>
    public static (int Files, long Bytes, string Note) Clean(CleanItem item, bool consent)
    {
        if (!item.Scanned) return (0, 0, L.T("Scan first"));
        if (item.AskFirst && !consent) return (0, 0, L.T("Needs your consent"));
        if (item.Owner.Length > 0 && RunningNow().Contains(item.Owner))
        {
            item.Running = true;
            return (0, 0, L.F("{0} is running: skipped", item.Owner));
        }
        int n = 0, locked = 0;
        long bytes = 0;
        foreach (var path in item.Found)
        {
            try
            {
                if (!item.Roots.Any(r => path.StartsWith(r.TrimEnd('\\', '/') + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))) continue;
                var f = new FileInfo(path);
                if (!f.Exists || Reparse(f) || Guard(path)) continue;
                long len = f.Length;
                f.Delete();
                n++;
                bytes += len;
            }
            catch (Exception) { locked++; }
        }
        item.Scanned = false;      // the next Clean needs a new Scan
        item.Found = new List<string>();
        return (n, bytes, locked > 0 ? L.F("{0} files are in use and stayed", locked) : "");
    }
}
