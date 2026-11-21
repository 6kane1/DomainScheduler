using Xunit;

namespace DomainScheduler.Tests;

/// <summary>The documented system changes (apply, backup, revert) and the byte work of the GPU clock and voltage interface.</summary>
[Collection("static-hooks")]
public class SystemToolsTests : IDisposable
{
    readonly MemoryRegStore _reg = new();
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-tweaks-" + Guid.NewGuid().ToString("N"));

    public SystemToolsTests()
    {
        Tweaks.Reg = _reg;
        Tweaks.BackupPathHook = () => Path.Combine(_dir, "backup.json");
        Settings.Backend = new MemorySettingsBackend();
        AppLog.Clear();
    }

    public void Dispose()
    {
        Tweaks.Reg = new WinRegStore();
        Tweaks.BackupPathHook = null;
        Settings.Backend = new RegistrySettingsBackend();
        NvOc.GetHook = null;
        NvOc.SetHook = null;
        NvOc.VoltGetHook = null;
        NvOc.VoltSetHook = null;
        if (Directory.Exists(_dir)) Directory.Delete(_dir, true);
    }

    [Fact]
    public void EveryChangeIsExplainedAndTouchesNoSecuritySetting()
    {
        Assert.True(Tweaks.Catalog.Count >= 12);
        Assert.Equal(Tweaks.Catalog.Count, Tweaks.Catalog.Select(t => t.Id).Distinct().Count());
        string[] forbidden = { "Defender", "SmartScreen", "DeviceGuard", "HypervisorEnforcedCodeIntegrity", "Mitigation", "SecureBoot", "EnableLUA", "Lsa", "Spectre", "FeatureSettingsOverride", "DisableAntiSpyware" };
        foreach (var t in Tweaks.Catalog)
        {
            Assert.True(t.What.Length > 20 && t.Why.Length > 20 && t.Cost.Length > 5, t.Id);
            Assert.NotEmpty(t.Changes);
            foreach (var c in t.Changes)
            {
                Assert.Contains(c.Hive, new[] { "HKLM", "HKCU" });
                Assert.DoesNotContain(forbidden, f => (c.Path + "\\" + c.Name).Contains(f, StringComparison.OrdinalIgnoreCase));
                Assert.NotEqual(c.On, c.Default);
            }
        }
    }

    [Fact]
    public void SecuritySwitchesAreNoLongerOffered()
    {
        Assert.Null(typeof(Tweaks).GetProperty("Security"));
        Assert.DoesNotContain(Tweaks.Catalog, t => t.Id.StartsWith("sec-", StringComparison.Ordinal));
        Assert.Equal(new[] { "sec-vbs", "sec-cpu", "sec-exploit", "sec-defender" }, Tweaks.Retired.Select(t => t.Id));
    }

    [Fact]
    public void RetiredSecuritySwitchesAreRevertedFromTheirBackupOnly()
    {
        var cpu = Tweaks.Retired.First(t => t.Id == "sec-cpu");
        var vbs = Tweaks.Retired.First(t => t.Id == "sec-vbs");
        var def = Tweaks.Retired.First(t => t.Id == "sec-defender");
        const string Mm = @"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management";
        // An older version switched these on: the previous values went to the backup first.
        _reg.Set("HKLM", Mm, "FeatureSettingsOverride", "dword", "72");                 // the user's own value before
        Assert.Null(Tweaks.Apply(cpu));
        Assert.Null(Tweaks.Apply(vbs));
        Assert.Equal("3", _reg.Get("HKLM", Mm, "FeatureSettingsOverride"));
        Assert.Equal("Off", _reg.Get("BCD", "{current}", "hypervisorlaunchtype"));
        // Defender policies set by something else (no backup of this program): left alone.
        foreach (var c in def.Changes) _reg.Set(c.Hive, c.Path, c.Name, c.Kind, c.On);

        var log = Tweaks.RevertRetired();
        Assert.Equal(2, log.Count);
        Assert.Equal("72", _reg.Get("HKLM", Mm, "FeatureSettingsOverride"));            // exactly the previous value
        Assert.Null(_reg.Get("HKLM", Mm, "FeatureSettingsOverrideMask"));               // there was none: there is none again
        Assert.Null(_reg.Get("BCD", "{current}", "hypervisorlaunchtype"));
        Assert.Null(_reg.Get("HKLM", @"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled"));
        Assert.False(Tweaks.HasBackup(cpu));
        Assert.False(Tweaks.HasBackup(vbs));
        Assert.Equal(TweakState.On, Tweaks.State(def));
        int writes = _reg.Writes;
        Assert.Empty(Tweaks.RevertRetired());                                            // nothing left to do
        Assert.Equal(writes, _reg.Writes);
    }

    [Fact]
    public void StayOnThisReleaseUsesTheRunningRelease()
    {
        Assert.Equal("26H2", Tweaks.PinRelease(26200, "25H2"));                         // held on 26H2, as the playbook does
        Assert.Equal("26H2", Tweaks.PinRelease(26300, "26H2"));
        Assert.Equal("24H2", Tweaks.PinRelease(26100, "24H2"));
        Assert.Equal("26H2", Tweaks.PinRelease(0, null));
        var wu = Tweaks.Catalog.First(t => t.Id == "wu-feature");
        Assert.DoesNotContain(wu.Changes, c => c.On == "25H2");
    }

    [Fact]
    public void ApplySavesWhatWasThereAndRevertPutsExactlyThatBack()
    {
        var dvr = Tweaks.Catalog.First(t => t.Id == "game-dvr");
        var first = dvr.Changes[0];
        _reg.Set(first.Hive, first.Path, first.Name, "dword", "7");                 // an unusual value the user had
        Assert.Equal(TweakState.Off, Tweaks.State(dvr));
        Assert.Null(Tweaks.Apply(dvr));
        Assert.Equal(TweakState.On, Tweaks.State(dvr));
        Assert.True(Tweaks.HasBackup(dvr));
        int writes = _reg.Writes;
        Assert.Null(Tweaks.Apply(dvr));                                               // already on: nothing is written
        Assert.Equal(writes, _reg.Writes);

        Assert.Null(Tweaks.Revert(dvr));
        Assert.Equal("7", _reg.Get(first.Hive, first.Path, first.Name));              // the user's own value, not a guess
        Assert.Null(_reg.Get(dvr.Changes[2].Hive, dvr.Changes[2].Path, dvr.Changes[2].Name));   // there was no value: there is none again
        Assert.False(Tweaks.HasBackup(dvr));
        Assert.Equal(TweakState.Off, Tweaks.State(dvr));

        // Switched on by something else (the playbook): no backup, Revert writes the Windows defaults.
        var mouse = Tweaks.Catalog.First(t => t.Id == "mouse-accel");
        foreach (var c in mouse.Changes) _reg.Set(c.Hive, c.Path, c.Name, c.Kind, c.On);
        Assert.Equal(TweakState.On, Tweaks.State(mouse));
        Assert.Null(Tweaks.Revert(mouse));
        Assert.Equal("1", _reg.Get("HKCU", @"Control Panel\Mouse", "MouseSpeed"));
        Assert.Equal("6", _reg.Get("HKCU", @"Control Panel\Mouse", "MouseThreshold1"));

        // One value of three set: the state is "partly".
        _reg.Set(mouse.Changes[0].Hive, mouse.Changes[0].Path, mouse.Changes[0].Name, "sz", "0");
        Assert.Equal(TweakState.Mixed, Tweaks.State(mouse));
    }

    static byte[] Table(int version, int coreDelta, int memDelta, int coreMax = 250_000, int memMax = 2_000_000)
    {
        var b = new byte[version == NvOc.Version ? NvOc.Size : NvOc.SizeV2];
        void W(int at, int v) => BitConverter.GetBytes(v).CopyTo(b, at);
        W(0, version); W(4, 1); W(8, 1); W(12, 2);
        int c0 = 20 + 8, c1 = c0 + 44;
        W(c0, 0); W(c0 + 4, 1); W(c0 + 8, 1); W(c0 + 12, coreDelta); W(c0 + 16, -500_000); W(c0 + 20, coreMax); W(c0 + 24, 300_000); W(c0 + 28, 2_600_000);
        W(c1, 4); W(c1 + 4, 0); W(c1 + 8, 1); W(c1 + 12, memDelta); W(c1 + 16, -500_000); W(c1 + 20, memMax); W(c1 + 24, 9_000_000);
        return b;
    }

    [Fact]
    public void ClockTableIsParsedCheckedAndBuiltToTheDriversLayout()
    {
        Assert.Equal(7316, NvOc.Size);
        Assert.Equal(0x11C94, NvOc.Version);
        Assert.Equal(0x21CF8, NvOc.Version2);
        Assert.Equal(0x31CF8, NvOc.Version3);
        Assert.Equal(0x10028, NvOc.VoltVersion);

        var clocks = NvOc.Parse(Table(NvOc.Version3, 50_000, 0));
        Assert.Equal(2, clocks.Count);
        Assert.Equal((0, 2_600_000, 50_000, true), (clocks[0].Domain, clocks[0].BaseKHz, clocks[0].DeltaKHz, clocks[0].Editable));   // a range clock: its maximum
        Assert.Equal((4, 9_000_000), (clocks[1].Domain, clocks[1].BaseKHz));
        Assert.NotNull(NvOc.Parse(Table(NvOc.Version, 0, 0)));

        // Tables that do not look like one are refused, so nothing is ever written from a misread.
        Assert.Null(NvOc.Parse(new byte[100]));
        var bad = Table(NvOc.Version3, 0, 0);
        BitConverter.GetBytes(0x12345).CopyTo(bad, 0);
        Assert.Null(NvOc.Parse(bad));
        bad = Table(NvOc.Version3, 0, 0);
        BitConverter.GetBytes(40).CopyTo(bad, 12);                                     // 40 clocks: impossible
        Assert.Null(NvOc.Parse(bad));
        bad = Table(NvOc.Version3, 0, 0);
        BitConverter.GetBytes(5).CopyTo(bad, 20 + 8 + 28);                             // a 5 kHz core clock: impossible
        Assert.Null(NvOc.Parse(bad));

        // The request never leaves the driver's range nor our cap.
        Assert.Equal(250_000, NvOc.Clamp(clocks[0], 900_000));                         // the driver's maximum is lower than the cap
        Assert.Equal(NvOc.CapMemoryKHz, NvOc.Clamp(clocks[1], 5_000_000));              // the cap is lower than the driver's maximum
        Assert.Equal(NvOc.FloorKHz, NvOc.Clamp(clocks[0], -900_000));

        var set = NvOc.BuildSet(NvOc.DomainMemory, 125_000, NvOc.Version3);
        Assert.Equal(NvOc.SizeV2, set.Length);
        Assert.Equal(NvOc.Version3, BitConverter.ToInt32(set, 0));
        Assert.Equal((1, 1, 0), (BitConverter.ToInt32(set, 8), BitConverter.ToInt32(set, 12), BitConverter.ToInt32(set, 16)));
        Assert.Equal((4, 125_000), (BitConverter.ToInt32(set, 28), BitConverter.ToInt32(set, 40)));
        Assert.All(set.Skip(44), x => Assert.Equal(0, x));                             // nothing after the one delta is filled in
        var volt = NvOc.BuildVolt(250);
        Assert.Equal((0x10028, 100), (BitConverter.ToInt32(volt, 0), BitConverter.ToInt32(volt, 4)));
        Assert.Equal(40, volt.Length);
    }

    [Fact]
    public void OffsetsAndVoltageGoThroughTheFlagAreClampedAndReadBack()
    {
        int core = 0, mem = 0, volt = 0, sets = 0;
        NvOc.GetHook = () => Table(NvOc.Version3, core, mem);
        NvOc.SetHook = t =>
        {
            sets++;
            int domain = BitConverter.ToInt32(t, 28), delta = BitConverter.ToInt32(t, 40);
            if (domain == 0) core = delta; else mem = delta;
            return 0;
        };
        NvOc.VoltGetHook = () => NvOc.BuildVolt(volt);
        NvOc.VoltSetHook = b => { sets++; volt = BitConverter.ToInt32(b, 4); return 0; };

        Flags.Set(Flags.GpuOc, false);                                                  // open by default since 1.16.0: close it for this check
        Assert.Null(NvOc.Set(NvOc.DomainCore, 100_000));                               // the flag is closed
        Assert.Null(NvOc.SetVoltage(50));
        Assert.Equal(0, sets);

        Flags.Set(Flags.GpuOc, true);
        Assert.Equal(100_000, NvOc.Set(NvOc.DomainCore, 100_000));
        Assert.Equal(250_000, NvOc.Set(NvOc.DomainCore, 999_000));                     // clamped to the driver's range
        Assert.Equal(NvOc.CapMemoryKHz, NvOc.Set(NvOc.DomainMemory, 9_000_000));
        Assert.Equal(100, NvOc.SetVoltage(400));
        Assert.Equal(35, NvOc.SetVoltage(35));
        Assert.Equal((250_000, NvOc.CapMemoryKHz, 35), (core, mem, volt));

        NvOc.SetHook = _ => -104;                                                       // the card says "not supported"
        Assert.Null(NvOc.Set(NvOc.DomainCore, 50_000));
        Assert.Contains("does not support", NvOc.Error);

        NvOc.SetHook = t => { int d = BitConverter.ToInt32(t, 28); if (d == 0) core = 0; else mem = 0; return 0; };
        Assert.True(NvOc.Reset(force: true));                                           // only when asked: everything back to stock
        Assert.Equal((0, 0, 0), (core, mem, volt));
    }
}
