using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>
/// 1.11.0: the latency experiments of 1.9.1/1.10.0 (cores do not fall asleep, soft background, key threads +2)
/// lowered the frame rate and are not released. Their settings, toggles and code are gone; the behaviour is the one
/// from before: main/render threads +1, Top-5 lead slots +1, background on the background cores.
/// Only the cleanup stays: GamePower.Restore puts back power values a test build left in game-power.json.
/// </summary>
[Collection("static-hooks")]
public class LatencyDefaultsTests : IDisposable
{
    public LatencyDefaultsTests() => Settings.Backend = new MemorySettingsBackend();

    public void Dispose() => Settings.Backend = new RegistrySettingsBackend();

    static string Root()
    {
        string root = AppContext.BaseDirectory;
        while (root != null && !File.Exists(Path.Combine(root, "DomainScheduler.sln"))) root = Path.GetDirectoryName(root);
        return root;
    }

    static string AllSources()
    {
        var files = Directory.GetFiles(Path.Combine(Root(), "src"), "*.cs", SearchOption.AllDirectories)
            .Where(x => !x.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                     && !x.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar));
        return string.Join("\n", files.Select(File.ReadAllText));
    }

    [Theory]
    [InlineData("GameNoIdle")]
    [InlineData("SoftBackground")]
    [InlineData("HighBoost")]
    public void TheLatencyExperimentsHaveNoSettingAndNoToggle(string key)
    {
        string all = AllSources();
        Assert.DoesNotContain($"\"{key}\"", all);              // no settings key, no toggle, no read
        Assert.False(Settings.GetBool(key));                    // no default left in the table
        string page = File.ReadAllText(Path.Combine(Root(), "src", "DomainScheduler", "SettingsPage.cs"));
        Assert.DoesNotContain($"Toggle(\"{key}\"", page);
    }

    [Fact]
    public void TheLatencyCodeIsGoneButTheCleanupStays()
    {
        string all = AllSources();
        Assert.DoesNotContain("LatencyOptions", all);
        Assert.DoesNotContain("GamePower.Apply", all);
        Assert.DoesNotContain("PowerTick", all);
        Assert.DoesNotContain("PowerWriteACValueIndex(IntPtr.Zero, ref scheme, ref sub, ref st, value)", all);
        Assert.Contains("GamePower.Restore()", File.ReadAllText(Path.Combine(Root(), "src", "DomainScheduler", "App.xaml.cs")));
        Assert.Contains("GamePower.Restore()", File.ReadAllText(Path.Combine(Root(), "src", "DomainScheduler.Lib", "Maintenance.cs")));
        Assert.Contains("DefenderGameExclusion.RemoveAll()", all);
    }

    [Fact]
    public void RemovedSettingsAreGoneFromTheSources()
    {
        string all = AllSources();
        foreach (string key in new[] { "\"GameNoParking\"", "\"FrameWatchdog\"", "\"ThreadSetsOn\"", "\"Warmup\"", "\"DomainEngine\"" })
            Assert.DoesNotContain(key, all);
        Assert.DoesNotContain("DefenderGameExclusion.Add", all);
    }

    [Fact]
    public void KeyGameThreadsGetPlusOne()
    {
        var plan = new List<Classified>
        {
            new() { Row = new ThreadRow { Tid = 100, Born = 1000 }, Role = ThreadRole.Main, LP = 2 },
            new() { Row = new ThreadRow { Tid = 101, Born = 1001 }, Role = ThreadRole.Render, LP = 4 },
        };
        var preset = AutoLogic.BuildAutoPreset("cs2.exe", plan, true);
        Assert.Equal("above", preset.Threads[0].Prio);
        Assert.Equal("above", preset.Threads[1].Prio);
    }
}
