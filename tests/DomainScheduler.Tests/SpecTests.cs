using Xunit;

namespace DomainScheduler.Tests;

/// <summary>The preset layer, eligibility, reconciliation of built-ins, languages and the cleaner.</summary>
[Collection("static-hooks")]
public class SpecTests : IDisposable
{
    public SpecTests()
    {
        Topo.LiveHook = Synth.X7800X3D;
        Reservation.Reg = new FakeReserved();
        GameMask.IrqHook = () => new();
        Settings.Backend = new MemorySettingsBackend();
    }

    public void Dispose()
    {
        Topo.LiveHook = null;
        Reservation.Reg = new WinReservedRegistry();
        GameMask.IrqHook = null;
        Settings.Backend = new RegistrySettingsBackend();
        L.Load("en");
    }

    static FakeNative System()
    {
        var f = FakeNative.Cs2();
        f.Procs[600] = new FakeNative.P { Name = "discord.exe", Created = 5, Mods = new() { new ModRange { Base = 0x1000, Size = 0x1000, Name = "discord_voice.node" }, new ModRange { Base = 0x9000, Size = 0x1000, Name = "Discord.exe" } } };
        f.Threads[601] = new FakeNative.T { Pid = 600, Born = 1, Start = 0x1010 };
        f.Threads[602] = new FakeNative.T { Pid = 600, Born = 2, Start = 0x9010, Desc = "Compositor" };
        f.Procs[700] = new FakeNative.P { Name = "csrss.exe", Created = 6 };
        f.Threads[701] = new FakeNative.T { Pid = 700, Born = 1 };
        return f;
    }

    [Fact]
    public void SysLayerAppliesBuiltinsOnceAndUserPresetsOnlyOnTheWatchlist()
    {
        var f = System();
        var e = new Engine(f) { GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false) };
        var presets = new Dictionary<string, Preset>(StringComparer.OrdinalIgnoreCase);
        Store.MarkBuiltin(presets, ThreadIni.LoadBundled().Presets.Values);
        presets["cs2.exe"] = new Preset { Exe = "cs2.exe", Class = "above", Threads = { new ThreadRule { Match = "desc", Pattern = "MainThrd", Prio = "above" } } };
        presets["csrss.exe"] = new Preset { Exe = "csrss.exe", Class = "above" };
        var sys = new SysLayer(e);

        // cs2 is not on the watchlist: its user preset does nothing; discord gets the built-in one.
        sys.Configure(presets, Array.Empty<string>(), new(), "auto", true);
        Assert.Equal(1, sys.Pass());
        Assert.Equal(1, f.Threads[601].Prio);
        Assert.Equal(-1, f.Threads[602].Prio);
        Assert.True(f.Procs[600].BoostDisabled);
        Assert.Equal(0, f.Threads[100].Prio);
        Assert.DoesNotContain(500u, f.OpenedPids);
        Assert.DoesNotContain(700u, f.OpenedPids);      // denylisted: never opened

        // Nothing changed in the system: the second pass opens nothing and writes nothing.
        int opens = f.ProcessOpens, writes = f.Writes;
        Assert.Equal(0, sys.Pass());
        Assert.Equal(opens, f.ProcessOpens);
        Assert.Equal(writes, f.Writes);

        // On the watchlist the user preset applies; the process Auto owns is skipped.
        sys.Configure(presets, new[] { "cs2.exe", "csrss.exe" }, new(), "auto", true);
        sys.Pass(skip: 500);
        Assert.Equal(32u, f.Procs[500].Cls);
        sys.Pass();
        Assert.Equal(32768u, f.Procs[500].Cls);
        Assert.Equal(1, f.Threads[100].Prio);
        Assert.DoesNotContain(700u, f.OpenedPids);

        // A revert drops the cache: the next pass puts the presets back.
        e.RevertAll();
        Assert.Equal(0, f.Threads[601].Prio);
        Assert.False(f.Procs[600].BoostDisabled);
        sys.Pass();
        Assert.Equal(1, f.Threads[601].Prio);

        // The switch turns the built-ins off.
        e.RevertAll();
        sys.Configure(presets, Array.Empty<string>(), new(), "auto", false);
        Assert.Equal(0, sys.Pass());
    }

    [Fact]
    public void EligibilityNeedsTheWatchlistAndRespectsTheDenylist()
    {
        var watch = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "cs2.exe", "lsass.exe", "VALORANT-Win64-Shipping.exe" };
        Assert.Null(Guard.Eligibility("cs2.exe", watch));
        Assert.NotNull(Guard.Eligibility("other.exe", watch));
        Assert.NotNull(Guard.Eligibility("lsass.exe", watch));
        Assert.NotNull(Guard.Eligibility("VALORANT-Win64-Shipping.exe", watch));
        Assert.NotNull(Guard.Eligibility("cs2.exe", watch, new[] { "faceit.exe" }));
        Assert.True(Guard.IsDenied("csrss.exe"));
        Assert.False(Guard.IsDenied("dwm.exe"));
    }

    [Fact]
    public void ManualApisRefuseDenylistedProcessesWithoutOpeningThem()
    {
        var f = System();
        var e = new Engine(f) { GateHook = (pid, exe) => Guard.Gate(exe, Guard.Op.Process, false) };
        var log = new List<string>();
        Assert.False(e.ProcessClassManual(700, "above", log));
        Assert.False(e.ProcessBoost(700, true, log));
        Assert.Contains("system-critical", e.ThreadManual(700, 701, "above", "", ""));
        Assert.True(e.ReadProcess(700, "csrss.exe").Denied);
        Assert.DoesNotContain(700u, f.OpenedPids);
        Assert.Equal(0, f.Writes);
        // A thread of a denylisted module inside an allowed process is not changed either.
        e.Snapshot(500);
        Assert.Contains("denylisted", e.ThreadManual(500, 106, "above", "", ""));
        Assert.Equal(0, f.Threads[106].Prio);
    }

    [Fact]
    public void AutoPicksOnlyAGameOnTheWatchlist()
    {
        var f = FakeNative.Cs2();
        DateTime now = new(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);
        var probe = new FakeProbe { Fg = 500 };
        probe.Windows.Add(500);
        probe.Graphics.Add(500);
        probe.Cpu[500] = (77, 0);
        void Advance(int ms)
        {
            now = now.AddMilliseconds(ms);
            f.Run(new() { [100] = 4_000_000_000, [101] = 2_000_000_000, [103] = 900_000_000 }, ms / 1000.0);
            probe.Cpu[500] = (77, probe.Cpu[500].cpu + (long)(ms * 10_000 * 0.8));
        }
        var e = new Engine(f) { Now = () => now, Sleep = Advance };
        var watch = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var c = new AutoController(e, probe, new NoFrames(), () => now) { SaveSession = _ => { }, AutostartLine = () => "", WatchList = () => watch };
        c.Click();
        for (int i = 0; i < 12; i++) { Advance(1000); c.Tick(); }
        Assert.Equal(AutoState.Armed, c.State);
        Assert.Contains("watchlist", c.Status);
        Assert.Equal(32u, f.Procs[500].Cls);
        watch.Add("cs2.exe");
        for (int i = 0; i < 12; i++) { Advance(1000); c.Tick(); }
        Assert.Equal(AutoState.Active, c.State);
        Assert.Equal(500u, c.GamePid);
        Assert.Equal(32768u, f.Procs[500].Cls);
        c.Shutdown();
        Assert.Equal(32u, f.Procs[500].Cls);
    }

    [Fact]
    public void BuiltinsAreReplacedByTheShippedTextAndUserCopiesOverrideThem()
    {
        var shipped = new[] { new Preset { Exe = "a.exe", Class = "", Threads = { new ThreadRule { Pattern = "x.dll", Prio = "1" } } }, new Preset { Exe = "b.exe", Class = "" } };
        var presets = new Dictionary<string, Preset>(StringComparer.OrdinalIgnoreCase)
        {
            ["a.exe"] = new Preset { Exe = "a.exe", Builtin = true, Enabled = false, Class = "high" },      // an edited stored built-in
            ["b.exe"] = new Preset { Exe = "b.exe", Class = "below" },                                     // the user's copy
            ["gone.exe"] = new Preset { Exe = "gone.exe", Builtin = true },
        };
        var notes = Store.MarkBuiltin(presets, shipped);
        Assert.Equal(3, notes.Count);
        Assert.Equal("", presets["a.exe"].Class);
        Assert.False(presets["a.exe"].Enabled);              // the switch is the user's
        Assert.True(presets["a.exe"].Builtin);
        Assert.False(presets["b.exe"].Builtin);
        Assert.Equal("below", presets["b.exe"].Class);
        Assert.False(presets.ContainsKey("gone.exe"));
        Assert.Empty(Store.MarkBuiltin(presets, shipped).Where(n => !n.Contains("overrides")));
    }

    [Fact]
    public void ProfilesRoundTrip()
    {
        string dir = Path.Combine(Path.GetTempPath(), "ds-prof-" + Guid.NewGuid().ToString("N"));
        Store.DirHook = () => dir;
        try
        {
            var presets = new Dictionary<string, Preset> { ["x.exe"] = new Preset { Exe = "x.exe", Class = "above", Threads = { new ThreadRule { Match = "desc", Pattern = "Main", Prio = "1", Tid = "first" } } } };
            Store.SaveProfile("one", presets, new());
            Assert.True(Store.CopyProfile("one", "two"));
            Assert.Equal(new[] { "one", "two" }, Store.ListProfiles());
            Store.Load(out var back, out _, Store.ProfilePath("two"));
            Assert.True(Store.Same(presets["x.exe"], back["x.exe"]));
            Assert.True(Store.DeleteProfile("one"));
            Assert.False(Store.DeleteProfile("one"));
        }
        finally
        {
            Store.DirHook = null;
            if (Directory.Exists(dir)) Directory.Delete(dir, true);
        }
    }

    [Fact]
    public void RussianAndEnglishOnlyLoadWithTheirFontsAndFallBackToTheKey()
    {
        Assert.Equal(new[] { "en", "ru" }, L.Available);
        foreach (var code in L.Available)
        {
            L.Load(code);
            Assert.Equal(code, L.Lang);
            Assert.NotEqual("", L.Family);
            Assert.Equal("no such key", L.T("no such key"));
            Assert.Equal("x", L.N("x"));
        }
        L.Load("ru");
        Assert.NotEqual("Turn on", L.T("Turn on"));
        L.Load("zh");                                  // removed in 1.11.0
        Assert.Equal("en", L.Lang);
        Assert.DoesNotContain(typeof(L).Assembly.GetManifestResourceNames(), n => n.EndsWith(".zh.json", StringComparison.OrdinalIgnoreCase));
        L.Load("xx");
        Assert.Equal("en", L.Lang);
        Settings.Set("Language", "ru");
        Assert.Equal("ru", Settings.Language());
        Settings.Set("Language", "klingon");
        Assert.Equal(L.Detect(), Settings.Language());
        Settings.Set("Language", "zh");                // a saved removed language: English, or Russian on a Russian Windows
        Assert.Equal(L.Detect(), Settings.Language());
        Assert.Contains(L.Detect(), new[] { "en", "ru" });
    }
}
