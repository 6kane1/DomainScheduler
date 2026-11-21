using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>1.16.0 Test 1: the GPU crash memory (same values are not marked for the start again by one click).</summary>
public class Release1160Tests : IDisposable
{
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-1160-" + Guid.NewGuid().ToString("N"));

    public void Dispose()
    {
        try { Directory.Delete(_dir, true); } catch (Exception) { }
    }

    static readonly DateTime Crash = new(2026, 11, 19, 17, 10, 0, DateTimeKind.Utc);

    [Fact]
    public void CrashedValuesAreRecognisedAgain()
    {
        var g = new GpuProfiles();
        g.Put("Saved values", 105, 1500, 100, vendor: GpuOc.Nvidia);
        g.RecordCrash(g.Find("Saved values"), Crash);
        // the same values, under the same or another name
        Assert.NotNull(GpuOcLogic.CrashedBefore(g.Find("Saved values"), g.Crashes));
        g.Put("Copy", 105, 1500, 100, vendor: GpuOc.Nvidia);
        Assert.Equal(Crash, GpuOcLogic.CrashedBefore(g.Find("Copy"), g.Crashes).Utc);
        // higher in everything: still the crashed values or more
        g.Put("Higher", 120, 1600, 100, vendor: GpuOc.Nvidia);
        Assert.NotNull(GpuOcLogic.CrashedBefore(g.Find("Higher"), g.Crashes));
    }

    [Fact]
    public void LowerOrOtherMakerIsNotWarned()
    {
        var g = new GpuProfiles();
        g.Put("Saved values", 105, 1500, 100, vendor: GpuOc.Nvidia);
        g.RecordCrash(g.Find("Saved values"), Crash);
        g.Put("Softer memory", 105, 1000, 100, vendor: GpuOc.Nvidia);
        Assert.Null(GpuOcLogic.CrashedBefore(g.Find("Softer memory"), g.Crashes));
        g.Put("Softer core", 60, 1500, 100, vendor: GpuOc.Nvidia);
        Assert.Null(GpuOcLogic.CrashedBefore(g.Find("Softer core"), g.Crashes));
        g.Put("Less voltage", 105, 1500, 50, vendor: GpuOc.Nvidia);
        Assert.Null(GpuOcLogic.CrashedBefore(g.Find("Less voltage"), g.Crashes));
        g.Put("Radeon", 105, 1500, 0, vendor: GpuOc.Amd);
        Assert.Null(GpuOcLogic.CrashedBefore(g.Find("Radeon"), g.Crashes));
        g.Put("Stock", 0, 0, 0, vendor: GpuOc.Nvidia);
        Assert.Null(GpuOcLogic.CrashedBefore(g.Find("Stock"), g.Crashes));
        Assert.Null(GpuOcLogic.CrashedBefore(g.Find("Stock"), null));
    }

    [Fact]
    public void CrashMemorySurvivesSaveLoadAndIsCapped()
    {
        string file = Path.Combine(_dir, "gpu-profiles.json");
        var g = new GpuProfiles();
        g.Put("P", 100, 1000, 0, vendor: GpuOc.Nvidia);
        for (int i = 0; i < GpuProfiles.MaxCrashes + 5; i++) g.RecordCrash(g.Find("P"), Crash.AddDays(i));
        Assert.Equal(GpuProfiles.MaxCrashes, g.Crashes.Count);
        Assert.Equal(Crash.AddDays(5), g.Crashes[0].Utc);          // the oldest ones went first
        g.Save(file);
        var back = GpuProfiles.Load(file);
        Assert.Equal(GpuProfiles.MaxCrashes, back.Crashes.Count);
        Assert.Equal(1000, back.Crashes[^1].MemMHz);
        Assert.Equal(Crash.AddDays(GpuProfiles.MaxCrashes + 4), GpuOcLogic.CrashedBefore(back.Find("P"), back.Crashes).Utc);   // the newest crash
    }

    [Fact]
    public void OldFileWithoutCrashesLoads()
    {
        Directory.CreateDirectory(_dir);
        string file = Path.Combine(_dir, "gpu-profiles.json");
        File.WriteAllText(file, "{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":50,\"MemMHz\":500}],\"AtStart\":\"A\"}");
        var g = GpuProfiles.Load(file);
        Assert.NotNull(g.Crashes);
        Assert.Empty(g.Crashes);
        Assert.Equal("A", g.AtStart);
    }

    [Fact]
    public void ThreadSharesCountOnlyWithinTheWindow()
    {
        SelfThreads.Shares(new(), 0);                                                   // forget earlier readings
        var first = new Dictionary<string, (uint Tid, long Time)> { ["worker"] = (10, 5_000_000), ["window"] = (11, 20_000_000) };
        Assert.Empty(SelfThreads.Shares(first, 0));                                     // the first reading only sets the base
        var second = new Dictionary<string, (uint Tid, long Time)> { ["worker"] = (10, 5_000_000 + 3_000_000), ["window"] = (11, 20_000_000 + 6_000_000), ["frame trace"] = (12, 1_200_000) };
        var s = SelfThreads.Shares(second, 60);
        Assert.Equal(0.005, s["worker"], 6);                                            // 0.3 s in 60 s = 0.5 % of one processor
        Assert.Equal(0.01, s["window"], 6);
        Assert.Equal(0.002, s["frame trace"], 6);                                       // started within the window: counted from zero
        var third = new Dictionary<string, (uint Tid, long Time)> { ["worker"] = (99, 600_000) };   // another thread under the same name
        Assert.Equal(0.001, SelfThreads.Shares(third, 60)["worker"], 6);
    }

    [Fact]
    public void ChannelNeedsKeyAndChoice()
    {
        Assert.Equal("test", AppVersion.ChannelFor(true, true));
        Assert.Equal("stable", AppVersion.ChannelFor(true, false));     // key kept, stable releases only
        Assert.Equal("stable", AppVersion.ChannelFor(false, true));
        Assert.Equal("stable", AppVersion.ChannelFor(false, false));
    }
}
