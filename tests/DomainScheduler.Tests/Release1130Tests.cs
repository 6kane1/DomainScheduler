using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>1.13.0: the home device button's state, GPU slider ranges, the GPU profile store and the start decision.</summary>
public class Release1130Tests : IDisposable
{
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-gpu-" + Guid.NewGuid().ToString("N"));
    string FileP => Path.Combine(_dir, "gpu-profiles.json");

    public void Dispose()
    {
        try { Directory.Delete(_dir, true); } catch (Exception) { }
    }

    [Fact]
    public void HomeDeviceStateMatchesApplyAndBoot()
    {
        var boot = new DateTime(2026, 11, 18, 8, 0, 0, DateTimeKind.Utc);
        Assert.Equal(BootGuard.HomeDevices.Offer, BootGuard.HomeState(false, false, default, boot));
        Assert.Equal(BootGuard.HomeDevices.BlockedByGame, BootGuard.HomeState(false, true, default, boot));
        // Applied in this boot: restart needed, a running game does not change that.
        Assert.Equal(BootGuard.HomeDevices.AppliedRestartNeeded, BootGuard.HomeState(true, false, boot.AddMinutes(30), boot));
        Assert.Equal(BootGuard.HomeDevices.AppliedRestartNeeded, BootGuard.HomeState(true, true, boot.AddMinutes(30), boot));
        // Applied before this boot (Windows restarted since), or confirmed (no unconfirmed change): active, no note.
        Assert.Equal(BootGuard.HomeDevices.AppliedActive, BootGuard.HomeState(true, false, boot.AddMinutes(-5), boot));
        Assert.Equal(BootGuard.HomeDevices.AppliedActive, BootGuard.HomeState(true, true, boot.AddMinutes(-5), boot));
        Assert.Equal(BootGuard.HomeDevices.AppliedActive, BootGuard.HomeState(true, false, default, boot));
    }

    [Fact]
    public void SliderRangesFollowTheDriverAndTheCaps()
    {
        // The driver allows −1000…+1000 MHz on the core: NvOc's caps cut it to −300…+300.
        var core = new NvClock { Domain = NvOc.DomainCore, Editable = true, MinDeltaKHz = -1_000_000, MaxDeltaKHz = 1_000_000 };
        Assert.Equal(new GpuRange(-300, 300, 5), GpuOcLogic.CoreRange(core));
        // A narrower driver range wins, pulled inward onto the 25 MHz grid.
        var mem = new NvClock { Domain = NvOc.DomainMemory, Editable = true, MinDeltaKHz = -510_000, MaxDeltaKHz = 1_010_000 };
        Assert.Equal(new GpuRange(-300, 1000, 25), GpuOcLogic.MemRange(mem));
        var mem2 = new NvClock { Domain = NvOc.DomainMemory, Editable = true, MinDeltaKHz = -240_000, MaxDeltaKHz = 990_000 };
        Assert.Equal(new GpuRange(-225, 975, 25), GpuOcLogic.MemRange(mem2));
        // Nothing reported: the fixed ranges, still under the caps.
        Assert.Equal(new GpuRange(-200, 300, 5), GpuOcLogic.CoreRange(null));
        Assert.Equal(new GpuRange(-300, 1500, 25), GpuOcLogic.MemRange(null));
        Assert.Equal(new GpuRange(0, 100, 1), GpuOcLogic.Volt);
    }

    [Fact]
    public void SnapClampsAndRoundsToTheStep()
    {
        var r = new GpuRange(-225, 975, 25);
        Assert.Equal(0, r.Snap(0));
        Assert.Equal(25, r.Snap(13));
        Assert.Equal(0, r.Snap(12));
        Assert.Equal(-225, r.Snap(-5000));
        Assert.Equal(975, r.Snap(2000));
        Assert.Equal(975, r.Snap(974));
        Assert.Equal(0, r.Snap(double.NaN));
        var c = new GpuRange(-300, 300, 5);
        Assert.Equal(105, c.Snap(103));
        Assert.Equal(-105, c.Snap(-103));
        Assert.Equal(55, GpuOcLogic.Volt.Snap(55.4));
        Assert.Equal(0, GpuOcLogic.Volt.Snap(-3));
    }

    [Fact]
    public void ProfileStoreSavesRenamesDeletesAndKeepsTheStartMark()
    {
        var g = new GpuProfiles();
        Assert.True(g.Put("  Daily  ", 100, 500, 0));
        Assert.True(g.Put("Bench", 150, 1000, 30));
        Assert.False(g.Put("   ", 1, 1, 1));
        Assert.False(g.Put(new string('x', GpuProfiles.MaxName + 1), 1, 1, 1));
        Assert.True(g.Put("daily", 105, 525, 150));     // same name in another case: overwritten, voltage clamped
        Assert.Equal(2, g.Profiles.Count);
        Assert.Equal((105, 525, 100), (g.Find("Daily").CoreMHz, g.Find("Daily").MemMHz, g.Find("Daily").VoltPct));

        Assert.True(g.SetAtStart("bench"));
        Assert.Equal("Bench", g.AtStart);
        Assert.False(g.SetAtStart("nope"));
        Assert.False(g.Rename("Bench", "DAILY"));        // taken
        Assert.False(g.Rename("Bench", ""));
        Assert.True(g.Rename("Bench", "Bench 2"));
        Assert.Equal("Bench 2", g.AtStart);              // the mark follows the rename
        Assert.True(g.Rename("Bench 2", "bench 2"));     // only the case changes

        g.Save(FileP);
        var back = GpuProfiles.Load(FileP);
        Assert.Equal(2, back.Profiles.Count);
        Assert.Equal("bench 2", back.StartProfile?.Name);
        Assert.Equal(150, back.StartProfile.CoreMHz);

        Assert.True(back.Delete("BENCH 2"));
        Assert.Equal("", back.AtStart);                  // the mark goes with the profile
        Assert.False(back.Delete("BENCH 2"));
        Assert.Single(back.Profiles);
    }

    [Fact]
    public void BrokenOrMissingFileGivesAnEmptyStore()
    {
        Assert.Empty(GpuProfiles.Load(FileP).Profiles);
        Directory.CreateDirectory(_dir);
        File.WriteAllText(FileP, "{ not json");
        Assert.Empty(GpuProfiles.Load(FileP).Profiles);
        File.WriteAllText(FileP, "{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":50},{\"Name\":\"a\"},{\"Name\":\"\"},null],\"AtStart\":\"ghost\"}");
        var g = GpuProfiles.Load(FileP);
        Assert.Single(g.Profiles);
        Assert.Equal(50, g.Profiles[0].CoreMHz);
        Assert.Equal("", g.AtStart);                     // points at no profile: dropped
    }

    [Fact]
    public void StartDecisionKeepsTheCrashGuard()
    {
        var g = new GpuProfiles();
        g.Put("Daily", 100, 500, 0);
        g.Put("Stock", 0, 0, 0);
        Assert.Equal(GpuOcLogic.Start.Nothing, GpuOcLogic.DecideStart(true, g, false).What);   // nothing marked
        g.SetAtStart("Daily");
        Assert.Equal(GpuOcLogic.Start.Apply, GpuOcLogic.DecideStart(true, g, false).What);
        Assert.Equal("Daily", GpuOcLogic.DecideStart(true, g, false).Profile.Name);
        Assert.Equal(GpuOcLogic.Start.SkipCrash, GpuOcLogic.DecideStart(true, g, true).What);  // last session ended < 10 min after an apply
        Assert.Equal(GpuOcLogic.Start.Nothing, GpuOcLogic.DecideStart(false, g, false).What);  // the feature flag is closed
        Assert.Equal(GpuOcLogic.Start.Nothing, GpuOcLogic.DecideStart(false, g, true).What);
        g.SetAtStart("Stock");
        Assert.Equal(GpuOcLogic.Start.Nothing, GpuOcLogic.DecideStart(true, g, true).What);    // all zero: nothing to apply
        Assert.Equal(GpuOcLogic.Start.Nothing, GpuOcLogic.DecideStart(true, null, false).What);
    }

    [Fact]
    public void OldStartValuesBecomeAProfileOnce()
    {
        var g = new GpuProfiles();
        Assert.False(GpuOcLogic.Migrate(g, fileExists: true, atStart: true, 100_000, 0, 0, "Saved"));
        Assert.False(GpuOcLogic.Migrate(g, false, atStart: false, 100_000, 0, 0, "Saved"));
        Assert.False(GpuOcLogic.Migrate(g, false, true, 0, 0, 0, "Saved"));
        Assert.True(GpuOcLogic.Migrate(g, false, true, 120_000, 500_000, 20, "Saved"));
        Assert.Equal("Saved", g.AtStart);
        Assert.Equal((120, 500, 20), (g.StartProfile.CoreMHz, g.StartProfile.MemMHz, g.StartProfile.VoltPct));
    }

    [Fact]
    public void MarkingAProfileClearsTheCrashNotice()
    {
        var g = new GpuProfiles();
        g.Put("Daily", 100, 0, 0);
        g.Skipped = "Daily";
        g.Rename("Daily", "Main");
        Assert.Equal("Main", g.Skipped);
        g.SetAtStart("Main");
        Assert.Equal("", g.Skipped);
    }
}
