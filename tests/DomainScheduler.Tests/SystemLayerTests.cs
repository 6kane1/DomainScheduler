using Xunit;

namespace DomainScheduler.Tests;

sealed class FakeReserved : IReservedRegistry
{
    public byte[] Value;
    public bool Lie;
    public byte[] Read() => Value;
    public void Write(byte[] v) => Value = Lie ? new byte[8] : (byte[])v.Clone();
    public void Delete() => Value = null;
}

[Collection("static-hooks")]
public class ReservationTests : IDisposable
{
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-res-" + Guid.NewGuid().ToString("N"));
    readonly FakeReserved _reg = new();

    public ReservationTests()
    {
        Directory.CreateDirectory(_dir);
        Reservation.Reg = _reg;
        Reservation.BackupPathHook = () => Path.Combine(_dir, "reserved.bak");
    }

    public void Dispose()
    {
        Reservation.Reg = new WinReservedRegistry();
        Reservation.BackupPathHook = null;
        Reservation.BootUtc = () => DateTime.UtcNow - TimeSpan.FromMilliseconds(Native.GetTickCount64());
        Directory.Delete(_dir, true);
    }

    [Fact]
    public void Refusals()
    {
        var x = Synth.X7950X3D();
        Assert.Empty(Reservation.Refusals(x, 0xFFFF0000));
        Assert.NotEmpty(Reservation.Refusals(x, 0xFFFF0001));   // CPU0
        Assert.NotEmpty(Reservation.Refusals(x, 0x10000));      // half an SMT core, not the whole half
        Assert.NotEmpty(Reservation.Refusals(x, 0xFFFF));       // the game half
        Assert.NotEmpty(Reservation.Refusals(x, 0));
        Assert.NotEmpty(Reservation.Refusals(Synth.X7950X(), 0xFFFF0000));
        Assert.NotEmpty(Reservation.Refusals(Synth.X7800X3D(), 0xFF00));
        Assert.Empty(Reservation.Refusals(Synth.I13900K(), 0xFFFF0000));
        Assert.NotEmpty(Reservation.Refusals(null, 1));
    }

    [Fact]
    public void UnmixAndMixes()
    {
        Assert.Equal(0xFFFFUL, Reservation.Unmix(0xFFFFFFFF, 0xFFFF0000, false));
        Assert.Equal(0xFFFF0000UL, Reservation.Unmix(0xFFFFFFFF, 0xFFFF0000, true));
        Assert.Equal(0xFF0000UL, Reservation.Unmix(0xFF0000, 0xFFFF0000, false));
        Assert.True(Reservation.Mixes(0xFFFFFFFF, 0xFFFF0000));
        Assert.False(Reservation.Mixes(0xFFFF, 0xFFFF0000));
        Assert.False(Reservation.Mixes(0xFF, 0));
    }

    [Fact]
    public void WriteBackupReadBackPendingRevert()
    {
        _reg.Value = new byte[] { 1, 2, 0, 0, 0, 0, 0, 0 };
        Reservation.BootUtc = () => DateTime.UtcNow.AddHours(-1);
        Assert.Null(Reservation.ApplyNonGame(Synth.X7950X3D()));
        Assert.True(File.Exists(Reservation.BackupPath));
        Assert.Equal(BitConverter.GetBytes(0xFFFF0000UL), _reg.Value);
        var st = Reservation.Read();
        Assert.True(st.Pending);
        Assert.True(st.HasBackup);
        Assert.Equal("0102000000000000", st.Original);
        Reservation.BootUtc = () => DateTime.UtcNow.AddSeconds(5);   // rebooted since
        Assert.False(Reservation.Read().Pending);
        Assert.Null(Reservation.Revert());
        Assert.Equal(new byte[] { 1, 2, 0, 0, 0, 0, 0, 0 }, _reg.Value);
        Assert.False(File.Exists(Reservation.BackupPath));
        // A value that does not read back is reported.
        _reg.Lie = true;
        Assert.NotNull(Reservation.ApplyNonGame(Synth.X7950X3D()));
        // No split: refused before anything is written.
        _reg.Lie = false;
        _reg.Value = null;
        Assert.NotNull(Reservation.ApplyNonGame(Synth.X7800X3D()));
        Assert.Null(_reg.Value);
    }
}

[Collection("static-hooks")]
public class DevicesTests : IDisposable
{
    public void Dispose()
    {
        Devices.ListHook = null;
        Devices.ReadHook = null;
    }

    [Fact]
    public void PinnedIrqsTakeNonZeroOverrides()
    {
        Devices.ListHook = () => new() { new PciDevice { Name = "GPU", Pdo = "PCI\\GPU" }, new PciDevice { Name = "NIC", Pdo = "PCI\\NIC" }, new PciDevice { Name = "USB", Pdo = "PCI\\USB" } };
        Devices.ReadHook = id => id switch
        {
            "PCI\\GPU" => new IrqConfig { Assign = 0x10 },
            "PCI\\NIC" => new IrqConfig { Assign = 0 },
            _ => new IrqConfig(),
        };
        var p = Devices.PinnedIrqs();
        Assert.Single(p);
        Assert.Equal("GPU", p[0].Key);
        Assert.Equal(0x10UL, p[0].Value);
    }

    [Fact]
    public void ParseIrqDescriptor()
    {
        var d = new byte[48];
        BitConverter.GetBytes(0xFFFFFFF0u).CopyTo(d, 12);
        BitConverter.GetBytes(0x30UL).CopyTo(d, 16);
        BitConverter.GetBytes(16u).CopyTo(d, 36);
        var r = Devices.ParseIrq(d);
        Assert.Equal(2, r.Count);
        Assert.True(r[0].Message);
        Assert.Equal(0x30UL, r[0].Affinity);
        Assert.False(r[1].Message);
        Assert.Equal(16, r[1].Vector);
        Assert.All(r, x => Assert.Equal(IrqKind.Isr, x.Kind));
    }

    [Fact]
    public void FactsReadRestartFromTheStateFile()
    {
        string dir = Path.Combine(Path.GetTempPath(), "ds-dev-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        Devices.StatePathHook = () => Path.Combine(dir, "state.json");
        Devices.ResourceHook = _ => { var d = new byte[24]; BitConverter.GetBytes(0x80000010u).CopyTo(d, 12); return d; };
        try
        {
            var at = new DateTime(2026, 5, 1, 10, 0, 0, DateTimeKind.Utc);
            File.WriteAllText(Devices.StatePath, System.Text.Json.JsonSerializer.Serialize(new Dictionary<string, DateTime> { ["PCI\\GPU"] = at }));
            var f = Devices.Facts(new PciDevice { Name = "GPU", Pdo = "PCI\\GPU" }, new() { new("GPU", 0x10), new("NIC", 0x20) });
            Assert.Equal(at, f.Restarted);
            Assert.True(f.Resources[0].Message);
            Assert.Single(f.Others);
            Assert.Equal("NIC", f.Others[0].Key);
        }
        finally
        {
            Devices.StatePathHook = null;
            Devices.ResourceHook = null;
            Directory.Delete(dir, true);
        }
    }
}

[Collection("static-hooks")]
public class SettingsStoreTests : IDisposable
{
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-store-" + Guid.NewGuid().ToString("N"));

    public SettingsStoreTests()
    {
        Settings.Backend = new MemorySettingsBackend();
        Store.DirHook = () => _dir;
    }

    public void Dispose()
    {
        Settings.Backend = new RegistrySettingsBackend();
        Store.DirHook = null;
        if (Directory.Exists(_dir)) Directory.Delete(_dir, true);
    }

    [Fact]
    public void WatchList()
    {
        Assert.Equal(new[] { "game.exe" }, Settings.WatchList("game"));
        Assert.Equal(new[] { "game.exe", "cs2.exe" }, Settings.WatchList("game; cs2.exe + GAME.exe,game"));
    }

    [Fact]
    public void StoredAndDefaults()
    {
        Assert.False(Settings.TryStored("X"));
        Settings.Set("X", "");
        Assert.True(Settings.TryStored("X"));
        Assert.False(Settings.Stored("X"));
        Assert.Equal("fb", Settings.Get("X", "fb"));
        Assert.Equal("10000", Settings.Get("ReapplyMs"));
        Assert.Equal(7000, Settings.GetInt("Y", 7000));
        Settings.SetBool("B", true);
        Assert.True(Settings.GetBool("B"));
        Settings.SaveAffRule("cs2.exe", "vcache");
        Assert.Equal("vcache", Settings.LoadAffRules()["cs2.exe"]);
    }

    [Fact]
    public void StoreRoundTrip()
    {
        var presets = new Dictionary<string, Preset>(StringComparer.OrdinalIgnoreCase)
        {
            ["cs2.exe"] = new Preset { Exe = "cs2.exe", Class = "above", Threads = { new ThreadRule { Match = "desc", Pattern = "MainThrd", Ideal = "auto" } } },
        };
        Store.Save(presets, new Dictionary<string, string> { ["discord.exe"] = "below" });
        var notes = Store.Load(out var p2, out var bg);
        Assert.Empty(notes);
        Assert.Equal("above", p2["cs2.exe"].Class);
        Assert.Equal("MainThrd", p2["cs2.exe"].Threads[0].Pattern);
        Assert.Equal("below", bg["discord.exe"]);
        Assert.Equal("a_b_c", Store.Sanitize("a/b:c"));
        Assert.NotEmpty(Store.MarkBuiltin(p2, new[] { new Preset { Exe = "other.exe" } }));
        Assert.False(p2["cs2.exe"].Builtin);
        Assert.True(p2["other.exe"].Builtin);
    }
}
