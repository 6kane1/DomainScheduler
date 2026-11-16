using System.Text.Json;
using System.Text.RegularExpressions;

namespace DomainScheduler;

/// <summary>
/// Tables as data (section 22.11): known games and dual-CCD X3D models come from the embedded
/// tables.json merged with an optional user file next to the presets (the user file wins).
/// A broken user file is ignored with an error; the code itself knows no model or game.
/// </summary>
public static class Tables
{
    public sealed class Data
    {
        public Dictionary<string, string> KnownGames = new(StringComparer.OrdinalIgnoreCase);   // exe → label
        public Dictionary<string, int> X3d = new(StringComparer.OrdinalIgnoreCase);             // sku → cores
        public bool OverrideUsed;
        public string Error = "";
    }

    static readonly Regex SkuRx = new(@"^\d{4}X3D$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    static readonly object _gate = new();
    static Data _data;
    public static Func<string> EmbeddedHook, OverridePathHook;

    public static string OverridePath => OverridePathHook?.Invoke() ?? Path.Combine(Path.GetDirectoryName(Store.PresetsPath) ?? Store.Dir, "tables.json");

    public static Data Current
    {
        get { lock (_gate) return _data ??= Load(true); }
    }

    public static Dictionary<string, int> X3d => Current.X3d;
    public static Dictionary<string, string> KnownGames => Current.KnownGames;

    public static void Reload()
    {
        lock (_gate) _data = Load(true);
    }

    static string Embedded()
    {
        if (EmbeddedHook != null) return EmbeddedHook();
        using var s = typeof(Tables).Assembly.GetManifestResourceStream("tables.json");
        if (s == null) return "{}";
        using var r = new StreamReader(s);
        return r.ReadToEnd();
    }

    /// <summary>Parses and validates one file into the data; returns how many entries were rejected.</summary>
    static int Merge(Data d, string json)
    {
        int bad = 0;
        using var doc = JsonDocument.Parse(json, new JsonDocumentOptions { AllowTrailingCommas = true, CommentHandling = JsonCommentHandling.Skip });
        if (doc.RootElement.ValueKind != JsonValueKind.Object) throw new JsonException("root is not an object");
        if (doc.RootElement.TryGetProperty("knownGames", out var games))
        {
            if (games.ValueKind != JsonValueKind.Array) throw new JsonException("knownGames is not a list");
            foreach (var g in games.EnumerateArray())
            {
                string exe = g.ValueKind == JsonValueKind.Object && g.TryGetProperty("exe", out var e) && e.ValueKind == JsonValueKind.String ? e.GetString().Trim() : "";
                string label = g.ValueKind == JsonValueKind.Object && g.TryGetProperty("label", out var l) && l.ValueKind == JsonValueKind.String ? l.GetString().Trim() : "";
                if (exe.Length == 0 || exe.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0) { bad++; continue; }
                if (!exe.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) exe += ".exe";
                if (Guard.IsDenied(exe)) { bad++; continue; }       // a denylisted program is never a known game
                d.KnownGames[exe] = label.Length > 0 ? label : exe;
            }
        }
        if (doc.RootElement.TryGetProperty("x3d", out var x3d))
        {
            if (x3d.ValueKind != JsonValueKind.Array) throw new JsonException("x3d is not a list");
            foreach (var x in x3d.EnumerateArray())
            {
                string sku = x.ValueKind == JsonValueKind.Object && x.TryGetProperty("sku", out var s) && s.ValueKind == JsonValueKind.String ? s.GetString().Trim().ToUpperInvariant() : "";
                int cores = x.ValueKind == JsonValueKind.Object && x.TryGetProperty("cores", out var c) && c.ValueKind == JsonValueKind.Number && c.TryGetInt32(out int n) ? n : 0;
                if (!SkuRx.IsMatch(sku) || cores < 4 || cores > 64 || cores % 2 != 0) { bad++; continue; }
                d.X3d[sku] = cores;
            }
        }
        return bad;
    }

    public static Data Load(bool log)
    {
        var d = new Data();
        int bad = 0;
        try { bad = Merge(d, Embedded()); }
        catch (Exception e)
        {
            d.Error = e.Message;
            if (log) AppLog.Add(L.F("Built-in tables: {0}", e.Message), AppLog.Sev.Error);
        }
        string path = OverridePath;
        try
        {
            if (File.Exists(path))
            {
                var merged = new Data();
                foreach (var kv in d.KnownGames) merged.KnownGames[kv.Key] = kv.Value;
                foreach (var kv in d.X3d) merged.X3d[kv.Key] = kv.Value;
                bad += Merge(merged, File.ReadAllText(path));
                merged.OverrideUsed = true;
                d = merged;
            }
        }
        catch (Exception e)
        {
            d.Error = e.Message;       // d is still the embedded data
            if (log) AppLog.Add(L.F("tables.json override is broken ({0}): the built-in tables are used", e.Message), AppLog.Sev.Error);
        }
        if (log)
            AppLog.Add(L.F("Tables: {0} known games, {1} X3D models{2}{3}", d.KnownGames.Count, d.X3d.Count,
                d.OverrideUsed ? L.T(", with the user file") : "", bad > 0 ? L.F(", {0} entries rejected", bad) : ""), bad > 0 ? AppLog.Sev.Warn : AppLog.Sev.Info);
        return d;
    }
}
