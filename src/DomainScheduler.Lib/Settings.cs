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

/// <summary>
/// The settings in the registry (only strings). Two places (1.15.0 Test 1.3):
///  - HKCU\Software\DomainScheduler: preferences of the window (language, position, charts, the watchlist…), which
///    only change how the program looks or which games it may handle;
///  - HKLM\SOFTWARE\Domain OS\Toolbox: the feature flags and every value that makes the program, with administrator
///    rights, change something by itself at start (the GPU overclock at start, the one-shot device clean-ups).
///    Only administrators can write there, so a program running without rights cannot switch them.
/// <see cref="Settings.IsMachine"/> decides which goes where.
/// </summary>
public sealed class RegistrySettingsBackend : ISettingsBackend
{
    public const string Root = @"Software\DomainScheduler";
    public const string LegacyRoot = @"Software\DomainScheduler.Legacy";
    public const string MachineRoot = @"SOFTWARE\Domain OS\Toolbox";

    static string Path(string sub) => string.IsNullOrEmpty(sub) ? Root : Root + "\\" + sub;
    static string MachinePath(string sub) => string.IsNullOrEmpty(sub) ? MachineRoot : MachineRoot + "\\" + sub;
    static (RegistryKey Hive, string Path) Where(string sub, string name) =>
        Settings.IsMachine(sub, name) ? (Registry.LocalMachine, MachinePath(sub)) : (Registry.CurrentUser, Path(sub));

    public bool TryGet(string sub, string name, out string value)
    {
        value = null;
        try
        {
            var (hive, path) = Where(sub, name);
            using var k = hive.OpenSubKey(path);
            object v = k?.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames);
            if (v == null) return false;
            value = Convert.ToString(v, CultureInfo.InvariantCulture) ?? "";
            return true;
        }
        catch (Exception) { return false; }
    }

    public void Set(string sub, string name, string value)
    {
        var (hive, path) = Where(sub, name);
        using var k = hive.CreateSubKey(path, true);
        k.SetValue(name, value ?? "", RegistryValueKind.String);
    }

    public void Delete(string sub, string name)
    {
        var (hive, path) = Where(sub, name);
        using var k = hive.OpenSubKey(path, true);
        k?.DeleteValue(name, false);
    }

    public IEnumerable<string> Names(string sub)
    {
        var (hive, path) = Where(sub, null);
        using var k = hive.OpenSubKey(path);
        return k?.GetValueNames().ToList() ?? new List<string>();
    }

    /// <summary>
    /// Once per Windows account: the values that moved to HKLM are copied over from this account's HKCU. Only known
    /// names with a valid value (<see cref="Settings.MachineValueValid"/>) are taken, and only where HKLM has none yet;
    /// the HKCU copies stay (an older version reads them) but are never read again. Returns lines for the log.
    /// </summary>
    public static List<string> MigrateToMachine(string account)
    {
        var log = new List<string>();
        using var lm = Registry.LocalMachine.CreateSubKey(MachineRoot, true);
        string marker = "ImportedFrom-" + account;
        if (lm.GetValue(marker) != null) return log;
        using var cu = Registry.CurrentUser.OpenSubKey(Root);
        var plan = Settings.MachineImport(sub =>
        {
            using var k = string.IsNullOrEmpty(sub) ? null : cu?.OpenSubKey(sub);
            var key = string.IsNullOrEmpty(sub) ? cu : k;
            return key == null ? new Dictionary<string, string>() : key.GetValueNames().ToDictionary(n => n, n => Convert.ToString(key.GetValue(n, null, RegistryValueOptions.DoNotExpandEnvironmentNames), CultureInfo.InvariantCulture) ?? "", StringComparer.OrdinalIgnoreCase);
        }, (sub, name) =>
        {
            using var k = Registry.LocalMachine.OpenSubKey(MachinePath(sub));
            return k?.GetValue(name) != null;
        }, log);
        foreach (var (sub, name, value) in plan)
        {
            using var k = Registry.LocalMachine.CreateSubKey(MachinePath(sub), true);
            k.SetValue(name, value, RegistryValueKind.String);
        }
        lm.SetValue(marker, DateTime.UtcNow.ToString("o", CultureInfo.InvariantCulture), RegistryValueKind.String);
        return log;
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

    /// <summary>
    /// Values kept in HKLM (administrators only): they make the program act by itself with administrator rights.
    /// The whole Flags subkey goes there too.
    /// </summary>
    public static readonly HashSet<string> MachineKeys = new(StringComparer.OrdinalIgnoreCase)
    {
        "GpuOcAtStart", "GpuOcCoreKHz", "GpuOcMemKHz", "GpuOcVoltPct", "GpuOcAppliedUtc", "SafeDeviceLayout112", "LegacyRawMouseChecked",
    };

    public static bool IsMachine(string sub, string name) =>
        string.Equals(sub, Flags.Sub, StringComparison.OrdinalIgnoreCase) || (string.IsNullOrEmpty(sub) && name != null && MachineKeys.Contains(name));

    /// <summary>
    /// Whether a value read from HKCU may be copied to HKLM (pure). Flags: a known flag, "true"/"false". The one-shot
    /// markers: only "true" (a planted "false" would make an old clean-up run again). The GPU values: integers in the
    /// range the program writes and a time stamp. GpuOcAtStart is never taken: an overclock applied at start has to be
    /// chosen again in the program (it cannot be told from one planted by another program).
    /// </summary>
    public static bool MachineValueValid(string sub, string name, string value)
    {
        value ??= "";
        if (string.Equals(sub, Flags.Sub, StringComparison.OrdinalIgnoreCase))
            return Flags.FlagKeys.Contains(name) && (value == "true" || value == "false");
        if (!string.IsNullOrEmpty(sub)) return false;
        bool Int(int lo, int hi) => int.TryParse(value, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out int v) && v >= lo && v <= hi;
        return name switch
        {
            "SafeDeviceLayout112" or "LegacyRawMouseChecked" => value == "true",
            "GpuOcCoreKHz" => Int(-1_000_000, 1_000_000),
            "GpuOcMemKHz" => Int(-3_000_000, 3_000_000),
            "GpuOcVoltPct" => Int(0, 100),
            "GpuOcAppliedUtc" => value.Length == 0 || DateTime.TryParse(value, CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind, out _),
            _ => false,
        };
    }

    /// <summary>What <see cref="RegistrySettingsBackend.MigrateToMachine"/> copies (pure): read(sub) gives the HKCU values of a subkey.</summary>
    public static List<(string Sub, string Name, string Value)> MachineImport(Func<string, Dictionary<string, string>> read, Func<string, string, bool> machineHas, List<string> log)
    {
        var plan = new List<(string, string, string)>();
        foreach (var sub in new[] { "", Flags.Sub })
            foreach (var (name, value) in read(sub))
            {
                if (!IsMachine(sub, name)) continue;
                if (!MachineValueValid(sub, name, value)) { log?.Add(L.F("Settings: {0} was not taken over from the user settings (not a value the program writes)", (sub.Length > 0 ? sub + "\\" : "") + name)); continue; }
                if (machineHas(sub, name)) continue;
                plan.Add((sub, name, value));
            }
        return plan;
    }

    /// <summary>Keys the product reads (section 17.1).</summary>
    public static readonly HashSet<string> ProductKeys = new(StringComparer.OrdinalIgnoreCase) { "Language", "WindowLeft", "WindowTop", "Autostart" };

    static readonly object _gate = new();
    static readonly Dictionary<string, int> _reads = new(StringComparer.OrdinalIgnoreCase);

    static readonly Dictionary<string, Func<string>> Defaults = new(StringComparer.OrdinalIgnoreCase)
    {
        ["MaskName"] = DefaultMask,
        ["IrqMode"] = () => "Off",          // 1.16.0: owner's setting (manual mask rules do not step around pinned devices)
        ["DoMask"] = () => "true",
        ["DoThreads"] = () => "true",
        ["DoBackground"] = () => "true",
        ["BgAffinity"] = () => "true",
        ["ReapplyMs"] = () => ThreadIni.LoadBundled().Sleep.ToString(CultureInfo.InvariantCulture),
        ["SysPresets"] = () => "true",
        ["BgOn"] = () => "true",
        ["Top5On"] = () => "true",   // no longer read since 1.16.0 Test 1.9: Top-5 is always on
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
        ["Advanced"] = () => "true",        // 1.16.0: the advanced layout by default (owner's setting)
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
