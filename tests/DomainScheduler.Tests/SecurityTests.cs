using System.Text;
using System.Text.Json;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>
/// 1.15.0 Test 1.3 (audit 2026-11-18): what the program restores or acts on with administrator rights is only ever
/// what it could have written itself; planted entries, injection payloads and planted downloads are refused.
/// </summary>
[Collection("static-hooks")]
public class SecurityTests : IDisposable
{
    readonly string _state = Path.Combine(Path.GetTempPath(), "ds-state-" + Guid.NewGuid().ToString("N"));
    readonly string _legacy = Path.Combine(Path.GetTempPath(), "ds-legacy-" + Guid.NewGuid().ToString("N"));
    readonly MemoryRegStore _reg = new();

    const string Gpu = @"SYSTEM\CurrentControlSet\Enum\PCI\VEN_10DE&DEV_2D04&SUBSYS_00000000&REV_A1\4&2f9a8b1&0&0008";
    const string Ap = Gpu + @"\Device Parameters\Interrupt Management\Affinity Policy";
    const string Nic = @"SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\0001";

    public SecurityTests()
    {
        Directory.CreateDirectory(_state);
        Directory.CreateDirectory(_legacy);
        Store.DirHook = () => _state;
        Tweaks.Reg = _reg;
        Tweaks.BackupPathHook = () => Path.Combine(_state, Tweaks.BackupName);
        Settings.Backend = new MemorySettingsBackend();
        AppLog.Clear();
    }

    public void Dispose()
    {
        Store.DirHook = null;
        Tweaks.Reg = new WinRegStore();
        Tweaks.BackupPathHook = null;
        Settings.Backend = new RegistrySettingsBackend();
        AppLog.PathHook = null;
        foreach (var d in new[] { _state, _legacy }) if (Directory.Exists(d)) Directory.Delete(d, true);
    }

    static bool Logged(AppLog.Sev sev, string part) => AppLog.Tail(500).Any(e => e.Sev == sev && e.Msg.Contains(part));

    // ---- device backups ----------------------------------------------------------------------------------

    [Fact]
    public void DeviceLayoutBackupAcceptsOnlyWhatTheLayoutWrites()
    {
        Assert.True(IrqAuto.AllowedEntry("HKLM|" + Ap + "|DevicePolicy", ""));
        Assert.True(IrqAuto.AllowedEntry("HKLM|" + Ap + "|DevicePolicy", "d:4"));
        Assert.True(IrqAuto.AllowedEntry("HKLM|" + Ap + "|AssignmentSetOverride", "b:0400000000000000"));
        Assert.True(IrqAuto.AllowedEntry("HKLM|" + Nic + "|*RssBaseProcNumber", "s:2"));
        Assert.True(IrqAuto.AllowedEntry("HKLM|" + Nic + "|PnPCapabilities", "d:24"));
        Assert.True(IrqAuto.AllowedEntry(@"HKLM|SYSTEM\CurrentControlSet\Control\Session Manager\Kernel|ReservedCpuSets", "b:FEFF000000000000"));
        Assert.True(IrqAuto.AllowedEntry(@"HKCU|Control Panel\Mouse|RawMouseThrottleDuration", "d:20"));
        Assert.True(IrqAuto.AllowedEntry("HKLM|" + Gpu + @"\Device Parameters\Interrupt Management\Priority|*", "s:key"));

        // Planted: another key, another value name, a type or range the layout never writes, another hive.
        Assert.False(IrqAuto.AllowedEntry(@"HKLM|SYSTEM\CurrentControlSet\Services\evil|ImagePath", @"s:C:\Users\x\evil.exe"));
        Assert.False(IrqAuto.AllowedEntry("HKLM|" + Ap + "|ImagePath", "s:x"));
        Assert.False(IrqAuto.AllowedEntry("HKLM|" + Ap + "|DevicePolicy", "b:04"));
        Assert.False(IrqAuto.AllowedEntry("HKLM|" + Ap + "|DevicePolicy", "s:4"));
        Assert.False(IrqAuto.AllowedEntry("HKLM|" + Ap + "|DevicePolicy", "d:99999999999"));
        Assert.False(IrqAuto.AllowedEntry("HKU|" + Ap + "|DevicePolicy", "d:4"));
        Assert.False(IrqAuto.AllowedEntry(@"HKLM|SYSTEM\CurrentControlSet\Control\Class\..\..\Services\evil|PnPCapabilities", "d:1"));
        Assert.False(IrqAuto.AllowedEntry(@"HKCU|Software\Microsoft\Windows\CurrentVersion\Run|evil", "s:evil.exe"));
        Assert.False(IrqAuto.AllowedEntry(@"HKLM|SYSTEM\CurrentControlSet\Enum\PCI\A|B\Device Parameters\Interrupt Management\Affinity Policy|DevicePolicy", "d:4"));
        Assert.False(IrqAuto.AllowedEntry(@"HKLM|SYSTEM\CurrentControlSet\Services\evil\Device Parameters|SelectiveSuspendEnabled", "d:0"));
        Assert.False(IrqAuto.AllowedEntry("HKLM|" + Ap + "|AssignmentSetOverride", "b:" + new string('F', 200)));

        // The manual device backup: a PCI device and the five interrupt values only.
        string pdo = @"PCI\VEN_10DE&DEV_2D04&SUBSYS_00000000&REV_A1\4&2f9a8b1&0&0008";
        Assert.True(Devices.ValidBackupDevice(pdo, new() { ["DevicePolicy"] = "d:4", ["AssignmentSetOverride"] = "b:04", ["MSISupported"] = "" }));
        Assert.False(Devices.ValidBackupDevice(pdo, new() { ["ImagePath"] = "s:x" }));
        Assert.False(Devices.ValidBackupDevice(pdo, new() { ["DevicePriority"] = "s:7" }));
        Assert.False(Devices.ValidBackupDevice(@"ROOT\LEGACY_EVIL\0000", new() { ["DevicePolicy"] = "d:4" }));
    }

    [Fact]
    public void PlantedDeviceBackupEntriesAreDroppedWhenTakenOver()
    {
        var planted = new Dictionary<string, object>
        {
            ["Reg"] = new Dictionary<string, string>
            {
                ["HKLM|" + Ap + "|DevicePolicy"] = "",
                [@"HKLM|SYSTEM\CurrentControlSet\Services\evil|ImagePath"] = @"s:C:\evil.exe",
                ["HKLM|" + Ap + "|DevicePriority"] = "s:250",
            },
            ["Wmi"] = new Dictionary<string, bool> { [@"PCI\VEN_8086&DEV_7AE0\3&11583659&0&A0_0"] = true, ["evil|x"] = false },
            ["UsbSuspend"] = new uint[] { 1, 7 },
        };
        var dropped = new List<string>();
        string json = IrqAuto.ImportBackup(JsonSerializer.Serialize(planted), null, dropped);
        Assert.NotNull(json);
        Assert.Contains("DevicePolicy", json);
        Assert.DoesNotContain("evil", json);
        Assert.DoesNotContain("DevicePriority", json);
        Assert.DoesNotContain("UsbSuspend\":[", json);
        Assert.Equal(4, dropped.Count);       // ImagePath, the priority as text, the WMI name with "|", the suspend pair

        // A backup already in the state folder wins over the old one.
        string existing = IrqAuto.ImportBackup(JsonSerializer.Serialize(new { Reg = new Dictionary<string, string> { ["HKLM|" + Ap + "|DevicePolicy"] = "d:0" } }), null, null);
        string merged = IrqAuto.ImportBackup(JsonSerializer.Serialize(planted), existing, new());
        Assert.Contains("\"d:0\"", merged);
    }

    // ---- system changes ----------------------------------------------------------------------------------

    [Fact]
    public void PlantedSystemChangeBackupIsNotWrittenBack()
    {
        var vbs = Tweaks.Retired.First(t => t.Id == "sec-vbs");
        foreach (var c in vbs.Changes) _reg.Set(c.Hive, c.Path, c.Name, c.Kind, c.On);
        var planted = new Dictionary<string, Dictionary<string, string>>
        {
            ["sec-vbs"] = new()
            {
                [@"BCD\{current}\hypervisorlaunchtype"] = "Off /set {bootmgr} path \\evil.efi",
                [@"HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity\Enabled"] = "not a number",
            },
            ["not-a-tweak"] = new() { [@"HKLM\SYSTEM\CurrentControlSet\Services\evil\ImagePath"] = "evil.exe" },
        };
        File.WriteAllText(Tweaks.BackupPath, JsonSerializer.Serialize(planted));
        Tweaks.RevertRetired();
        Assert.Null(_reg.Get("BCD", "{current}", "hypervisorlaunchtype"));                 // the Windows default, not the planted text
        Assert.Equal("1", _reg.Get("HKLM", @"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled"));
        Assert.Null(_reg.Get("HKLM", @"SYSTEM\CurrentControlSet\Services\evil", "ImagePath"));
        Assert.True(Logged(AppLog.Sev.Warn, "hypervisorlaunchtype"));

        // The take-over drops unknown changes and values the change does not use.
        var dropped = new List<string>();
        string json = Tweaks.ImportBackup(JsonSerializer.Serialize(planted), null, dropped);
        Assert.DoesNotContain("evil", json);
        Assert.Equal(3, dropped.Count);

        // The boot entry never reaches bcdedit as a command line put together from text.
        Assert.Throws<InvalidOperationException>(() => new WinRegStore().Set("BCD", "{current} /set {bootmgr} path x", "hypervisorlaunchtype", "sz", "Auto"));
        Assert.Throws<InvalidOperationException>(() => new WinRegStore().Set("BCD", "{current}", "hypervisorlaunchtype", "sz", "Auto & calc"));
    }

    [Fact]
    public void ReservationAndPowerRestoreOnlyTheirOwnValues()
    {
        Assert.True(Reservation.ValidOriginal("absent"));
        Assert.True(Reservation.ValidOriginal("0102000000000000"));
        Assert.False(Reservation.ValidOriginal("zz"));
        Assert.False(Reservation.ValidOriginal(new string('0', 200)));
        Assert.Null(Reservation.ImportBackup("evil|2026-01-01T00:00:00Z", new()));

        Assert.True(GamePower.ValidValue("5d76a2ca-e8c0-402f-a133-2158492d58ad", 1));
        Assert.False(GamePower.ValidValue("5d76a2ca-e8c0-402f-a133-2158492d58ad", 2));
        Assert.True(GamePower.ValidValue("0cc5b647-c1df-4637-891a-dec35c318583", 100));
        Assert.False(GamePower.ValidValue("3b04d4fd-1cc7-4f23-ab1c-d1337819c4bb", 0));     // a setting it never managed
        var dropped = new List<string>();
        Assert.Null(GamePower.ImportFile("{\"Scheme\":\"381b4222-f694-41f0-9685-ff5bb260df2e\",\"Values\":{\"3b04d4fd-1cc7-4f23-ab1c-d1337819c4bb\":0}}", dropped));
        Assert.Single(dropped);
    }

    // ---- crash session -----------------------------------------------------------------------------------

    [Fact]
    public void CrashRestoreChecksValuesNamesAndTheGuard()
    {
        var f = FakeNative.Cs2();
        f.Procs[801] = new FakeNative.P { Name = "notepad.exe", Created = 6 };
        f.Procs[802] = new FakeNative.P { Name = "lsass.exe", Created = 7 };
        f.Procs[804] = new FakeNative.P { Name = "game.exe", Created = 9 };
        var e = new Engine(f) { GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false) ?? (AutoController.IsSystem(exe) ? "system" : null) };
        var s = new SessionData
        {
            Procs =
            {
                new SessionProc { Pid = 801, Name = "notepad.exe", Created = 6, Prio = 256, PrioSet = true },   // realtime: never written
                new SessionProc { Pid = 802, Name = "lsass.exe", Created = 7, Prio = 16384, PrioSet = true },   // the guard refuses it
                new SessionProc { Pid = 804, Name = "other.exe", Created = 9, Prio = 16384, PrioSet = true },   // the PID is another program now
                new SessionProc { Pid = 500, Name = "cs2.exe", Created = f.Procs[500].Created, Prio = 16384, PrioSet = true },
            },
            Threads = { new SessionThread { Pid = 500, Tid = 100, Born = f.Threads[100].Born, Prio = 15, PrioSet = true } },   // time-critical: refused
        };
        e.RestoreSession(s);
        Assert.Equal(32u, f.Procs[801].Cls);
        Assert.Equal(32u, f.Procs[802].Cls);
        Assert.Equal(32u, f.Procs[804].Cls);
        Assert.Equal(16384u, f.Procs[500].Cls);
        Assert.NotEqual(15, f.Threads[100].Prio);
        Assert.False(Engine.SessionThreadValid(new SessionThread { Ideal = 4000, IdealSet = true }));
    }

    // ---- Defender clean-up -------------------------------------------------------------------------------

    [Fact]
    public void DefenderListRefusesInjectionAndNeverBuildsACommandFromIt()
    {
        Assert.True(DefenderGameExclusion.IsSafePath(@"C:\Games\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"));
        Assert.True(DefenderGameExclusion.IsSafePath(@"D:\SteamLibrary\steamapps\common\Игра (2024)\game.exe"));
        string[] payloads =
        {
            "C:\\Games\\a\u2018; Start-Process calc; \u2018.exe",       // typographic single quotes: quotes for PowerShell
            "C:\\Games\\a\u2019+$(calc)+\u2019.exe",
            "C:\\Games\\a\u201A;calc;\u201B.exe",
            "C:\\Games\\a\u201C;calc;\u201D.exe",
            "C:\\Games\\a';calc;'.exe",
            "C:\\Games\\$(calc).exe",
            "C:\\Games\\a`;calc.exe",
            "C:\\Games\\a|calc.exe",
            "C:\\Games\\a&calc.exe",
            "C:\\Games\\a>x.exe",
            "C:\\Games\\a\ncalc.exe",
            "C:\\Games\\..\\Windows\\a.exe",
            "\\\\server\\share\\a.exe",
            "C:\\Games\\game.bat",
        };
        foreach (var p in payloads) Assert.False(DefenderGameExclusion.IsSafePath(p), p);

        // The path only travels in an environment variable; the command is fixed.
        string payload = payloads[0];
        var psi = SysExe.PowerShell(DefenderGameExclusion.Command, new Dictionary<string, string> { ["DOMAINOS_EXCLUSION"] = payload });
        Assert.DoesNotContain(psi.ArgumentList, a => a.Contains("calc"));
        Assert.Equal(payload, psi.Environment["DOMAINOS_EXCLUSION"]);
        Assert.Contains("-NoProfile", psi.ArgumentList);
        Assert.Contains("-NonInteractive", psi.ArgumentList);
        Assert.Equal(SysExe.ModulesPath, psi.Environment["PSModulePath"]);
        Assert.EndsWith(@"WindowsPowerShell\v1.0\powershell.exe", psi.FileName);

        // A planted list with only the payload: nothing is started, the file goes, the log says so.
        File.WriteAllText(Path.Combine(_state, DefenderGameExclusion.FileName), payload + "\n");
        Assert.Equal("", DefenderGameExclusion.RemoveAll());
        Assert.False(File.Exists(Path.Combine(_state, DefenderGameExclusion.FileName)));
        Assert.True(Logged(AppLog.Sev.Warn, "Defender"));
    }

    [Fact]
    public void ChildProcessesGetNoProfilerOrUserModules()
    {
        Environment.SetEnvironmentVariable("COR_ENABLE_PROFILING", "1");
        Environment.SetEnvironmentVariable("DOTNET_STARTUP_HOOKS", "evil.dll");
        try
        {
            var psi = SysExe.Start("schtasks.exe");
            Assert.False(psi.Environment.ContainsKey("COR_ENABLE_PROFILING"));
            Assert.False(psi.Environment.ContainsKey("DOTNET_STARTUP_HOOKS"));
            Assert.Equal(SysExe.ModulesPath, psi.Environment["PSModulePath"]);
            Assert.Equal(SysExe.System32("schtasks.exe"), psi.FileName);          // System32's own, by its full path on Windows
        }
        finally
        {
            Environment.SetEnvironmentVariable("COR_ENABLE_PROFILING", null);
            Environment.SetEnvironmentVariable("DOTNET_STARTUP_HOOKS", null);
        }
    }

    // ---- updates ------------------------------------------------------------------------------------------

    [Fact]
    public void UpdaterRefusesBadVersionsWrongBytesAndPlantedFiles()
    {
        Assert.Matches(Updater.VersionRx, "1.15.0");
        Assert.Matches(Updater.VersionRx, "1.15.0-test1.3");
        foreach (var v in new[] { "1.15.0\\..\\x", "1.15.0 ", "v1.15.0", "1.15.0-test1.3/../a", "1.15", "1.15.0-beta" })
            Assert.DoesNotMatch(Updater.VersionRx, v);
        string feed = "{\"toolbox\":{\"version\":\"1.16.0\\\\..\\\\..\\\\evil\",\"url\":\"https://x/y\",\"sha256\":\"" + new string('a', 64) + "\",\"size\":5}}";
        Assert.Null(Updater.ParseFeed(feed).Toolbox);

        byte[] data = Encoding.UTF8.GetBytes("hello");
        var f = new UpdateFile("1.16.0", "https://x/y", Updater.Sha256(data), data.Length, "", "");
        Assert.Equal(data, Updater.Check(data, f));
        Assert.Throws<InvalidDataException>(() => Updater.Check(Encoding.UTF8.GetBytes("hellO"), f));
        Assert.ThrowsAsync<InvalidDataException>(() => Updater.ReadAll(new MemoryStream(Encoding.UTF8.GetBytes("hello!")), new byte[5], null, default)).Wait();
        Assert.ThrowsAsync<InvalidDataException>(() => Updater.ReadAll(new MemoryStream(Encoding.UTF8.GetBytes("hell")), new byte[5], null, default)).Wait();

        // Downloads: a file planted under the expected name is never reused; each write gets a new random folder.
        string dl = Path.Combine(_state, "Updates");
        Directory.CreateDirectory(Path.Combine(dl, "aaaa"));
        File.WriteAllText(Path.Combine(dl, "Domain-OS-2.1.2.apbx"), "planted");
        File.WriteAllText(Path.Combine(dl, "aaaa", "Domain-OS-2.1.2.apbx"), "planted");
        string a = Updater.WriteNew(data, "Domain-OS-2.1.2.apbx", dl);
        string b = Updater.WriteNew(data, "Domain-OS-2.1.2.apbx", dl);
        Assert.NotEqual(a, b);
        Assert.Equal("hello", File.ReadAllText(b));
        Assert.DoesNotContain(Directory.GetFiles(dl, "*", SearchOption.AllDirectories), x => File.ReadAllText(x) == "planted");
        Assert.Throws<InvalidDataException>(() => Updater.WriteNew(data, @"..\..\DomainScheduler.exe", dl));
        Assert.Throws<InvalidDataException>(() => Updater.WriteNew(data, "a/b.apbx", dl));
    }

    [Fact]
    public void RollbackSwapsThePreviousFileDirectly()
    {
        string exe = Path.Combine(_state, "DomainScheduler.exe");
        File.WriteAllText(exe, "new");
        File.WriteAllText(exe + ".prev", "old");
        Updater.SwapToPrevious(exe);
        Assert.Equal("old", File.ReadAllText(exe));
        Assert.Equal("new", File.ReadAllText(exe + ".next"));
        Updater.UndoSwapToPrevious(exe);          // the previous one did not start: the newer one comes back
        Assert.Equal("new", File.ReadAllText(exe));
        Assert.Equal("old", File.ReadAllText(exe + ".prev"));
        File.Delete(exe + ".prev");
        Assert.Throws<FileNotFoundException>(() => Updater.SwapBack(exe));
        Assert.Equal("new", File.ReadAllText(exe));                // never left without a program file
    }

    // ---- taking over an older version's data ---------------------------------------------------------------

    [Fact]
    public void MigrationTakesOverValidEntriesDropsPlantedOnesAndRenamesTheOldFiles()
    {
        File.WriteAllText(Path.Combine(_legacy, IrqAuto.BackupName), JsonSerializer.Serialize(new
        {
            Reg = new Dictionary<string, string>
            {
                ["HKLM|" + Ap + "|DevicePolicy"] = "d:0",
                [@"HKLM|SYSTEM\CurrentControlSet\Services\evil|ImagePath"] = @"s:C:\evil.exe",
            },
        }));
        File.WriteAllText(Path.Combine(_legacy, DefenderGameExclusion.FileName),
            "C:\\Games\\cs2.exe\nC:\\Games\\a\u2018; Start-Process calc; \u2018.exe\n");
        File.WriteAllText(Path.Combine(_legacy, GpuProfiles.FileName),
            "{\"Profiles\":[{\"Name\":\"Max\",\"CoreMHz\":300,\"MemMHz\":1500,\"VoltPct\":100}],\"AtStart\":\"Max\"}");
        File.WriteAllText(Path.Combine(_legacy, "session.json"), JsonSerializer.Serialize(new SessionData
        {
            CleanShutdown = false,
            Procs = { new SessionProc { Pid = 9, Name = "a.exe", Prio = 256, PrioSet = true }, new SessionProc { Pid = 10, Name = "b.exe", Prio = 16384, PrioSet = true } },
        }));
        File.WriteAllText(Path.Combine(_legacy, Tweaks.BackupName), "{ not json");

        var log = LegacyImport.Run(_legacy);

        string irq = File.ReadAllText(Path.Combine(_state, IrqAuto.BackupName));
        Assert.Contains("DevicePolicy", irq);
        Assert.DoesNotContain("evil", irq);
        string excl = File.ReadAllText(Path.Combine(_state, DefenderGameExclusion.FileName));
        Assert.Contains("cs2.exe", excl);
        Assert.DoesNotContain("calc", excl);
        var gpu = GpuProfiles.Load(Path.Combine(_state, GpuProfiles.FileName));
        Assert.Single(gpu.Profiles);
        Assert.Equal("", gpu.AtStart);                                   // never applied from the old file as it is
        Assert.Equal("Max", gpu.Confirm);                                // asked once in the program's window
        Assert.Equal("Max", gpu.ConfirmProfile?.Name);
        var session = JsonSerializer.Deserialize<SessionData>(File.ReadAllText(Path.Combine(_state, "session.json")));
        Assert.Single(session.Procs);
        Assert.Equal(10u, session.Procs[0].Pid);
        Assert.False(File.Exists(Path.Combine(_state, Tweaks.BackupName)));   // broken: not taken over

        foreach (var n in new[] { IrqAuto.BackupName, DefenderGameExclusion.FileName, GpuProfiles.FileName, "session.json", Tweaks.BackupName })
        {
            Assert.False(File.Exists(Path.Combine(_legacy, n)), n);
            Assert.True(File.Exists(Path.Combine(_legacy, n + ".migrated")), n);
        }
        Assert.Contains(log, l => l.Sev == AppLog.Sev.Warn && l.Text.Contains("ImagePath"));
        Assert.Contains(log, l => l.Sev == AppLog.Sev.Warn && l.Text.Contains(Tweaks.BackupName));
        Assert.Empty(LegacyImport.Run(_legacy));                         // read once
    }

    [Fact]
    public void MachineSettingsTakeOverOnlyKnownValidValues()
    {
        Assert.True(Settings.IsMachine(Flags.Sub, "gpu-oc"));
        Assert.True(Settings.IsMachine("", "SafeDeviceLayout112"));
        Assert.True(Settings.IsMachine("", "GpuOcAtStart"));
        Assert.False(Settings.IsMachine("", "Language"));
        Assert.False(Settings.IsMachine("", "WatchList"));

        var user = new Dictionary<string, Dictionary<string, string>>
        {
            [""] = new(StringComparer.OrdinalIgnoreCase)
            {
                ["Language"] = "ru", ["SafeDeviceLayout112"] = "true", ["LegacyRawMouseChecked"] = "false",
                ["GpuOcAtStart"] = "true", ["GpuOcCoreKHz"] = "300000", ["GpuOcVoltPct"] = "500",
            },
            [Flags.Sub] = new(StringComparer.OrdinalIgnoreCase) { ["devices-write"] = "true", ["gpu-oc"] = "maybe", ["evil"] = "true", ["etwirq"] = "false" },
        };
        var log = new List<string>();
        var plan = Settings.MachineImport(sub => user[sub], (sub, name) => name == "etwirq", log);
        var got = plan.Select(p => (p.Sub, p.Name, p.Value)).ToList();
        Assert.Contains(("", "SafeDeviceLayout112", "true"), got);
        Assert.Contains(("", "GpuOcCoreKHz", "300000"), got);
        Assert.Contains((Flags.Sub, "devices-write", "true"), got);
        Assert.DoesNotContain(got, p => p.Name is "Language" or "LegacyRawMouseChecked" or "GpuOcAtStart" or "GpuOcVoltPct" or "gpu-oc" or "evil" or "etwirq");
        Assert.True(log.Count >= 4);
    }

    // ---- the state folder's permissions ---------------------------------------------------------------------

    [Fact]
    public void StateFolderPermissionsAreJudgedStrictly()
    {
        const string Sys = "S-1-5-18", Adm = "S-1-5-32-544", Users = "S-1-5-32-545", Me = "S-1-5-21-1-2-3-1001", Creator = "S-1-3-0";
        var ours = StateDir.Canonical().ToList();
        Assert.Equal(StateDir.Trust.Trusted, StateDir.Judge(Adm, true, true, ours));
        Assert.Equal(StateDir.Trust.Trusted, StateDir.Judge(Sys, true, true, ours));
        Assert.Equal(StateDir.Trust.Untrusted, StateDir.Judge(Me, true, true, ours));                       // another owner
        Assert.Equal(StateDir.Trust.Untrusted, StateDir.Judge(Adm, false, true, ours));                     // no list = everybody
        Assert.Equal(StateDir.Trust.Untrusted, StateDir.Judge(Adm, true, true, ours.Append(new(Users, 0x2 | 0x4, true, 3, true)).ToList()));   // ProgramData's inherited "create files"
        Assert.Equal(StateDir.Trust.Untrusted, StateDir.Judge(Adm, true, true, ours.Append(new(Creator, 0x10000000, true, 0xB, false)).ToList()));
        Assert.Equal(StateDir.Trust.Untrusted, StateDir.Judge(Adm, true, true, ours.Append(new(Me, 0x40000, true, 0, false)).ToList()));      // may change the permissions
        Assert.Equal(StateDir.Trust.ResetPermissions, StateDir.Judge(Adm, true, true, ours.Append(new(Me, 0x1200A9, true, 3, false)).ToList()));   // Explorer's read entry
        Assert.Equal(StateDir.Trust.ResetPermissions, StateDir.Judge(Adm, true, false, ours));             // not protected from inheritance
        Assert.Equal(StateDir.Trust.Trusted, StateDir.Judge(Adm, true, true, ours.Append(new(Me, 0x40000, false, 0, false)).ToList().Where(a => a.Allow).ToList()));
    }

    // ---- the native libraries' folder -----------------------------------------------------------------------

    [Fact]
    public void RelaunchOnlyWhenTheHostUnpacksElsewhere()
    {
        string want = @"C:\ProgramData\Domain OS Toolbox\native";
        Assert.True(NativeHome.ShouldRelaunch(true, true, null, want, null));
        Assert.True(NativeHome.ShouldRelaunch(true, true, @"C:\Users\x\AppData\Local\Temp\evil", want, null));
        Assert.False(NativeHome.ShouldRelaunch(true, true, want + "\\", want, null));
        Assert.False(NativeHome.ShouldRelaunch(true, true, null, want, "1234"));      // already the second copy: never a loop
        Assert.False(NativeHome.ShouldRelaunch(true, false, null, want, null));       // not a single-file build
        Assert.False(NativeHome.ShouldRelaunch(false, true, null, want, null));
        Assert.True(NativeHome.WaitsForChild(new[] { "--revert-devices" }));
        Assert.True(NativeHome.WaitsForChild(new[] { "--after-update" }));
        Assert.False(NativeHome.WaitsForChild(new[] { "--autostart" }));
    }

    // ---- the log ------------------------------------------------------------------------------------------

    [Fact]
    public void WarningsReachTheFileAtOnceAndNothingIsWrittenTwice()
    {
        string file = Path.Combine(_state, "log.txt");
        AppLog.PathHook = () => file;
        AppLog.Flush();
        AppLog.Add("first info");
        AppLog.Add("a warning", AppLog.Sev.Warn);
        string text = File.ReadAllText(file);
        Assert.Contains("a warning", text);
        Assert.Contains("first info", text);
        AppLog.Flush();
        AppLog.Flush();
        AppLog.Add("an error", AppLog.Sev.Error);
        var lines = File.ReadAllLines(file);
        Assert.Single(lines, l => l.Contains("a warning"));
        Assert.Single(lines, l => l.Contains("first info"));
        Assert.Single(lines, l => l.Contains("an error"));
    }
}

/// <summary>F6: a revert that could not put everything back is never recorded as a clean shutdown.</summary>
[Collection("static-hooks")]
public class CleanShutdownTests : IDisposable
{
    readonly FakeNative _f = FakeNative.Cs2();
    readonly FakeProbe _p = new();
    readonly List<SessionData> _saved = new();
    DateTime _now = new(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);

    public CleanShutdownTests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        GameMask.IrqHook = () => new();
        Reservation.Reg = new FakeReserved();
        Settings.Backend = new MemorySettingsBackend();
        AppLog.Clear();
    }

    public void Dispose()
    {
        Topo.LiveHook = null;
        GameMask.IrqHook = null;
        Reservation.Reg = new WinReservedRegistry();
        Settings.Backend = new RegistrySettingsBackend();
    }

    [Fact]
    public void AFailedRevertKeepsItsBaselineAndTheSessionIsNotClean()
    {
        _f.Procs[801] = new FakeNative.P { Name = "notepad.exe", Created = 6 };
        _f.Procs[805] = new FakeNative.P { Name = "steamless.exe", Created = 8 };
        var e = new Engine(_f) { Now = () => _now };
        var c = new AutoController(e, _p, new NoFrames(), () => _now) { SaveSession = s => _saved.Add(s), AutostartLine = () => "" };
        e.ApplyBackground(new uint[] { 801, 805 }, 0, false);
        Assert.Equal(16384u, _f.Procs[801].Cls);
        _f.Procs[801].ReadOnly = true;                     // Windows refuses to put it back now

        c.RevertEverything("test");
        Assert.False(_saved[^1].CleanShutdown);
        Assert.Contains(_saved[^1].Procs, p => p.Pid == 801);
        Assert.DoesNotContain(_saved[^1].Procs, p => p.Pid == 805);   // the other process was put back all the same
        Assert.Equal(32u, _f.Procs[805].Cls);
        Assert.True(e.Pending);

        _f.Procs[801].ReadOnly = false;                    // the next revert gets it
        c.RevertEverything("test");
        Assert.Equal(32u, _f.Procs[801].Cls);
        Assert.True(_saved[^1].CleanShutdown);
        Assert.False(e.Pending);
    }

    [Fact]
    public void TheStandaloneBackgroundPassReadsTheProcessListOnce()
    {
        for (uint pid = 900; pid < 960; pid++) _f.Procs[pid] = new FakeNative.P { Name = "bg" + pid + ".exe", Created = pid };
        var e = new Engine(_f) { Now = () => _now, GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false) };
        e.GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false, e.ProcessesNow().Values.Select(v => v.Name));
        var c = new AutoController(e, _p, new NoFrames(), () => _now) { SaveSession = s => _saved.Add(s), AutostartLine = () => "", WatchList = () => new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "cs2.exe" } };
        _f.ProcessLists = 0;
        c.StandaloneBackground(true, 0);
        Assert.True(e.BackgroundCount >= 60);
        Assert.True(_f.ProcessLists <= 2, $"{_f.ProcessLists} process lists for one pass");
        c.StandaloneBackground(false, 0);
    }

    [Fact]
    public void TheWarmupPauseDoesNotHoldTheLock()
    {
        var probes = new List<bool>();
        AutoController c = null;
        var e = new Engine(_f) { Now = () => _now };
        e.Sleep = ms =>
        {
            // The window reads the state under the controller's lock: it must get it during the pause.
            if (c != null) probes.Add(Task.Run(() => c.ButtonEnabled).Wait(2000));
            _now = _now.AddMilliseconds(ms);
            _f.Run(new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [103] = 900_000_000 }, ms / 1000.0);
        };
        c = new AutoController(e, _p, new NoFrames(), () => _now) { SaveSession = s => _saved.Add(s), AutostartLine = () => "", WatchList = () => new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "cs2.exe" } };
        _p.Windows.Add(500);
        _p.Graphics.Add(500);
        _p.Cpu[500] = (_f.Procs[500].Created, 0);
        _p.Fg = 500;
        c.Click();
        for (int i = 0; i < 12; i++)
        {
            _now = _now.AddSeconds(1);
            _p.Cpu[500] = (_f.Procs[500].Created, _p.Cpu[500].cpu + 8_000_000);
            c.Tick();
        }
        Assert.Equal(AutoState.Active, c.State);
        Assert.NotEmpty(probes);
        Assert.All(probes, Assert.True);
    }

    [Fact]
    public void OldStartMark_WaitsForAnswer_AndAnyChoiceAnswersIt()
    {
        var g = new GpuProfiles();
        g.Put("Max", 300, 1500, 50);
        g.Put("Daily", 100, 500, 0);
        g.Confirm = "max";
        Assert.Equal("Max", g.ConfirmProfile?.Name);
        var (what, _) = GpuOcLogic.DecideStart(true, g, pending: false);
        Assert.Equal(GpuOcLogic.Start.Nothing, what);                    // not applied before the yes
        Assert.True(g.Rename("Max", "Max 2"));
        Assert.Equal("Max 2", g.Confirm);                                // follows a rename
        Assert.True(g.SetAtStart("Max 2"));                              // yes
        Assert.Equal("", g.Confirm);
        Assert.Equal(GpuOcLogic.Start.Apply, GpuOcLogic.DecideStart(true, g, pending: false).What);
        Assert.Equal(GpuOcLogic.Start.SkipCrash, GpuOcLogic.DecideStart(true, g, pending: true).What);   // crash guard still holds

        g.SetAtStart(null); g.Confirm = "Daily";
        Assert.True(g.SetAtStart(null));                                 // no
        Assert.Null(g.ConfirmProfile);
        g.Confirm = "Daily"; Assert.True(g.Delete("Daily"));
        Assert.Equal("", g.Confirm);                                     // goes with the profile

        string f = Path.Combine(Path.GetTempPath(), "gp-" + Guid.NewGuid().ToString("N") + ".json");
        try
        {
            File.WriteAllText(f, "{\"Profiles\":[{\"Name\":\"A\"}],\"Confirm\":\"ghost\"}");
            Assert.Equal("", GpuProfiles.Load(f).Confirm);               // points at no profile: dropped
            File.WriteAllText(f, "{\"Profiles\":[{\"Name\":\"A\"},{\"Name\":\"B\"}],\"AtStart\":\"A\",\"Confirm\":\"B\"}");
            var l = GpuProfiles.Load(f);
            Assert.Equal("A", l.AtStart);
            Assert.Null(l.ConfirmProfile);                               // a mark already chosen wins: nothing to ask
        }
        finally { File.Delete(f); }
    }
}
