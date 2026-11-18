using System.Text.Json;

namespace DomainScheduler;

/// <summary>One set of GPU overclock values the user saved under a name (MHz offsets and the voltage boost in percent).</summary>
public sealed class GpuProfile
{
    public string Name { get; set; } = "";
    public int CoreMHz { get; set; }
    public int MemMHz { get; set; }
    public int VoltPct { get; set; }

    public bool IsStock => CoreMHz == 0 && MemMHz == 0 && VoltPct == 0;
}

/// <summary>A slider range: whole steps from Min to Max (both on the step grid, Min ≤ 0 ≤ Max when the driver allows it).</summary>
public readonly record struct GpuRange(int Min, int Max, int Step)
{
    /// <summary>Clamps into the range and rounds to the nearest whole step (steps counted from zero).</summary>
    public int Snap(double value)
    {
        if (double.IsNaN(value)) return 0;
        double v = Math.Clamp(value, Min, Max);
        int s = Step < 1 ? 1 : Step;
        int r = (int)Math.Round(v / s, MidpointRounding.AwayFromZero) * s;
        while (r > Max) r -= s;
        while (r < Min) r += s;
        return Math.Clamp(r, Min, Max);
    }
}

/// <summary>The saved GPU profiles: gpu-profiles.json in the data folder.</summary>
public sealed class GpuProfiles
{
    public List<GpuProfile> Profiles { get; set; } = new();
    /// <summary>The profile set again when the program starts (empty = none).</summary>
    public string AtStart { get; set; } = "";
    /// <summary>The profile that was NOT re-applied at the last start because of the crash guard (empty = none).</summary>
    public string Skipped { get; set; } = "";
    public DateTime SkippedUtc { get; set; }

    public const int MaxName = 40;
    public static Func<string> PathHook;
    public static string FilePath => PathHook?.Invoke() ?? Path.Combine(Store.Dir, "gpu-profiles.json");

    static readonly JsonSerializerOptions Opts = new() { WriteIndented = true };

    public GpuProfile Find(string name) =>
        string.IsNullOrWhiteSpace(name) ? null : Profiles.FirstOrDefault(p => string.Equals(p.Name, name.Trim(), StringComparison.OrdinalIgnoreCase));

    public GpuProfile StartProfile => Find(AtStart);

    /// <summary>A usable name: trimmed, 1–40 characters, no control characters. Null when it is not usable.</summary>
    public static string CleanName(string name)
    {
        string n = (name ?? "").Trim();
        if (n.Length == 0 || n.Length > MaxName || n.Any(char.IsControl)) return null;
        return n;
    }

    /// <summary>Saves the values under a name; a profile with the same name (any case) is overwritten. False for a bad name.</summary>
    public bool Put(string name, int coreMHz, int memMHz, int voltPct)
    {
        string n = CleanName(name);
        if (n == null) return false;
        var p = Find(n);
        if (p == null) Profiles.Add(p = new GpuProfile());
        p.Name = n;
        p.CoreMHz = coreMHz;
        p.MemMHz = memMHz;
        p.VoltPct = Math.Clamp(voltPct, 0, 100);
        return true;
    }

    /// <summary>Renames a profile (the "apply at start" mark follows it). False when it is missing, the name is bad or taken.</summary>
    public bool Rename(string from, string to)
    {
        var p = Find(from);
        string n = CleanName(to);
        if (p == null || n == null) return false;
        var other = Find(n);
        if (other != null && other != p) return false;
        bool start = p == StartProfile;
        bool skipped = string.Equals(Skipped, p.Name, StringComparison.OrdinalIgnoreCase);
        p.Name = n;
        if (start) AtStart = n;
        if (skipped) Skipped = n;
        return true;
    }

    /// <summary>Deletes a profile; its "apply at start" mark goes with it.</summary>
    public bool Delete(string name)
    {
        var p = Find(name);
        if (p == null) return false;
        if (p == StartProfile) AtStart = "";
        if (string.Equals(Skipped, p.Name, StringComparison.OrdinalIgnoreCase)) Skipped = "";
        Profiles.Remove(p);
        return true;
    }

    /// <summary>Marks one profile (or none, with null/empty) to be applied at start. Marking clears the crash-guard notice.</summary>
    public bool SetAtStart(string name)
    {
        if (string.IsNullOrWhiteSpace(name)) { AtStart = ""; return true; }
        var p = Find(name);
        if (p == null) return false;
        AtStart = p.Name;
        Skipped = "";
        return true;
    }

    public static GpuProfiles Load(string path = null)
    {
        try
        {
            path ??= FilePath;
            if (!File.Exists(path)) return new GpuProfiles();
            var g = JsonSerializer.Deserialize<GpuProfiles>(File.ReadAllText(path)) ?? new GpuProfiles();
            g.Profiles ??= new();
            g.Profiles = g.Profiles.Where(p => p != null && CleanName(p.Name) != null)
                .GroupBy(p => p.Name.Trim(), StringComparer.OrdinalIgnoreCase).Select(x => x.First()).ToList();
            foreach (var p in g.Profiles) { p.Name = p.Name.Trim(); p.VoltPct = Math.Clamp(p.VoltPct, 0, 100); }
            g.AtStart = g.Find(g.AtStart)?.Name ?? "";
            g.Skipped ??= "";
            return g;
        }
        catch (Exception) { return new GpuProfiles(); }
    }

    /// <summary>Atomic write (temporary file, then a move).</summary>
    public void Save(string path = null)
    {
        path ??= FilePath;
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        string tmp = path + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(this, Opts));
        File.Move(tmp, path, true);
    }
}

/// <summary>The pure decisions of the GPU overclock: slider ranges and what happens at the start of the program.</summary>
public static class GpuOcLogic
{
    public const int CoreStep = 5, MemStep = 25, VoltStep = 1;
    /// <summary>Used when the driver did not report a range: conservative fixed ranges (still cut by NvOc's caps).</summary>
    public static readonly GpuRange CoreFallback = new(-200, 300, CoreStep), MemFallback = new(-500, 2000, MemStep), Volt = new(0, 100, VoltStep);

    /// <summary>The slider range of a clock: the driver's reported offset range cut by NvOc's own caps, on the step grid.</summary>
    public static GpuRange Range(NvClock c, int domain, int step, GpuRange fallback)
    {
        // No reported range: the fixed one, still cut by the same caps the driver write uses.
        c ??= new NvClock { Domain = domain, MinDeltaKHz = fallback.Min * 1000, MaxDeltaKHz = fallback.Max * 1000 };
        int lo = NvOc.Clamp(c, int.MinValue + 1) / 1000, hi = NvOc.Clamp(c, int.MaxValue) / 1000;
        int min = (int)Math.Ceiling(lo / (double)step) * step;   // inward onto the step grid
        int max = (int)Math.Floor(hi / (double)step) * step;
        if (min > max) min = max = 0;
        return new GpuRange(min, max, step);
    }

    public static GpuRange CoreRange(NvClock c) => Range(c, NvOc.DomainCore, CoreStep, CoreFallback);
    public static GpuRange MemRange(NvClock c) => Range(c, NvOc.DomainMemory, MemStep, MemFallback);

    public enum Start { Nothing, Apply, SkipCrash }

    /// <summary>
    /// At the start of the program: apply the marked profile, unless the flag is closed, there is none, it is all zero,
    /// or Windows crashed after the last apply (pending = a blue screen or sudden power-off was logged since then).
    /// </summary>
    public static (Start What, GpuProfile Profile) DecideStart(bool flagOpen, GpuProfiles store, bool pending)
    {
        var p = store?.StartProfile;
        if (!flagOpen || p == null || p.IsStock) return (Start.Nothing, p);
        return pending ? (Start.SkipCrash, p) : (Start.Apply, p);
    }

    /// <summary>
    /// Brings over the "apply at start" values of 1.12 and older (stored in the settings) as a profile marked for the start.
    /// Only when there is no profile file yet; returns true when the store was changed.
    /// </summary>
    public static bool Migrate(GpuProfiles store, bool fileExists, bool atStart, int coreKHz, int memKHz, int voltPct, string name)
    {
        if (fileExists || store == null || !atStart || (coreKHz == 0 && memKHz == 0 && voltPct == 0)) return false;
        if (!store.Put(name, coreKHz / 1000, memKHz / 1000, voltPct)) return false;
        store.SetAtStart(name);
        return true;
    }
}
