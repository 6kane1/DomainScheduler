using System.Globalization;
using Microsoft.Win32;

namespace DomainScheduler;

/// <summary>String storage behind Settings (the registry, or memory in tests).</summary>
public interface ISettingsBackend
{
    bool TryGet(string sub, string name, out string value);   // value exists (maybe empty)
    void Set(string sub, string name, string value);
    void Delete(string sub, string name);
    IEnumerable<string> Names(string sub);
}

/// <summary>HKCU\Software\DomainScheduler (only strings).</summary>
public sealed class RegistrySettingsBackend : ISettingsBackend
{
    public const string Root = @"Software\DomainScheduler";
    public const string LegacyRoot = @"Software\DomainScheduler.Legacy";

    static string Path(string sub) => string.IsNullOrEmpty(sub) ? Root : Root + "\\" + sub;

    public bool TryGet(string sub, string name, out string value)
    {
        value = null;
        try
        {
            using var k = Registry.CurrentUser.OpenSubKey(Path(sub));
            object v = k?.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames);
            if (v == null) return false;
            value = Convert.ToString(v, CultureInfo.InvariantCulture) ?? "";
            return true;
        }
        catch (Exception) { return false; }
    }

    public void Set(string sub, string name, string value)
    {
        using var k = Registry.CurrentUser.CreateSubKey(Path(sub), true);
        k.SetValue(name, value ?? "", RegistryValueKind.String);
    }

    public void Delete(string sub, string name)
    {
        using var k = Registry.CurrentUser.OpenSubKey(Path(sub), true);
        k?.DeleteValue(name, false);
    }

    public IEnumerable<string> Names(string sub)
    {
        using var k = Registry.CurrentUser.OpenSubKey(Path(sub));
        return k?.GetValueNames().ToList() ?? new List<string>();
    }

    /// <summary>Copies the legacy root (values with their kinds, subkeys recursively) once.</summary>
    public static int MigrateFromLegacy()
    {
        using var old = Registry.CurrentUser.OpenSubKey(LegacyRoot);
        if (old == null) return 0;
        using var cur = Registry.CurrentUser.CreateSubKey(Root, true);
        if (cur.GetValue("Migrated") is string m && m == "true") return 0;
        int n = Copy(old, cur);
        cur.SetValue("Migrated", "true", RegistryValueKind.String);
        return n;
    }

    static int Copy(RegistryKey from, RegistryKey to)
    {
        int n = 0;
        foreach (var name in from.GetValueNames())
        {
            if (to.GetValue(name) != null) continue;
            to.SetValue(name, from.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames), from.GetValueKind(name));
            n++;
        }
        foreach (var sub in from.GetSubKeyNames())
        {
            using var f = from.OpenSubKey(sub);
            using var t = to.CreateSubKey(sub, true);
            n += Copy(f, t);
        }
        return n;
    }
}

/// <summary>In-memory backend (tests).</summary>
public sealed class MemorySettingsBackend : ISettingsBackend
{
    readonly Dictionary<string, string> _v = new(StringComparer.OrdinalIgnoreCase);
    public bool TryGet(string sub, string name, out string value) => _v.TryGetValue(sub + "\\" + name, out value);
    public void Set(string sub, string name, string value) => _v[sub + "\\" + name] = value ?? "";
    public void Delete(string sub, string name) => _v.Remove(sub + "\\" + name);
    public IEnumerable<string> Names(string sub) => _v.Keys.Where(k => k.StartsWith(sub + "\\", StringComparison.OrdinalIgnoreCase)).Select(k => k[(sub.Length + 1)..]).ToList();
}

/// <summary>
/// Application settings (section 9). In the product (Auto, section 17) only the
/// UI language, the window geometry and the autostart flag are read; every
/// read is counted so tests can prove the Auto path reads nothing else.
/// </summary>
public static class Settings
{
    public static ISettingsBackend Backend = new RegistrySettingsBackend();
    public const string AffinitySub = "Affinity";

    /// <summary>Keys the product reads (section 17.1).</summary>
    public static readonly HashSet<string> ProductKeys = new(StringComparer.OrdinalIgnoreCase) { "Language", "WindowLeft", "WindowTop", "Autostart" };

    static readonly object _gate = new();
    static readonly Dictionary<string, int> _reads = new(StringComparer.OrdinalIgnoreCase);

    static readonly Dictionary<string, Func<string>> Defaults = new(StringComparer.OrdinalIgnoreCase)
    {
        ["MaskName"] = DefaultMask,
        ["IrqMode"] = () => "Core",
        ["DoMask"] = () => "true",
        ["DoThreads"] = () => "true",
        ["DoBackground"] = () => "true",
        ["BgAffinity"] = () => "true",
        ["ReapplyMs"] = () => ThreadIni.LoadBundled().Sleep.ToString(CultureInfo.InvariantCulture),
        ["SysPresets"] = () => "true",
        ["BgOn"] = () => "true",
        ["Top5On"] = () => "false",
        ["GpuOcCoreKHz"] = () => "0",
        ["GpuOcMemKHz"] = () => "0",
        ["GpuOcVoltPct"] = () => "0",
        ["GpuOcAtStart"] = () => "false",
        ["GpuOcAppliedUtc"] = () => "",
        ["SoftCloseOn"] = () => "false",
        ["ClosableList"] = () => "",
        ["TimerResOn"] = () => "true",
        ["EtwIrq"] = () => "false",
        ["WatchList"] = () => "cs2.exe",
        ["MaskNoSmt"] = () => "false",
        ["MaskNoCore0"] = () => "false",
        ["GameClass"] = () => "above",
        ["AllowElevatedPriority"] = () => "false",
        ["Autostart"] = () => "true",
        ["Language"] = () => L.Detect(),
    };

    static string DefaultMask()
    {
        try
        {
            var t = TopoBuilder.Live();
            if (t == null) return "auto";
            return t.Split.CanSplit && t.Split.Family == "x3d" ? "vcache" : t.Hybrid ? "pcores" : "auto";
        }
        catch (Exception) { return "auto"; }
    }

    static void Count(string name)
    {
        lock (_gate) _reads[name] = _reads.GetValueOrDefault(name) + 1;
    }

    /// <summary>Reads per key since the last reset (tests).</summary>
    public static Dictionary<string, int> Reads()
    {
        lock (_gate) return new Dictionary<string, int>(_reads, StringComparer.OrdinalIgnoreCase);
    }

    public static void ResetReads()
    {
        lock (_gate) _reads.Clear();
    }

    /// <summary>Non-empty stored value, else the fallback, else the default, else "".</summary>
    public static string Get(string name, string fallback = null)
    {
        Count(name);
        if (Backend.TryGet("", name, out var v) && !string.IsNullOrEmpty(v)) return v;
        if (!string.IsNullOrEmpty(fallback)) return fallback;
        return Defaults.TryGetValue(name, out var d) ? d() : "";
    }

    public static bool GetBool(string name, bool fallback = false)
    {
        string s = Get(name);
        return s.Length == 0 ? fallback : s.Equals("true", StringComparison.OrdinalIgnoreCase);
    }

    public static int GetInt(string name, int fallback) =>
        int.TryParse(Get(name, fallback.ToString(CultureInfo.InvariantCulture)), NumberStyles.Integer, CultureInfo.InvariantCulture, out int n) ? n : fallback;

    public static void Set(string name, string value) => Backend.Set("", name, value ?? "");

    public static void Reset(string name) => Backend.Delete("", name);

    /// <summary>The stored language code when there is a resource for it, else the detected one.</summary>
    public static string Language()
    {
        string code = Backend.TryGet("", "Language", out var v) ? (v ?? "").Trim().ToLowerInvariant() : "";
        return L.Available.Contains(code) ? code : L.Detect();
    }

    /// <summary>The watchlist: the only processes whose scheduling the user asked to change.</summary>
    public static List<string> Watch() => WatchList(Get("WatchList"));

    public static void SetWatch(IEnumerable<string> exes) => Set("WatchList", string.Join(", ", WatchList(string.Join(",", exes))));

    public static void SetBool(string name, bool value) => Set(name, value ? "true" : "false");

    /// <summary>The value exists (even empty): one-shot defaults hang on this.</summary>
    public static bool TryStored(string name) => Backend.TryGet("", name, out _);

    /// <summary>The value exists and is not empty.</summary>
    public static bool Stored(string name) => Backend.TryGet("", name, out var v) && !string.IsNullOrEmpty(v);

    /// <summary>"game, other.exe; game" → ["game.exe", "other.exe"].</summary>
    public static List<string> WatchList(string text)
    {
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var list = new List<string>();
        foreach (var raw in (text ?? "").Split(new[] { ',', ';', '+' }, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            string exe = raw.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? raw : raw + ".exe";
            if (seen.Add(exe)) list.Add(exe);
        }
        return list;
    }

    /// <summary>Per-game mask rules (subkey Affinity, value name = game.exe).</summary>
    public static int LoadAffRulesCalls;

    public static Dictionary<string, string> LoadAffRules()
    {
        Interlocked.Increment(ref LoadAffRulesCalls);
        var d = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var n in Backend.Names(AffinitySub))
            if (Backend.TryGet(AffinitySub, n, out var v) && !string.IsNullOrEmpty(v)) d[n] = v;
        return d;
    }

    public static void SaveAffRule(string exe, string rule)
    {
        if (string.IsNullOrEmpty(rule)) Backend.Delete(AffinitySub, exe);
        else Backend.Set(AffinitySub, exe, rule);
    }
}
