using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>
/// 1.15.0 Test 1.5: AMD Radeon overclocking through ADL — choosing the card, Overdrive8 vs OverdriveN, ranges and caps,
/// what is written (only the top state on OverdriveN, nothing outside the driver's range), the reset to the driver's
/// stock values (also after a failed apply), the feature flag, a missing driver, and the crash guard with AMD profiles.
/// </summary>
[Collection("static-hooks")]
public class AmdOcTests : IDisposable
{
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-amd-" + Guid.NewGuid().ToString("N"));

    public AmdOcTests()
    {
        Settings.Backend = new MemorySettingsBackend();
        Flags.Set(Flags.GpuOc, true);
    }

    public void Dispose()
    {
        AmdOc.Use(null);
        Settings.Backend = new RegistrySettingsBackend();
        try { Directory.Delete(_dir, true); } catch (Exception) { }
    }

    static FakeAdl Use(FakeAdl f)
    {
        AmdOc.Use(f);
        return f;
    }

    // ---- choosing the card ----

    [Fact]
    public void PicksTheActivePhysicalAmdCardOncePerBus()
    {
        var all = new List<AdlAdapter>
        {
            new() { Index = 0, Bus = 5, Vendor = 0x10DE, Active = true, OdSupported = 1, OdVersion = 8 },             // NVIDIA
            new() { Index = 1, Bus = -1, Vendor = 1002, Active = true, OdSupported = 1, OdVersion = 8 },              // no bus: virtual
            new() { Index = 2, Bus = 7, Vendor = 1002, Active = false, OdSupported = 1, OdVersion = 8 },              // not active
            new() { Index = 3, Bus = 9, Vendor = 1002, Active = true, OdSupported = 1, OdVersion = 8, AsicKnown = true, AsicTypes = 2 },   // integrated
            new() { Index = 5, Bus = 3, Vendor = 1002, Active = true, OdSupported = 1, OdVersion = 7 },
            new() { Index = 4, Bus = 3, Vendor = 1002, Active = true, OdSupported = 1, OdVersion = 7 },               // same card, lower index
        };
        var (a, why) = AmdOc.Pick(all);
        Assert.Equal("", why);
        Assert.Equal(4, a.Index);
        Assert.Equal(7, a.OdVersion);
    }

    [Fact]
    public void OlderOrMissingOverdriveIsRefusedWithAReason()
    {
        var (a, why) = AmdOc.Pick(new[] { new AdlAdapter { Bus = 1, Vendor = 1002, Active = true, OdSupported = 1, OdVersion = 6 } });
        Assert.Null(a);
        Assert.Contains("6", why);
        (a, why) = AmdOc.Pick(new[] { new AdlAdapter { Bus = 1, Vendor = 1002, Active = true, OdSupported = 0, OdVersion = 8 } });
        Assert.Null(a);
        Assert.Contains("no overclocking support", why);
        (a, why) = AmdOc.Pick(Array.Empty<AdlAdapter>());
        Assert.Null(a);
        Assert.Contains("No active AMD", why);
    }

    [Fact]
    public void Version8IsOverdrive8AndVersion7IsOverdriveN()
    {
        Use(FakeAdl.Rx6800());
        var s = AmdOc.Read();
        Assert.Equal(8, s.Version);
        Assert.Equal("Overdrive8", s.Kind);
        Assert.Equal("AMD Radeon RX 6800 XT", s.Name);
        Use(FakeAdl.Rx580());
        s = AmdOc.Read();
        Assert.Equal(7, s.Version);
        Assert.Equal("OverdriveN", s.Kind);
    }

    [Fact]
    public void AMissingDriverOrNoOverdriveDoesNothing()
    {
        var f = Use(new FakeAdl { Missing = true });
        Assert.Null(AmdOc.Read());
        Assert.Contains("atiadlxx.dll", AmdOc.Error);
        Assert.Null(AmdOc.Apply(100, 100, 10, 0));
        Assert.False(AmdOc.Reset(force: true));
        Assert.Equal(0, f.Writes);

        f = FakeAdl.Rx580();
        f.Adapters.ForEach(a => a.Supported = 0);
        Use(f);
        Assert.Null(AmdOc.Read());
        Assert.Contains("no overclocking support", AmdOc.Error);
        Assert.Null(AmdOc.Apply(100, 100, 10, 0));
        Assert.Equal(0, f.Writes);
    }

    [Fact]
    public void AClosedFlagRefusesTheWrite()
    {
        var f = Use(FakeAdl.Rx580());
        Flags.Set(Flags.GpuOc, false);
        Assert.Null(AmdOc.Apply(50, 0, 0, 0));
        Assert.Contains("gpu-oc", AmdOc.Error);
        Assert.Equal(0, f.Writes);
    }

    // ---- RX 580 (OverdriveN) ----

    [Fact]
    public void Rx580RangesComeFromTheTopStateTheDriverRangeAndTheCaps()
    {
        Use(FakeAdl.Rx580());
        var s = AmdOc.Read();
        Assert.Equal(100, s.CoreScale);                       // 10 kHz units
        Assert.Equal((1366, 1366, 2000), (s.Core.Default, s.Core.Current, s.Core.Max));
        Assert.Equal(1300, s.Core.Min);                       // never below the state under the top one
        Assert.Equal((2000, 2000, 2250), (s.Mem.Default, s.Mem.Min, s.Mem.Max));   // no underclocking flag: not below stock
        Assert.Equal((-50, 50, 0), (s.Power.Min, s.Power.Max, s.Power.Default));
        Assert.False(s.Volt.Editable);                        // OverdriveN: voltages are never changed
        var clocks = AmdOc.Clocks(s);
        Assert.Equal(new GpuRange(-65, 300, 5), GpuOcLogic.CoreRange(clocks[0]));   // +634 allowed by the driver, our cap is +300
        Assert.Equal(new GpuRange(0, 250, 25), GpuOcLogic.MemRange(clocks[1]));
        Assert.Equal(new GpuRange(-50, 50, 1), GpuOcLogic.PowerRange(s.Power));
        Assert.Equal(1366_000, clocks[0].BaseKHz);
    }

    [Fact]
    public void Rx580ApplyRaisesOnlyTheTopStateAndKeepsEveryVoltage()
    {
        var f = Use(FakeAdl.Rx580());
        var lowSys = f.SysClk.Take(7).ToArray();
        var s = AmdOc.Apply(50, 100, 20, -50);
        Assert.Equal("", AmdOc.Error);
        Assert.Equal((50, 100, 20), (s.Core.Offset, s.Mem.Offset, s.Power.Offset));
        var sys = f.OdnSets.Single(x => !x.Memory);
        Assert.Equal(AmdOc.ModeManual, sys.Mode);
        Assert.Equal(lowSys, sys.Clocks.Take(7).ToArray());   // lower states untouched
        Assert.Equal(141_600, sys.Clocks[7]);
        Assert.Equal(new[] { 750, 769, 906, 1000, 1056, 1100, 1125, 1150 }, sys.Vddc);   // voltages as read
        Assert.All(sys.Enabled, e => Assert.InRange(e, 0, 1));
        var mem = f.OdnSets.Single(x => x.Memory);
        Assert.Equal(new[] { 30_000, 100_000, 210_000 }, mem.Clocks);
        Assert.Equal((AmdOc.ModeManual, 20), f.PowerSets.Single());
    }

    [Fact]
    public void Rx580ValuesAreCutToTheCapsAndTheDriverRange()
    {
        var f = Use(FakeAdl.Rx580());
        var s = AmdOc.Apply(1000, 5000, 200, 0);
        Assert.Equal((300, 250, 50), (s.Core.Offset, s.Mem.Offset, s.Power.Offset));
        Assert.Equal(166_600, f.SysClk[7]);
        s = AmdOc.Apply(-500, -500, -200, 0);
        Assert.Equal((-66, 0, -50), (s.Core.Offset, s.Mem.Offset, s.Power.Offset));
        Assert.Equal(130_000, f.SysClk[7]);                  // equal to the state under it, never lower
    }

    [Fact]
    public void Rx580UnchangedValuesAreNotWritten()
    {
        var f = Use(FakeAdl.Rx580());
        AmdOc.Apply(0, 0, 0, 0);
        Assert.Equal(0, f.Writes);
        AmdOc.Apply(25, 0, 0, 0);
        Assert.Single(f.OdnSets);
        Assert.Empty(f.PowerSets);
    }

    [Fact]
    public void Rx580ResetUsesTheDriversStockTableAlsoAfterAFailedApply()
    {
        var f = Use(FakeAdl.Rx580());
        f.FailMemSet = -3;
        var s = AmdOc.Apply(60, 100, 10, 0);
        Assert.NotEqual("", AmdOc.Error);                    // the memory write failed
        Assert.Equal(60, s.Core.Offset);                      // the core write went through
        f.FailMemSet = 0;
        Assert.True(AmdOc.Reset(force: true));
        Assert.Equal(f.SysDefClk, f.SysClk);
        Assert.Equal(f.MemDefClk, f.MemClk);
        Assert.Equal(AmdOc.ModeDefault, f.OdnSets[^1].Mode);
        Assert.Equal((AmdOc.ModeAuto, 0), f.PowerSets[^1]);
        Assert.Equal(0, f.Tdp);
        // Nothing changed since: an unforced reset writes nothing.
        int writes = f.Writes;
        Assert.False(AmdOc.Reset());
        Assert.Equal(writes, f.Writes);
    }

    [Fact]
    public void Rx580ResetWorksInANewSessionWithoutAnyRememberedState()
    {
        var f = FakeAdl.Rx580();
        f.SysClk[7] = 150_000;                                // set by an earlier run of the program
        f.Tdp = 30;
        Use(f);
        Assert.True(AmdOc.Reset(force: true));
        Assert.Equal(136_600, f.SysClk[7]);
        Assert.Equal(0, f.Tdp);
    }

    [Fact]
    public void OverdriveNClocksInMhzAndTheOldCallsWork()
    {
        Assert.Equal(100, AmdOc.Scale(200_000));
        Assert.Equal(1, AmdOc.Scale(2000));
        var f = Use(FakeAdl.Rx580(x2: false, scale: 1));
        var s = AmdOc.Read();
        Assert.Equal(1, s.CoreScale);
        Assert.False(s.X2);
        Assert.Equal((1366, 1300), (s.Core.Default, s.Core.Min));
        AmdOc.Apply(40, 0, 0, 0);
        Assert.Equal(1406, f.SysClk[7]);
        Assert.Equal(new[] { 300, 600, 900, 1145, 1215, 1257, 1300 }, f.SysClk.Take(7).ToArray());
    }

    [Fact]
    public void OverdriveNIsNotOfferedWhenAVoltageIsNotWhatTheDriverReports()
    {
        var f = FakeAdl.Rx580();
        f.SysVddc[3] = 5000;                                 // neither in the voltage range nor in the stock table
        f.DefaultReadFails = true;
        Use(f);
        var s = AmdOc.Read();
        Assert.False(s.Core.Editable);
        AmdOc.Apply(50, 0, 0, 0);
        Assert.DoesNotContain(f.OdnSets, x => !x.Memory);
    }

    [Fact]
    public void OverdriveNTopStateTheDriverLocksIsNotOffered()
    {
        var f = FakeAdl.Rx580();
        f.SysControl = new[] { 7, 7, 7, 7, 7, 7, 7, 0 };       // the top state's clock is not editable
        Use(f);
        var s = AmdOc.Read();
        Assert.False(s.Core.Editable);
        Assert.Equal(new[] { NvOc.DomainMemory }, AmdOc.Clocks(s).Select(c => c.Domain).ToArray());   // the page says the core is locked
    }

    // ---- RX 6000/7000 (Overdrive8) ----

    [Fact]
    public void Rx6800RangesComeFromTheDriverTable()
    {
        Use(FakeAdl.Rx6800());
        var s = AmdOc.Read();
        Assert.Equal((2250, 500, 2800), (s.Core.Default, s.Core.Min, s.Core.Max));
        var clocks = AmdOc.Clocks(s);
        Assert.Equal(new GpuRange(-300, 300, 5), GpuOcLogic.CoreRange(clocks[0]));
        Assert.Equal(new GpuRange(0, 150, 25), GpuOcLogic.MemRange(clocks[1]));
        Assert.Equal(new GpuRange(-10, 15, 1), GpuOcLogic.PowerRange(s.Power));
        Assert.False(s.Volt.Editable);
    }

    [Fact]
    public void Rx6800ApplySwitchesToManualThenWritesOnlyWhatChanged()
    {
        var f = Use(FakeAdl.Rx6800());
        var s = AmdOc.Apply(100, 100, 15, 0);
        Assert.Equal("", AmdOc.Error);
        Assert.Equal((100, 100, 15), (s.Core.Offset, s.Mem.Offset, s.Power.Offset));
        Assert.Equal(2, f.Od8Sets.Count);
        Assert.Equal(new[] { (AmdOc.PowerMode, AmdOc.PowerModeManual, false) }, f.Od8Sets[0]);
        Assert.Equal(new[] { (0, 2350, false), (1, 500, false), (8, 1100, false), (9, 15, false) }, f.Od8Sets[1]);
        // Only the core again: the mode is already manual, memory and power are not sent.
        AmdOc.Apply(50, 100, 15, 0);
        Assert.Equal(new[] { (0, 2300, false), (1, 500, false) }, f.Od8Sets[2]);
    }

    [Fact]
    public void Rx6800ResetSendsTheResetFlagWithTheDefaults()
    {
        var f = Use(FakeAdl.Rx6800());
        AmdOc.Apply(100, 100, 15, 0);
        Assert.True(AmdOc.Reset(force: true));
        var reset = f.Od8Sets[^1];
        Assert.All(reset, x => Assert.True(x.Reset));
        Assert.Equal(new[] { 0, 1, 8, 9, 36 }, reset.Select(x => x.Id).ToArray());
        Assert.Equal((2250, 1000, 0, 0), (f.Cur[0], f.Cur[8], f.Cur[9], f.Cur[36]));
    }

    [Fact]
    public void Rx6800ARefusedApplyLeavesAWorkingReset()
    {
        var f = Use(FakeAdl.Rx6800());
        f.Od8SetRc = -3;
        Assert.NotNull(AmdOc.Apply(100, 0, 0, 0));
        Assert.Contains("refused", AmdOc.Error);
        f.Od8SetRc = 0;
        f.Cur[0] = 2400;                                     // whatever is in force, reset brings stock
        Assert.True(AmdOc.Reset(force: true));
        Assert.Equal(2250, f.Cur[0]);
    }

    [Fact]
    public void Rx7900VoltageOffsetOnlyGoesDown()
    {
        var f = Use(FakeAdl.Rx6800(voltage: true));
        var s = AmdOc.Read();
        Assert.True(s.Volt.Editable);
        Assert.Equal(0, s.Volt.Max);                         // the driver allows +50 mV: never offered
        Assert.Equal(new GpuRange(-200, 0, 5), GpuOcLogic.VoltMvRange(s.Volt));
        AmdOc.Apply(0, 0, 0, 30);
        Assert.Equal(0, f.Writes);                           // a raise is not written at all
        s = AmdOc.Apply(0, 0, 0, -50);
        Assert.Equal(-50, s.Volt.Offset);
        s = AmdOc.Apply(0, 0, 0, -900);
        Assert.Equal(-200, s.Volt.Offset);
        Assert.True(AmdOc.Reset(force: true));
        Assert.Equal(0, f.Cur[AmdOc.OdVoltage]);
    }

    [Fact]
    public void ImplausibleTablesAreNotOffered()
    {
        var f = FakeAdl.Rx6800();
        f.Od8(AmdOc.GfxMax, 500, 2800, 3500);                // stock outside its own range
        f.Od8(AmdOc.UclkMax, 0, 90_000, 1000);                // a memory clock no card has
        f.Od8(AmdOc.PowerPct, 10, -10, 0);                    // a reversed range
        Use(f);
        Assert.Null(AmdOc.Read());
        Assert.Contains("no adjustable", AmdOc.Error);
        Assert.Null(AmdOc.Apply(100, 100, 10, 0));
        Assert.Equal(0, f.Writes);
    }

    [Fact]
    public void TargetsStayOnTheDriversStepRoundedTowardsStock()
    {
        var d = new AmdDomain { Editable = true, Min = 300, Max = 2000, Default = 1366, Current = 1366, Step = 5 };
        Assert.Equal(1366, AmdOc.Target(d, 3, -300, 300));   // one grid step (1365/1370) would overshoot: stock
        Assert.Equal(1370, AmdOc.Target(d, 7, -300, 300));
        Assert.Equal(1360, AmdOc.Target(d, -7, -300, 300));
        Assert.Equal(1665, AmdOc.Target(d, 1000, -300, 300));
        Assert.Equal(1366, AmdOc.Target(d, 0, -300, 300));
    }

    [Fact]
    public void TemperatureComesFromTheRightSensor()
    {
        Use(FakeAdl.Rx580());
        Assert.NotNull(AmdOc.Read());
        Assert.Equal(65, AmdOc.Temperature());               // OverdriveN: millidegrees
        Use(FakeAdl.Rx6800());
        Assert.NotNull(AmdOc.Read());
        Assert.Equal(60, AmdOc.Temperature());               // Overdrive8: PMLog edge sensor
    }

    // ---- profiles and the start ----

    [Fact]
    public void AmdProfilesKeepTheirValuesAndMaker()
    {
        Directory.CreateDirectory(_dir);
        string file = Path.Combine(_dir, "gpu-profiles.json");
        var g = new GpuProfiles();
        Assert.True(g.Put("Radeon", 50, 100, 30, 12, -40, GpuOc.Amd));
        Assert.True(g.Put("Clamp", 0, 0, 0, 500, 80, GpuOc.Amd));
        g.Save(file);
        var back = GpuProfiles.Load(file);
        var p = back.Find("Radeon");
        Assert.Equal((50, 100, 30, 12, -40, "amd"), (p.CoreMHz, p.MemMHz, p.VoltPct, p.PowerPct, p.VoltMv, p.Vendor));
        var c = back.Find("Clamp");
        Assert.Equal((100, 0), (c.PowerPct, c.VoltMv));      // raising the voltage is never stored
        // A file of 1.15.0 Test 1.4: no new fields, an NVIDIA profile.
        File.WriteAllText(file, "{\"Profiles\":[{\"Name\":\"Old\",\"CoreMHz\":100,\"MemMHz\":500,\"VoltPct\":10}],\"AtStart\":\"Old\"}");
        var old = GpuProfiles.Load(file).Find("Old");
        Assert.Equal(("", "nvidia", 0, 0), (old.Vendor, old.Maker, old.PowerPct, old.VoltMv));
    }

    [Fact]
    public void CrashGuardAndStartWorkTheSameForAmd()
    {
        var g = new GpuProfiles();
        g.Put("Power only", 0, 0, 0, 10, 0, GpuOc.Amd);
        Assert.False(g.Find("Power only").IsStock);           // a power limit alone is an overclock to re-apply
        g.SetAtStart("Power only");
        Assert.Equal(GpuOcLogic.Start.Apply, GpuOcLogic.DecideStart(true, g, false).What);
        Assert.Equal(GpuOcLogic.Start.SkipCrash, GpuOcLogic.DecideStart(true, g, true).What);   // unexpected shutdown since the apply
        Assert.Equal(GpuOcLogic.Start.Nothing, GpuOcLogic.DecideStart(false, g, false).What);   // flag closed
        g.Put("Undervolt", 0, 0, 0, 0, -30, GpuOc.Amd);
        g.SetAtStart("Undervolt");
        Assert.Equal(GpuOcLogic.Start.SkipCrash, GpuOcLogic.DecideStart(true, g, true).What);
    }

    [Fact]
    public void ProfilesAreAppliedAtStartOnlyOnTheirMakersCard()
    {
        var g = new GpuProfiles();
        g.Put("Radeon", 50, 0, 0, 0, 0, GpuOc.Amd);
        g.Put("GeForce", 50, 0, 10, 0, 0, GpuOc.Nvidia);
        g.Put("Old", 50, 0, 10);
        Assert.True(GpuOcLogic.Fits(g.Find("Radeon"), GpuOc.Amd));
        Assert.False(GpuOcLogic.Fits(g.Find("Radeon"), GpuOc.Nvidia));
        Assert.True(GpuOcLogic.Fits(g.Find("GeForce"), GpuOc.Nvidia));
        Assert.False(GpuOcLogic.Fits(g.Find("Old"), GpuOc.Amd));
        Assert.True(GpuOcLogic.Fits(g.Find("Old"), GpuOc.Nvidia));
        Assert.False(GpuOcLogic.Fits(g.Find("Radeon"), ""));
    }

    [Fact]
    public void Od8SetBlockHasTheSdkLayout()
    {
        var b = AmdOc.BuildOd8Set(77, new[] { (0, 2350, false), (36, 3, true), (200, 1, false) });
        Assert.Equal(4 + 77 * 12, b.Length);
        Assert.Equal(77, BitConverter.ToInt32(b, 0));
        Assert.Equal((2350, 1, 0), (BitConverter.ToInt32(b, 4), BitConverter.ToInt32(b, 8), BitConverter.ToInt32(b, 12)));
        int at = 4 + 36 * 12;
        Assert.Equal((3, 1, 1), (BitConverter.ToInt32(b, at), BitConverter.ToInt32(b, at + 4), BitConverter.ToInt32(b, at + 8)));
        Assert.Equal(140, AmdOc.LevelsBytes(true, 8));      // ADLODNPerformanceLevelsX2: 3 ints + 8 × 16 bytes
        Assert.Equal(48, AmdOc.LevelsBytes(false, 3));      // ADLODNPerformanceLevels: 3 ints + 3 × 12 bytes
        Assert.Equal(1572, AmdOc.AdapterInfoSize);           // AdapterInfo on Windows
    }
}
