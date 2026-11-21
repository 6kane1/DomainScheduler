using System.Globalization;
using System.Text.Json;
using System.Diagnostics;
using Microsoft.Win32;

namespace DomainScheduler;

/// <summary>Registry access of the system-changes page (the real registry in the product, a dictionary in tests).</summary>
public interface IRegStore
{
    /// <summary>The value as text ("" for an empty string), null when it does not exist.</summary>
    string Get(string hive, string path, string name);
    void Set(string hive, string path, string name, string kind, string data);
    void Delete(string hive, string path, string name);
    /// <summary>The key exists (services: the service is installed on this Windows).</summary>
    bool HasKey(string hive, string path);
}

/// <summary>
/// The real registry. The pseudo hive "BCD" is the boot configuration of the running Windows: path is the entry
/// ("{current}"), name the element ("hypervisorlaunchtype"); it is read and written with bcdedit.
/// </summary>
public sealed class WinRegStore : IRegStore
{
    static RegistryKey Root(string hive) => hive == "HKLM" ? Registry.LocalMachine : Registry.CurrentUser;

    public string Get(string hive, string path, string name)
    {
        if (hive == "BCD") return BcdGet(path, name);
        using var k = Root(hive).OpenSubKey(path);
        object v = k?.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames);
        if (v is byte[] bytes) return Convert.ToHexString(bytes);
        return v == null ? null : Convert.ToString(v, CultureInfo.InvariantCulture);
    }

    public void Set(string hive, string path, string name, string kind, string data)
    {
        if (hive == "BCD") { Bcd("/set", path, name, data); return; }
        using var k = Root(hive).CreateSubKey(path, true);
        if (kind == "dword") k.SetValue(name, int.Parse(data, CultureInfo.InvariantCulture), RegistryValueKind.DWord);
        else if (kind == "binary") k.SetValue(name, Convert.FromHexString(data ?? ""), RegistryValueKind.Binary);
        else k.SetValue(name, data ?? "", RegistryValueKind.String);
    }

    public void Delete(string hive, string path, string name)
    {
        if (hive == "BCD") { Bcd("/deletevalue", path, name); return; }
        using var k = Root(hive).OpenSubKey(path, true);
        k?.DeleteValue(name, false);
    }

    public bool HasKey(string hive, string path)
    {
        if (hive == "BCD") return true;
        using var k = Root(hive).OpenSubKey(path);
        return k != null;
    }

    /// <summary>
    /// bcdedit with every part a separate, checked argument (no command line is put together from text): the entry is
    /// "{current}" or another "{…}" identifier, the element a plain word, the value letters and digits only.
    /// </summary>
    static string Bcd(string verb, string entry, string element = null, string value = null)
    {
        if (!BcdPart.IsMatch(entry ?? "") || !entry.StartsWith('{') || (element != null && !BcdPart.IsMatch(element)) || (value != null && !BcdPart.IsMatch(value)))
            throw new InvalidOperationException("bcdedit: refused (not a boot entry, element or value this program uses)");
        var psi = SysExe.Start("bcdedit.exe");
        psi.ArgumentList.Add(verb);
        psi.ArgumentList.Add(entry);
        if (element != null) psi.ArgumentList.Add(element);
        if (value != null) psi.ArgumentList.Add(value);
        using var p = Process.Start(psi)!;
        var err = p.StandardError.ReadToEndAsync();
        string output = p.StandardOutput.ReadToEnd() + err.Result;
        p.WaitForExit(10000);
        if (p.ExitCode != 0 && verb != "/enum" && verb != "/deletevalue")
            throw new InvalidOperationException("bcdedit " + verb + " " + entry + " " + element + ": " + output.Trim());
        return output;
    }

    static readonly System.Text.RegularExpressions.Regex BcdPart = new(@"^(\{[A-Za-z0-9\-]{1,40}\}|[A-Za-z0-9]{1,40})$", System.Text.RegularExpressions.RegexOptions.CultureInvariant);

    /// <summary>Element names in "bcdedit /enum" are not translated; the value is the second word of the line.</summary>
    static string BcdGet(string entry, string name)
    {
        foreach (var line in Bcd("/enum", entry).Split('\n'))
        {
            var parts = line.Trim().Split((char[])null, 2, StringSplitOptions.RemoveEmptyEntries);
            if (parts.Length == 2 && parts[0].Equals(name, StringComparison.OrdinalIgnoreCase)) return parts[1].Trim();
        }
        return null;
    }
}

public sealed class MemoryRegStore : IRegStore
{
    public readonly Dictionary<string, string> Values = new(StringComparer.OrdinalIgnoreCase);
    public int Writes;
    static string Key(string hive, string path, string name) => hive + "\\" + path + "\\" + name;
    public string Get(string hive, string path, string name) => Values.TryGetValue(Key(hive, path, name), out var v) ? v : null;
    public void Set(string hive, string path, string name, string kind, string data) { Writes++; Values[Key(hive, path, name)] = data ?? ""; }
    public void Delete(string hive, string path, string name) { Writes++; Values.Remove(Key(hive, path, name)); }
    /// <summary>Keys reported as not installed (everything else exists).</summary>
    public readonly HashSet<string> Missing = new(StringComparer.OrdinalIgnoreCase);
    public bool HasKey(string hive, string path) => !Missing.Contains(hive + "\\" + path);
}

/// <summary>One registry value of a change: what it is when the change is on, and what Windows has by default (null = no value).</summary>
public sealed class RegChange
{
    public string Hive = "HKCU", Path = "", Name = "", Kind = "dword", On = "", Default;
    /// <summary>Skipped when its key does not exist (a service that this Windows does not have); never creates the key.</summary>
    public bool IfKeyExists;
    public RegChange(string hive, string path, string name, string kind, string on, string def)
    {
        Hive = hive; Path = path; Name = name; Kind = kind; On = on; Default = def;
    }
}

/// <summary>A documented system change: what it does, why it is offered, what you give up.</summary>
public sealed class Tweak
{
    public string Id = "", Group = "", Title = "", What = "", Why = "", Cost = "";
    public bool Recommended = true;
    /// <summary>Takes effect only after a restart of Windows.</summary>
    public bool NeedsRestart;
    /// <summary>Runs before Apply; a non-null text stops it (for example "turn off Tamper Protection first").</summary>
    public Func<string> PreCheck;
    public List<RegChange> Changes = new();
}

public enum TweakState { Off, On, Mixed }

/// <summary>
/// The catalogue of system changes (the same list the Domain OS playbook applies), each with its
/// explanation. Apply saves the previous values first; Revert puts exactly those back, or the
/// Windows defaults when the change was made by something else. Nothing here touches security (the protection
/// switches of older versions are only reverted, see <see cref="RevertRetired"/>).
/// </summary>
public static class Tweaks
{
    public static IRegStore Reg = new WinRegStore();
    public static Func<string> BackupPathHook;
    public static string BackupPath => BackupPathHook?.Invoke() ?? Path.Combine(Store.Dir, BackupName);

    const string Cdm = @"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";
    const string Wu = @"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate";

    static Tweak T(string id, string group, string title, string what, string why, string cost, params RegChange[] changes) =>
        new() { Id = id, Group = group, Title = title, What = what, Why = why, Cost = cost, Changes = changes.ToList() };

    static RegChange D(string hive, string path, string name, int on, int? def) =>
        new(hive, path, name, "dword", on.ToString(CultureInfo.InvariantCulture), def?.ToString(CultureInfo.InvariantCulture));

    static RegChange S(string hive, string path, string name, string on, string def) => new(hive, path, name, "sz", on, def);

    public static IReadOnlyList<Tweak> Catalog { get; } = Build();

    static List<Tweak> Build() => new()
    {
        T("game-dvr", L.N("Games"), L.N("Background recording off (Game DVR)"),
            L.N("Windows stops recording the last minutes of every game in the background."),
            L.N("Background recording takes GPU encoder and disk time in every game, whether you ever save a clip or not."),
            L.N("\"Record what happened\" (Win+Alt+G) no longer works. Recording by hand (Win+Alt+R) still does."),
            D("HKCU", @"System\GameConfigStore", "GameDVR_Enabled", 0, 1),
            D("HKCU", @"Software\Microsoft\Windows\CurrentVersion\GameDVR", "AppCaptureEnabled", 0, 1),
            D("HKCU", @"Software\Microsoft\Windows\CurrentVersion\GameDVR", "HistoricalCaptureEnabled", 0, null)),
        T("mouse-accel", L.N("Games"), L.N("Pointer acceleration off"),
            L.N("\"Enhance pointer precision\" is switched off."),
            L.N("With acceleration the cursor travels further when the hand moves faster; without it the same hand movement always gives the same distance, which aiming relies on."),
            L.N("The pointer feels slower on the desktop at first. Games that read raw input are not affected either way."),
            S("HKCU", @"Control Panel\Mouse", "MouseSpeed", "0", "1"),
            S("HKCU", @"Control Panel\Mouse", "MouseThreshold1", "0", "6"),
            S("HKCU", @"Control Panel\Mouse", "MouseThreshold2", "0", "10")),
        T("sticky-keys", L.N("Games"), L.N("Sticky Keys shortcuts off"),
            L.N("Five presses of Shift, holding Shift for 8 seconds and holding Num Lock no longer open the accessibility prompts."),
            L.N("These prompts throw a window over a full-screen game at the worst moment."),
            L.N("The accessibility features themselves stay; only their keyboard shortcuts are off."),
            S("HKCU", @"Control Panel\Accessibility\StickyKeys", "Flags", "506", "510"),
            S("HKCU", @"Control Panel\Accessibility\Keyboard Response", "Flags", "122", "126"),
            S("HKCU", @"Control Panel\Accessibility\ToggleKeys", "Flags", "58", "62")),

        T("telemetry", L.N("Privacy"), L.N("Diagnostic data: required only"),
            L.N("Windows sends only the required diagnostic data."),
            L.N("Optional diagnostic data includes browsing and app-usage details that are of no use to the owner of the PC."),
            L.N("Nothing. The value 0 is not used: Home and Pro ignore it."),
            D("HKLM", @"SOFTWARE\Policies\Microsoft\Windows\DataCollection", "AllowTelemetry", 1, null)),
        T("ad-id", L.N("Privacy"), L.N("Advertising ID and tailored experiences off"),
            L.N("Apps do not get an advertising identifier, and Windows does not build tips and ads from your diagnostic data."),
            L.N("Less profiling; no effect on how Windows or games work."),
            L.N("Ads inside Store apps become generic instead of personal."),
            D("HKCU", @"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo", "Enabled", 0, 1),
            D("HKCU", @"Software\Microsoft\Windows\CurrentVersion\Privacy", "TailoredExperiencesWithDiagnosticDataEnabled", 0, 1)),
        T("activity", L.N("Privacy"), L.N("Activity history off"),
            L.N("Windows does not collect or upload the list of apps and files you used."),
            L.N("The history is only used for cross-device suggestions."),
            L.N("\"Pick up where you left off\" across devices stops working."),
            D("HKLM", @"SOFTWARE\Policies\Microsoft\Windows\System", "PublishUserActivities", 0, null),
            D("HKLM", @"SOFTWARE\Policies\Microsoft\Windows\System", "UploadUserActivities", 0, null)),
        T("recall", L.N("Privacy"), L.N("Recall off"),
            L.N("Windows does not save snapshots of the screen for Recall."),
            L.N("Snapshots take disk and processing time and contain everything that was on the screen."),
            L.N("The Recall search over past screens is not available."),
            D("HKLM", @"SOFTWARE\Policies\Microsoft\Windows\WindowsAI", "DisableAIDataAnalysis", 1, null)),

        T("suggestions", L.N("Advertising"), L.N("Suggestions and advertising off"),
            L.N("No suggested apps in Start, no tips and offers in Settings, on the lock screen and in notifications, no silently installed \"suggested\" apps."),
            L.N("These are advertising slots; some of them download and install apps on their own."),
            L.N("Nothing you would miss."),
            D("HKCU", Cdm, "SilentInstalledAppsEnabled", 0, 1),
            D("HKCU", Cdm, "SystemPaneSuggestionsEnabled", 0, 1),
            D("HKCU", Cdm, "SoftLandingEnabled", 0, 1),
            D("HKCU", Cdm, "RotatingLockScreenOverlayEnabled", 0, 1),
            D("HKCU", Cdm, "SubscribedContent-338388Enabled", 0, 1),
            D("HKCU", Cdm, "SubscribedContent-338389Enabled", 0, 1),
            D("HKCU", Cdm, "SubscribedContent-338393Enabled", 0, 1),
            D("HKCU", Cdm, "SubscribedContent-353694Enabled", 0, 1),
            D("HKCU", Cdm, "SubscribedContent-353696Enabled", 0, 1),
            D("HKCU", @"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "Start_IrisRecommendations", 0, 1)),
        T("bing-search", L.N("Advertising"), L.N("Start menu search without Bing"),
            L.N("Typing in the Start menu searches only this PC."),
            L.N("By default every keystroke in Start is sent to Bing and web results are mixed into the list."),
            L.N("No web results in the Start menu; the browser still searches the web."),
            D("HKCU", @"Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions", 1, null)),
        T("widgets", L.N("Advertising"), L.N("Widgets board off"),
            L.N("The news and weather board and its taskbar button are off."),
            L.N("The board keeps a web-view process running to load a news feed."),
            L.N("No weather on the taskbar."),
            D("HKLM", @"SOFTWARE\Policies\Microsoft\Dsh", "AllowNewsAndInterests", 0, null)),

        T("wu-feature", L.N("Windows Update"), L.N("Stay on this Windows release"),
            L.N("Windows Update offers only updates for the Windows 11 release this PC runs now; the next feature release is not installed by itself."),
            L.N("A feature update reinstalls Windows components and can undo settings and reinstall removed apps. You choose when to move on."),
            L.N("You get new Windows features only after switching this off. Security updates keep coming."),
            D("HKLM", Wu, "TargetReleaseVersion", 1, null),
            S("HKLM", Wu, "ProductVersion", "Windows 11", null),
            S("HKLM", Wu, "TargetReleaseVersionInfo", PinRelease(WinBuild(), WinDisplayVersion()), null)),
        T("wu-drivers", L.N("Windows Update"), L.N("No drivers from Windows Update"),
            L.N("Windows Update does not install or replace device drivers."),
            L.N("Windows Update can replace a graphics or chipset driver you chose with an older one from its catalogue."),
            L.N("You install drivers yourself (see the Drivers page). A new device may need its driver from the maker's site."),
            D("HKLM", Wu, "ExcludeWUDriversInQualityUpdate", 1, null),
            D("HKLM", @"SOFTWARE\Microsoft\Windows\CurrentVersion\DriverSearching", "SearchOrderConfig", 0, 1)),
        T("wu-reboot", L.N("Windows Update"), L.N("No automatic restart while signed in"),
            L.N("Windows does not restart by itself to finish an update while somebody is signed in, and does not wake the PC to install updates."),
            L.N("A forced restart can end a match or lose unsaved work."),
            L.N("Updates wait until you restart; do it yourself regularly."),
            D("HKLM", Wu + @"\AU", "NoAutoRebootWithLoggedOnUsers", 1, null),
            D("HKLM", Wu + @"\AU", "AUPowerManagement", 0, null)),

        T("file-ext", L.N("Interface"), L.N("Show file name extensions"),
            L.N("Explorer shows \".exe\", \".jpg\" and the other extensions."),
            L.N("You can tell a program from a document: \"photo.jpg.exe\" is no longer shown as \"photo.jpg\"."),
            L.N("Nothing."),
            D("HKCU", @"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "HideFileExt", 0, 1)),
    };

    // ---- the release "Stay on this Windows release" pins ---------------------------------------------------

    const string CurrentVersion = @"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    static int WinBuild()
    {
        try
        {
            using var k = Registry.LocalMachine.OpenSubKey(CurrentVersion);
            if (int.TryParse(k?.GetValue("CurrentBuildNumber") as string, NumberStyles.Integer, CultureInfo.InvariantCulture, out int b)) return b;
        }
        catch (Exception) { }
        return OperatingSystem.IsWindows() ? Environment.OSVersion.Version.Build : 0;
    }

    static string WinDisplayVersion()
    {
        try
        {
            using var k = Registry.LocalMachine.OpenSubKey(CurrentVersion);
            return k?.GetValue("DisplayVersion") as string;
        }
        catch (Exception) { return null; }
    }

    /// <summary>
    /// The release written to TargetReleaseVersionInfo. Builds 26200 (25H2) and 26300 (26H2) are held on 26H2, the
    /// same value the Domain OS playbook writes (25H2 → 26H2 is a small enablement update; 25H2 loses security updates
    /// a year earlier). Any other build keeps the release it runs now (DisplayVersion, for example "24H2").
    /// </summary>
    public static string PinRelease(int build, string displayVersion)
    {
        if (build is 26200 or 26300) return "26H2";
        string v = (displayVersion ?? "").Trim().ToUpperInvariant();
        return System.Text.RegularExpressions.Regex.IsMatch(v, @"^\d{2}H[12]$") ? v : "26H2";
    }

    // ---- retired: the "protection for speed" switches (removed in 1.12.0) -------------------------------------

    const string Dg = @"SYSTEM\CurrentControlSet\Control\DeviceGuard";
    const string Wd = @"SOFTWARE\Policies\Microsoft\Windows Defender";

    static RegChange B(string hive, string path, string name, string on, string def) => new(hive, path, name, "binary", on, def);

    /// <summary>
    /// The security switches of 1.6–1.11 (memory integrity / VBS / hypervisor, Spectre and Meltdown workarounds,
    /// exploit-protection defaults, Defender and SmartScreen). They are no longer offered anywhere. Only their values
    /// are kept, so that a PC where one of them was switched on gets its saved previous values back
    /// (<see cref="RevertRetired"/>, at every start and in the factory reset). Ids must stay as they were: the backup is keyed by them.
    /// </summary>
    internal static IReadOnlyList<Tweak> Retired { get; } = new List<Tweak>
    {
        new() { Id = "sec-vbs", Changes = new()
        {
            D("HKLM", Dg + @"\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled", 0, 1),
            D("HKLM", Dg, "EnableVirtualizationBasedSecurity", 0, null),
            D("HKLM", Dg + @"\Scenarios\KernelShadowStacks", "Enabled", 0, null),
            D("HKLM", Dg + @"\Scenarios\CredentialGuard", "Enabled", 0, null),
            new RegChange("BCD", "{current}", "hypervisorlaunchtype", "sz", "Off", null),
        } },
        new() { Id = "sec-cpu", Changes = new()
        {
            D("HKLM", @"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management", "FeatureSettingsOverride", 3, null),
            D("HKLM", @"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management", "FeatureSettingsOverrideMask", 3, null),
        } },
        new() { Id = "sec-exploit", Changes = new()
        {
            B("HKLM", @"SYSTEM\CurrentControlSet\Control\Session Manager\kernel", "MitigationOptions", new string('2', 48), null),
            B("HKLM", @"SYSTEM\CurrentControlSet\Control\Session Manager\kernel", "MitigationAuditOptions", new string('2', 48), null),
        } },
        new() { Id = "sec-defender", Changes = new()
        {
            D("HKLM", Wd, "DisableAntiSpyware", 1, null),
            D("HKLM", Wd + @"\Real-Time Protection", "DisableRealtimeMonitoring", 1, null),
            D("HKLM", Wd + @"\Real-Time Protection", "DisableBehaviorMonitoring", 1, null),
            D("HKLM", Wd + @"\Real-Time Protection", "DisableScanOnRealtimeEnable", 1, null),
            D("HKLM", Wd + @"\Real-Time Protection", "DisableOnAccessProtection", 1, null),
            D("HKLM", @"SOFTWARE\Policies\Microsoft\Windows\System", "EnableSmartScreen", 0, null),
            S("HKLM", @"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer", "SmartScreenEnabled", "Off", "Warn"),
        } },
    };

    /// <summary>
    /// Puts back the previous values of every retired security switch that this program switched on earlier (it has a
    /// backup). Switches without a backup were not changed by this program and are left alone. A failed revert keeps its
    /// backup and is tried again at the next start. Returns lines for the log.
    /// </summary>
    public static List<string> RevertRetired()
    {
        var log = new List<string>();
        Dictionary<string, Dictionary<string, string>> backup;
        try { backup = LoadBackup(); }
        catch (Exception) { return log; }
        foreach (var t in Retired.Where(t => backup.ContainsKey(t.Id)))
        {
            string err = Revert(t);
            log.Add(err == null
                ? L.F("A removed protection switch ({0}) was put back to your previous values; restart Windows", t.Id)
                : L.F("A removed protection switch ({0}) could not be put back yet: {1}", t.Id, err));
        }
        return log;
    }

    static IEnumerable<RegChange> Present(Tweak t) => t.Changes.Where(c => !c.IfKeyExists || Reg.HasKey(c.Hive, c.Path));

    public static TweakState State(Tweak t)
    {
        var list = Present(t).ToList();
        int on = list.Count(c => string.Equals(Reg.Get(c.Hive, c.Path, c.Name), c.On, StringComparison.OrdinalIgnoreCase));
        return list.Count > 0 && on == list.Count ? TweakState.On : on == 0 ? TweakState.Off : TweakState.Mixed;
    }

    // ---- backup: the values a change replaced, kept until the change is reverted ----------------------

    static Dictionary<string, Dictionary<string, string>> LoadBackup()
    {
        try
        {
            if (File.Exists(BackupPath))
                return JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, string>>>(File.ReadAllText(BackupPath)) ?? new();
        }
        catch (Exception) { }
        return new();
    }

    internal const string BackupName = "system-changes-backup.json";

    /// <summary>
    /// Whether a saved value may be written back for this change (pure): the change's own value name (the caller only
    /// looks up its own changes), in the change's type — a 32-bit number for a DWORD, hex bytes for a binary value, a
    /// short printable text for a string — and for the boot entry only "Auto". The retired security switches also
    /// refuse a value that would keep a protection off: their "Disable…" values and the "Enabled" switches accept only
    /// 0 / 1 (the value that turns them off is replaced by the Windows default anyway), SmartScreen only its own modes.
    /// "\0" (there was no value) is always fine.
    /// </summary>
    public static bool Acceptable(RegChange c, string saved, bool retired)
    {
        if (saved == null || saved == "\0") return true;
        if (c.Hive == "BCD") return saved.Equals("Auto", StringComparison.OrdinalIgnoreCase);
        bool typeOk = c.Kind switch
        {
            "dword" => int.TryParse(saved, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out _) || uint.TryParse(saved, NumberStyles.None, CultureInfo.InvariantCulture, out _),
            "binary" => saved.Length <= 256 && saved.Length % 2 == 0 && saved.All(Uri.IsHexDigit),
            _ => saved.Length <= 64 && !saved.Any(char.IsControl),
        };
        if (!typeOk) return false;
        if (c.Name == "TargetReleaseVersionInfo") return System.Text.RegularExpressions.Regex.IsMatch(saved, @"^\d{2}H[12]$");
        if (c.Name == "ProductVersion") return saved is "Windows 10" or "Windows 11";
        if (!retired) return true;
        if (c.Name == "SmartScreenEnabled") return saved is "Warn" or "RequireAdmin" or "Prompt" or "Off";
        if (c.Name.StartsWith("Disable", StringComparison.Ordinal) || c.Name is "Enabled" or "EnableVirtualizationBasedSecurity") return saved is "0" or "1";
        return true;
    }

    /// <summary>
    /// Takes over the backup of an older version (from the user's folder): only changes of the catalogue or the retired
    /// list, only their own values, only values <see cref="Acceptable"/> lets through. Null when nothing is left.
    /// </summary>
    internal static string ImportBackup(string json, string existing, List<string> dropped)
    {
        var src = JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, string>>>(json) ?? new();
        var res = existing == null ? new Dictionary<string, Dictionary<string, string>>()
            : JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, string>>>(ImportBackup(existing, null, null) ?? "{}") ?? new();
        var fresh = new Dictionary<string, Dictionary<string, string>>();
        foreach (var (id, vals) in src)
        {
            var t = Catalog.Concat(Retired).FirstOrDefault(x => x.Id == id);
            if (t == null || vals == null) { dropped?.Add(id); continue; }
            bool retired = Retired.Contains(t);
            var keep = new Dictionary<string, string>();
            foreach (var (key, v) in vals)
            {
                var c = t.Changes.FirstOrDefault(x => string.Equals(Id(x), key, StringComparison.OrdinalIgnoreCase));
                if (c != null && Acceptable(c, v, retired)) keep[Id(c)] = v;
                else dropped?.Add(id + ": " + key);
            }
            fresh[id] = keep;      // an entry without values still says "this program switched it on"
        }
        foreach (var (k, v) in fresh) res.TryAdd(k, v);      // what the state folder already has wins
        return res.Count == 0 ? null : JsonSerializer.Serialize(res, new JsonSerializerOptions { WriteIndented = true });
    }

    static void SaveBackup(Dictionary<string, Dictionary<string, string>> b)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(BackupPath)!);
        string tmp = BackupPath + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(b, new JsonSerializerOptions { WriteIndented = true }));
        File.Move(tmp, BackupPath, true);
    }

    static string Id(RegChange c) => c.Hive + "\\" + c.Path + "\\" + c.Name;

    public static bool HasBackup(Tweak t) => LoadBackup().ContainsKey(t.Id);

    /// <summary>Switches a change on: the previous values are saved first, each value is read back. Returns an error text or null.</summary>
    public static string Apply(Tweak t)
    {
        try
        {
            string pre = t.PreCheck?.Invoke();
            if (pre != null) return pre;
            var backup = LoadBackup();
            if (!backup.ContainsKey(t.Id))
            {
                // "\0" marks "there was no value" (an empty string is a real value).
                backup[t.Id] = Present(t).ToDictionary(Id, c => Reg.Get(c.Hive, c.Path, c.Name) ?? "\0");
                SaveBackup(backup);
            }
            // Every value is tried; the ones Windows refused are named at the end (the change then shows "Partly").
            var failed = new List<string>();
            foreach (var c in Present(t))
            {
                if (string.Equals(Reg.Get(c.Hive, c.Path, c.Name), c.On, StringComparison.OrdinalIgnoreCase)) continue;
                try { Reg.Set(c.Hive, c.Path, c.Name, c.Kind, c.On); }
                catch (Exception) when (t.Changes.Count > 1) { }
                if (!string.Equals(Reg.Get(c.Hive, c.Path, c.Name), c.On, StringComparison.OrdinalIgnoreCase))
                    failed.Add(c.IfKeyExists ? c.Path.Split('\\').Last() : c.Name);
            }
            return failed.Count == 0 ? null : L.F("Windows refused: {0}", string.Join(", ", failed));
        }
        catch (UnauthorizedAccessException) { return L.T("Access denied: run as administrator"); }
        catch (Exception e) { return e.Message; }
    }

    /// <summary>Switches a change off: the saved values come back; without a backup the Windows defaults are written.</summary>
    public static string Revert(Tweak t)
    {
        try
        {
            var backup = LoadBackup();
            backup.TryGetValue(t.Id, out var saved);
            bool retired = Retired.Contains(t);
            foreach (var c in Present(t))
            {
                string want = saved != null && saved.TryGetValue(Id(c), out var s) ? (s == "\0" ? null : s) : c.Default;
                // Only a value this change itself uses is written back; anything else falls back to the Windows default.
                if (saved != null && saved.TryGetValue(Id(c), out var raw) && !Acceptable(c, raw, retired))
                {
                    AppLog.Add(L.F("System changes: the saved value of {0} is not one this change uses; the Windows default is written instead", c.Name), AppLog.Sev.Warn);
                    want = c.Default;
                }
                // A backup taken while the change was already on would "restore" the change itself: the default is used then.
                if (want != null && string.Equals(want, c.On, StringComparison.OrdinalIgnoreCase) && c.Default != c.On) want = c.Default;
                string now = Reg.Get(c.Hive, c.Path, c.Name);
                if (want == null) { if (now != null) Reg.Delete(c.Hive, c.Path, c.Name); }
                else if (!string.Equals(now, want, StringComparison.OrdinalIgnoreCase)) Reg.Set(c.Hive, c.Path, c.Name, c.Kind, want);
            }
            if (backup.Remove(t.Id)) SaveBackup(backup);
            return null;
        }
        catch (UnauthorizedAccessException) { return L.T("Access denied: run as administrator"); }
        catch (Exception e) { return e.Message; }
    }
}
