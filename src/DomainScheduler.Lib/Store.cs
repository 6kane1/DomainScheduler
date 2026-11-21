using System.Globalization;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace DomainScheduler;

/// <summary>
/// Presets and profiles on disk (section 9). JSON via System.Text.Json (the
/// spec's JavaScriptSerializer does not exist on .NET 8). Not used by the Auto path.
/// </summary>
public static class Store
{
    public static Func<string> DirHook;
    /// <summary>
    /// The data folder: the admin-only state folder (<see cref="StateDir"/>), checked before its first use. Everything
    /// the program keeps lives here: presets, backups, the session, the boot guard, the log, update downloads.
    /// Throws when the folder cannot be made safe (nothing is then read from it or written to it).
    /// </summary>
    public static string Dir => DirHook?.Invoke() ?? StateDir.Ensure();
    /// <summary>The data folder for display (never throws, nothing is checked).</summary>
    public static string DirShown => DirHook?.Invoke() ?? StateDir.PathOnly;
    /// <summary>Where versions before 1.15.0 Test 1.3 kept their data (the user's own folder): only read once by <see cref="LegacyImport"/>.</summary>
    public static string LegacyDir => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "DomainScheduler");
    public static string ProfilesDir => Path.Combine(Dir, "profiles");
    public static string PresetsPath => Path.Combine(Dir, "presets.json");
    public const int Version = 1;

    static readonly JsonSerializerOptions Opts = new() { WriteIndented = true, MaxDepth = 64 };

    static JsonObject PresetJson(Preset p) => new()
    {
        ["exe"] = p.Exe, ["class"] = p.Class, ["io"] = p.Io, ["mem"] = p.Mem, ["eco"] = p.Eco, ["boost"] = p.Boost,
        ["enabled"] = p.Enabled, ["builtin"] = p.Builtin, ["hotFirst"] = p.HotFirst, ["hotOthers"] = p.HotOthers,
        ["threads"] = new JsonArray(p.Threads.Select(t => (JsonNode)new JsonObject
        {
            ["match"] = t.Match, ["pattern"] = t.Pattern, ["prio"] = t.Prio, ["ideal"] = t.Ideal,
            ["boost"] = t.Boost, ["tid"] = t.Tid, ["enabled"] = t.Enabled, ["pin"] = t.Pin,
        }).ToArray()),
    };

    static string S(JsonObject d, string key, string def) => d != null && d.TryGetPropertyValue(key, out var n) && n is JsonValue v && v.TryGetValue(out string s) ? s : def;
    static int I(JsonObject d, string key, int def) => d != null && d.TryGetPropertyValue(key, out var n) && n is JsonValue v && v.TryGetValue(out int i) ? i : def;
    static bool B(JsonObject d, string key, bool def) => d != null && d.TryGetPropertyValue(key, out var n) && n is JsonValue v && v.TryGetValue(out bool b) ? b : def;

    static string Heat(string v) => v is "pin" or "ideal" ? v : "none";

    static Preset PresetFrom(JsonObject o)
    {
        var p = new Preset
        {
            Exe = S(o, "exe", ""), Class = S(o, "class", "normal"), Io = S(o, "io", "normal"), Mem = S(o, "mem", "normal"),
            Eco = S(o, "eco", "off"), Boost = S(o, "boost", ""), Enabled = B(o, "enabled", true), Builtin = B(o, "builtin", false),
            HotFirst = Heat(S(o, "hotFirst", "none")), HotOthers = Heat(S(o, "hotOthers", "none")),
        };
        if (o.TryGetPropertyValue("threads", out var tn) && tn is JsonArray arr)
            foreach (var t in arr.OfType<JsonObject>())
                p.Threads.Add(new ThreadRule
                {
                    Match = S(t, "match", "module"), Pattern = S(t, "pattern", ""), Prio = S(t, "prio", ""), Ideal = S(t, "ideal", ""),
                    Boost = S(t, "boost", ""), Tid = S(t, "tid", ""), Enabled = B(t, "enabled", true), Pin = B(t, "pin", false),
                });
        return p;
    }

    /// <summary>Saves the presets (ordered by exe), the background classes and flags.</summary>
    public static void Save(Dictionary<string, Preset> presets, Dictionary<string, string> bg, string path = null)
    {
        var root = new JsonObject
        {
            ["version"] = Version,
            ["savedUtc"] = DateTime.UtcNow.ToString("o", CultureInfo.InvariantCulture),
            ["presets"] = new JsonArray(presets.Values.OrderBy(p => p.Exe, StringComparer.OrdinalIgnoreCase).Select(p => (JsonNode)PresetJson(p)).ToArray()),
            ["background"] = new JsonObject(bg.OrderBy(k => k.Key, StringComparer.OrdinalIgnoreCase).Select(k => new KeyValuePair<string, JsonNode>(k.Key, k.Value))),
        };
        path ??= PresetsPath;
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        string tmp = path + ".tmp";
        File.WriteAllText(tmp, root.ToJsonString(Opts));
        File.Move(tmp, path, true);
    }

    /// <summary>Loads presets and background classes; returns migration notes (missing fields get defaults).</summary>
    public static int LoadCalls, ListProfilesCalls;

    public static List<string> Load(out Dictionary<string, Preset> presets, out Dictionary<string, string> bg, string path = null)
    {
        Interlocked.Increment(ref LoadCalls);
        var notes = new List<string>();
        presets = new Dictionary<string, Preset>(StringComparer.OrdinalIgnoreCase);
        bg = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        path ??= PresetsPath;
        if (!File.Exists(path)) return notes;
        JsonObject root;
        try { root = JsonNode.Parse(File.ReadAllText(path), documentOptions: new JsonDocumentOptions { MaxDepth = 64 }) as JsonObject; }
        catch (JsonException e) { notes.Add(L.F("presets.json is damaged: {0}", e.Message)); return notes; }
        if (root == null) return notes;
        int ver = root.TryGetPropertyValue("version", out var vn) && vn is JsonValue vv && vv.TryGetValue(out int vi) ? vi : 0;
        if (ver < Version) notes.Add(L.F("presets.json version {0} migrated to {1}", ver, Version));
        if (root.TryGetPropertyValue("presets", out var pn) && pn is JsonArray arr)
            foreach (var o in arr.OfType<JsonObject>())
            {
                var p = PresetFrom(o);
                if (p.Exe.Length > 0) presets[p.Exe] = p;
            }
        if (root.TryGetPropertyValue("background", out var bn) && bn is JsonObject bo)
            foreach (var kv in bo)
                if (kv.Value is JsonValue v && v.TryGetValue(out string s)) bg[kv.Key] = s;
        return notes;
    }

    /// <summary>
    /// Reconciles the stored presets with the built-in ones: a stored copy flagged Builtin is
    /// replaced by the shipped text (built-ins are never edited in place), a user preset with the
    /// same exe stays and overrides the built-in, missing built-ins are added. Returns what changed.
    /// </summary>
    public static List<string> MarkBuiltin(Dictionary<string, Preset> presets, IEnumerable<Preset> builtins)
    {
        var notes = new List<string>();
        var shipped = (builtins ?? Enumerable.Empty<Preset>()).ToDictionary(b => b.Exe, StringComparer.OrdinalIgnoreCase);
        foreach (var key in presets.Keys.ToList())
        {
            var p = presets[key];
            if (!p.Builtin) continue;
            if (!shipped.ContainsKey(key))
            {
                presets.Remove(key);
                notes.Add(L.F("{0}: built-in preset no longer shipped, removed", key));
            }
        }
        foreach (var b in shipped.Values)
        {
            if (presets.TryGetValue(b.Exe, out var p) && !p.Builtin)
            {
                notes.Add(L.F("{0}: your preset overrides the built-in one", b.Exe));
                continue;
            }
            bool had = p != null;
            var c = b.Clone();
            c.Builtin = true;
            if (had) c.Enabled = p.Enabled;
            if (!had) notes.Add(L.F("{0}: built-in preset added", b.Exe));
            else if (!Same(p, c)) notes.Add(L.F("{0}: built-in preset updated", b.Exe));
            presets[b.Exe] = c;
        }
        return notes;
    }

    /// <summary>Two presets say the same (the dirty check of the Presets page and the reconciliation).</summary>
    public static bool Same(Preset a, Preset b) =>
        a != null && b != null && PresetJson(a).ToJsonString() == PresetJson(b).ToJsonString();

    /// <summary>Saves the current presets under a profile name.</summary>
    public static string SaveProfile(string name, Dictionary<string, Preset> presets, Dictionary<string, string> bg)
    {
        string path = ProfilePath(name);
        Save(presets, bg, path);
        return path;
    }

    public static bool CopyProfile(string from, string to)
    {
        string src = ProfilePath(from), dst = ProfilePath(to);
        if (!File.Exists(src) || string.Equals(src, dst, StringComparison.OrdinalIgnoreCase)) return false;
        File.Copy(src, dst, true);
        return true;
    }

    public static bool DeleteProfile(string name)
    {
        string path = ProfilePath(name);
        if (!File.Exists(path)) return false;
        File.Delete(path);
        return true;
    }

    public static string Sanitize(string name)
    {
        var bad = Path.GetInvalidFileNameChars().Concat(new[] { '\\', '/', ':', '*', '?', '"', '<', '>', '|' }).ToHashSet();
        var s = new string((name ?? "").Select(c => bad.Contains(c) || char.IsControl(c) ? '_' : c).ToArray()).Trim();
        return s.Length == 0 ? "profile" : s;
    }

    public static List<string> ListProfiles()
    {
        Interlocked.Increment(ref ListProfilesCalls);
        if (!Directory.Exists(ProfilesDir)) return new List<string>();
        return Directory.GetFiles(ProfilesDir, "*.json").Select(Path.GetFileNameWithoutExtension).OrderBy(x => x, StringComparer.OrdinalIgnoreCase).ToList();
    }

    public static string ProfilePath(string name) => Path.Combine(ProfilesDir, Sanitize(name) + ".json");
}
