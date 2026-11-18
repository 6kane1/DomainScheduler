using Xunit;

namespace DomainScheduler.Tests;

/// <summary>1.12.0 safety rules: storage, MSI, laptops, the device boot guard, the background deny list.</summary>
[Collection("static-hooks")]
public class SafetyTests : IDisposable
{
    DateTime _now = new(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);

    public SafetyTests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        GameMask.IrqHook = () => new();
        Reservation.Reg = new FakeReserved();
        Settings.Backend = new MemorySettingsBackend();
    }

    public void Dispose()
    {
        Topo.LiveHook = null;
        GameMask.IrqHook = null;
        Reservation.Reg = new WinReservedRegistry();
        Settings.Backend = new RegistrySettingsBackend();
    }

    [Fact]
    public void StorageControllersAreRecognisedAndNeverListed()
    {
        Assert.True(IrqAuto.IsStorage("SCSIAdapter", "Standard NVM Express Controller", "stornvme"));
        Assert.True(IrqAuto.IsStorage("HDC", "Standard SATA AHCI Controller", "storahci"));
        Assert.True(IrqAuto.IsStorage("System", "Intel(R) RST VMD Controller", "iaStorVD"));
        Assert.True(IrqAuto.IsStorage("", "AMD-RAID Bottom Device", "rcbottom"));
        Assert.True(IrqAuto.IsStorage("", "Samsung NVMe Controller", ""));
        Assert.True(IrqAuto.IsStorage("", "x", "secnvme"));
        Assert.False(IrqAuto.IsStorage("Display", "NVIDIA GeForce RTX 5060", "nvlddmkm"));
        Assert.False(IrqAuto.IsStorage("USB", "AMD USB 3.10 eXtensible Host Controller - 1.10 (Microsoft)", "USBXHCI"));
        Assert.False(IrqAuto.IsStorage("Net", "Realtek PCIe 2.5GbE Family Controller", "rt25cx21x64"));
        Assert.False(IrqAuto.IsStorage("MEDIA", "Realtek(R) Audio", "IntcAzAudAddService"));
        Assert.True(Devices.IsStorage(new PciDevice { Class = "SCSIAdapter", Name = "Standard NVM Express Controller" }));
        Assert.Equal(@"PCI\VEN_144D&DEV_A80A\4&1", IrqAuto.DeviceOfEntry(@"HKLM|SYSTEM\CurrentControlSet\Enum\PCI\VEN_144D&DEV_A80A\4&1\Device Parameters\Interrupt Management\Affinity Policy|DevicePriority"));
        Assert.Null(IrqAuto.DeviceOfEntry(@"HKCU|Control Panel\Mouse|RawMouseThrottleDuration"));
    }

    [Fact]
    public void MsiIsSwitchedOnOnlyWhenTheDeviceOffersIt()
    {
        Assert.True(IrqAuto.MsiAllowed(null, usesMsiNow: true, driverLimit: false));     // already message-signalled
        Assert.True(IrqAuto.MsiAllowed(2, false, false));                                // PCI: MSI
        Assert.True(IrqAuto.MsiAllowed(5, false, false));                                // PCI: line + MSI-X
        Assert.False(IrqAuto.MsiAllowed(1, false, true));                                // PCI: line only — the property wins
        Assert.True(IrqAuto.MsiAllowed(null, false, true));                              // unknown, the driver set a limit
        Assert.False(IrqAuto.MsiAllowed(null, false, false));                            // unsure: no
    }

    [Fact]
    public void PowerSavingIsLeftAloneWithABatteryOrModernStandby()
    {
        Assert.True(IrqAuto.PowerSavingAllowed(128, false, false));     // desktop: no system battery
        Assert.False(IrqAuto.PowerSavingAllowed(1, true, false));       // laptop, battery high
        Assert.False(IrqAuto.PowerSavingAllowed(8, true, false));       // charging
        Assert.False(IrqAuto.PowerSavingAllowed(128, false, true));     // Modern Standby
        Assert.False(IrqAuto.PowerSavingAllowed(255, false, false));    // unknown: counts as a battery
    }

    static readonly DateTime Boot1 = new(2026, 11, 18, 10, 0, 0, DateTimeKind.Utc);
    static readonly DateTime Boot2 = Boot1.AddHours(2);
    static readonly DateTime Boot3 = Boot1.AddHours(3);

    static BootGuard.State Pending(DateTime seen = default) =>
        new() { Pending = true, AppliedUtc = Boot1.AddMinutes(30), AppliedBootUtc = Boot1, SeenBootUtc = seen };

    [Fact]
    public void BootGuardDecision()
    {
        Assert.Equal(BootGuard.Verdict.Nothing, BootGuard.Decide(null, Boot2, false));
        Assert.Equal(BootGuard.Verdict.Nothing, BootGuard.Decide(new BootGuard.State(), Boot2, true));
        // Same boot as the change (the program was restarted, Windows was not): wait.
        Assert.Equal(BootGuard.Verdict.Wait, BootGuard.Decide(Pending(), Boot1.AddSeconds(2), false));
        // First boot after the change, no crash: wait for the 2-minute confirmation.
        Assert.Equal(BootGuard.Verdict.Wait, BootGuard.Decide(Pending(), Boot2, false));
        // The program started again in that same boot: still wait.
        Assert.Equal(BootGuard.Verdict.Wait, BootGuard.Decide(Pending(Boot2), Boot2.AddMilliseconds(30), false));
        // Windows went down unexpectedly since the change: revert.
        Assert.Equal(BootGuard.Verdict.Revert, BootGuard.Decide(Pending(), Boot2, true));
        // A boot after the change ended before it was confirmed: revert.
        Assert.Equal(BootGuard.Verdict.Revert, BootGuard.Decide(Pending(Boot2), Boot3, false));
    }

    [Fact]
    public void BootGuardConfirmsOnlyInALaterBootAfterTwoMinutes()
    {
        var s = Pending();
        Assert.False(BootGuard.CanConfirm(s, Boot1, TimeSpan.FromHours(1)));          // not restarted yet: nothing is active
        Assert.False(BootGuard.CanConfirm(s, Boot2, TimeSpan.FromSeconds(50)));       // too early
        Assert.True(BootGuard.CanConfirm(s, Boot2, TimeSpan.FromMinutes(2)));
        Assert.False(BootGuard.CanConfirm(new BootGuard.State(), Boot2, TimeSpan.FromHours(1)));
    }

    [Fact]
    public void BootGuardRevertsAtTheNextStartAfterACrash()
    {
        string dir = Path.Combine(Path.GetTempPath(), "ds-guard-" + Guid.NewGuid().ToString("N"));
        var bootBefore = Reservation.BootUtc;
        BootGuard.PathHook = () => Path.Combine(dir, "guard.json");
        try
        {
            DateTime boot = Boot1;
            Reservation.BootUtc = () => boot;
            bool crash = false;
            BootGuard.CrashHook = _ => crash;
            int reverts = 0;
            Func<string> revert = () => { reverts++; return null; };

            BootGuard.MarkPending();
            Assert.True(BootGuard.Load().Pending);
            Assert.Empty(BootGuard.OnStart(revert));                                  // same boot: waits
            boot = Boot2;
            Assert.Empty(BootGuard.OnStart(revert));                                  // the next boot is seen
            Assert.Equal(Boot2, BootGuard.Load().SeenBootUtc);
            boot = Boot3;                                                             // that boot never confirmed
            Assert.Single(BootGuard.OnStart(revert));
            Assert.Equal(1, reverts);
            Assert.Null(BootGuard.Load());

            boot = Boot1;
            BootGuard.MarkPending();
            boot = Boot2;
            crash = true;                                                             // Kernel-Power 41 since the change
            Assert.Single(BootGuard.OnStart(revert));
            Assert.Equal(2, reverts);

            crash = false;
            boot = Boot1;
            BootGuard.MarkPending();
            boot = Boot2;
            Assert.Null(BootGuard.ConfirmIfDue(TimeSpan.FromSeconds(30)));
            Assert.NotNull(BootGuard.ConfirmIfDue(TimeSpan.FromMinutes(3)));
            Assert.Null(BootGuard.Load());
            Assert.Empty(BootGuard.OnStart(revert));
            Assert.Equal(2, reverts);
        }
        finally
        {
            BootGuard.PathHook = null;
            BootGuard.CrashHook = null;
            Reservation.BootUtc = bootBefore;
            if (Directory.Exists(dir)) Directory.Delete(dir, true);
        }
    }

    [Fact]
    public void AudioStreamingVoiceAndVrAreNeverChanged()
    {
        foreach (var exe in new[] { "obs64.exe", "Discord.exe", "discord.exe", "voicemeeter8x64.exe", "VoiceMeeterPro.exe", "audiodg.exe",
                                    "DolbyDAX3API.exe", "vrserver.exe", "OVRServer_x64.exe", "VirtualDesktop.Streamer.exe", "NVIDIA Broadcast.exe",
                                    "ts3client_win64.exe", "WaveLink.exe", "NahimicSvc64.exe", "RtkAudUService64.exe", "medal.exe", "nvsphelper64.exe" })
            Assert.True(BackgroundDeny.Contains(exe), exe);
        Assert.False(BackgroundDeny.Contains("GCC.exe"));
        Assert.False(BackgroundDeny.Contains("nvcontainer.exe"));
        Assert.False(BackgroundDeny.Contains(""));
        Assert.False(BackgroundDeny.Contains("voicemeeter.txt"));

        var f = FakeNative.Cs2();
        var e = new Engine(f) { Now = () => _now };
        f.Procs[900] = new FakeNative.P { Name = "obs64.exe", Created = 5 };
        f.Procs[901] = new FakeNative.P { Name = "voicemeeter8x64.exe", Created = 6 };
        f.Procs[902] = new FakeNative.P { Name = "NahimicSvc64.exe", Created = 7 };       // a service: the timer-only path
        f.Procs[903] = new FakeNative.P { Name = "GCC.exe", Created = 8 };
        e.ApplyBackground(new uint[] { 900, 901, 903 }, 0x3, true, new uint[] { 902 });
        foreach (uint pid in new uint[] { 900, 901, 902 })
        {
            var p = f.Procs[pid];
            Assert.Equal(32u, p.Cls);
            Assert.Equal(2u, p.Io);
            Assert.Equal((0u, 0u), (p.EcoCtl, p.EcoState));
            Assert.Equal(ulong.MaxValue, p.Aff);
        }
        Assert.Equal(16384u, f.Procs[903].Cls);
        Assert.Equal(1u, f.Procs[903].Io);                                                 // low, never very low
    }

    [Fact]
    public void LaunchersAndTheShellAreNotLoweredByAuto()
    {
        var f = FakeNative.Cs2();
        var p = new FakeProbe();
        f.Procs[1] = new FakeNative.P { Name = "DomainScheduler.exe", Created = 1 };
        f.Procs[810] = new FakeNative.P { Name = "explorer.exe", Created = 5 };
        f.Procs[811] = new FakeNative.P { Name = "steamwebhelper.exe", Created = 6 };
        f.Procs[812] = new FakeNative.P { Name = "Discord.exe", Created = 7 };
        f.Procs[813] = new FakeNative.P { Name = "GCC.exe", Created = 8 };
        var e = new Engine(f) { Now = () => _now, Sleep = ms => _now = _now.AddMilliseconds(ms) };
        var c = new AutoController(e, p, new FakeFrames(), () => _now) { SaveSession = _ => { } };
        c.Click();
        for (int i = 0; i < 12; i++) { _now = _now.AddSeconds(1); c.Tick(); }
        Assert.Equal(16384u, f.Procs[813].Cls);
        foreach (uint pid in new uint[] { 810, 811, 812 })
        {
            Assert.Equal(32u, f.Procs[pid].Cls);
            Assert.Equal((0u, 0u), (f.Procs[pid].EcoCtl, f.Procs[pid].EcoState));
        }
        c.Click();
    }
}
