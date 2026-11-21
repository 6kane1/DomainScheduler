using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>
/// Fuzz and property tests (1.16.0, review 20.11.2026: "property-based + fuzz of the state/feed/byte-table parsers").
/// Every parser that reads data from outside (driver tables, the update feed, the tester key, sealed files, files in
/// the data folder) gets thousands of random and mutated inputs. The properties: it never throws anything but the one
/// documented exception, and whatever it accepts satisfies the same rules a hand-written good input does.
/// Seeded: a failure is reproducible; DS_FUZZ_ITER=100000 runs longer.
/// </summary>
[Collection("static-hooks")]
public class FuzzTests : IDisposable
{
    static readonly int N = int.TryParse(Environment.GetEnvironmentVariable("DS_FUZZ_ITER"), out int n) && n > 0 ? n : 3000;
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ds-fuzz-" + Guid.NewGuid().ToString("N"));

    public FuzzTests() => Directory.CreateDirectory(_dir);
    public void Dispose() { try { Directory.Delete(_dir, true); } catch (Exception) { } }

    static byte[] Bytes(Random r, int max)
    {
        var b = new byte[r.Next(0, max)];
        r.NextBytes(b);
        return b;
    }

    static byte[] Mutate(Random r, byte[] good)
    {
        var b = (byte[])good.Clone();
        int flips = r.Next(1, 12);
        for (int i = 0; i < flips && b.Length > 0; i++)
        {
            int at = r.Next(b.Length);
            b[at] = r.Next(4) switch { 0 => 0, 1 => 0xFF, 2 => (byte)(b[at] ^ (1 << r.Next(8))), _ => (byte)r.Next(256) };
        }
        if (r.Next(5) == 0) Array.Resize(ref b, Math.Max(0, b.Length - r.Next(1, 64)));
        return b;
    }

    static string Text(Random r, int max)
    {
        const string alphabet = "{}[]\":,0123456789abcdefABCDEF-+.eE \n\t\\/xyzDOSTАБВ\u0000￿😀";
        var sb = new StringBuilder();
        int len = r.Next(0, max);
        for (int i = 0; i < len; i++) sb.Append(alphabet[r.Next(alphabet.Length)]);
        return sb.ToString();
    }

    // ---- driver byte tables ---------------------------------------------------------------------------------------

    [Fact]
    public void NvidiaClockTableNeverThrowsAndStaysInBounds()
    {
        var r = new Random(1601);
        for (int i = 0; i < N; i++)
        {
            byte[] t = Bytes(r, NvOc.SizeV2 + 200);
            if (t.Length >= 16 && r.Next(2) == 0)
            {
                // a plausible header: the parser gets past the version check and has to cope with the body
                BitConverter.GetBytes(r.Next(3) switch { 0 => NvOc.Version, 1 => NvOc.Version2, _ => NvOc.Version3 }).CopyTo(t, 0);
                BitConverter.GetBytes(r.Next(-2, 20)).CopyTo(t, 8);
                BitConverter.GetBytes(r.Next(-2, 12)).CopyTo(t, 12);
            }
            var clocks = NvOc.Parse(t);
            if (clocks == null) continue;
            Assert.InRange(clocks.Count, 0, 8);
        }
    }

    [Fact]
    public void AmdAdapterTableNeverReadsPastItsEnd()
    {
        var r = new Random(1602);
        for (int i = 0; i < N; i++)
        {
            byte[] info = Bytes(r, AmdOc.AdapterInfoSize * 4 + 50);
            int count = r.Next(-3, 10);
            var list = AmdOc.ParseAdapters(info, count);
            Assert.True(list.Count <= Math.Max(0, count));
            Assert.True(list.Count * AmdOc.AdapterInfoSize <= info.Length);
        }
        Assert.Empty(AmdOc.ParseAdapters(null, 5));
    }

    [Fact]
    public void InterruptAndCpuSetBuffersNeverThrow()
    {
        var r = new Random(1603);
        for (int i = 0; i < N; i++)
        {
            Devices.ParseIrq(Bytes(r, 600));
            TopoBuilder.ParseCpuSets(Bytes(r, 2000));
        }
        Assert.Empty(Devices.ParseIrq(null));
        Assert.Empty(TopoBuilder.ParseCpuSets(null));
    }

    [Fact]
    public void TextParsersNeverThrow()
    {
        var r = new Random(1604);
        for (int i = 0; i < N; i++)
        {
            string s = Text(r, 400);
            IrqAuto.ParseCppc(s);
            IrqAuto.ParseCppc("<cppc>" + s + "</cppc>");
            ThreadIni.Parse(s);
            ThreadIni.Parse("[cs2.exe]\n" + s);
            UpdateVersion.Parse(s);
            UpdateVersion.Compare(s, "1.15.0");
        }
    }

    // ---- update feed ------------------------------------------------------------------------------------------------

    const string GoodFeed = "{\"toolbox\":{\"version\":\"1.16.0\",\"url\":\"https://github.com/6kane1/DomainOS/releases/download/v1/DomainOS-Toolbox-1.16.0.exe\"," +
        "\"sha256\":\"f82487cd4d692fc0a8d63b2b510b2ad27a2b1b06ea718551de45e6634dc0a604\",\"size\":74565547,\"notes_en\":\"x\",\"notes_ru\":\"y\"}," +
        "\"playbook\":{\"version\":\"2.2.4\",\"url\":\"https://github.com/6kane1/DomainOS/releases/download/v2.2.4/Domain-OS-2.2.4.apbx\"," +
        "\"sha256\":\"56c647a50f3bc15370a380ef829f925e4fbf550c112022e2cbe50c2c25cb064b\",\"size\":82336513}}";

    static void AssertValidFile(UpdateFile f)
    {
        if (f == null) return;
        Assert.StartsWith("https://", f.Url, StringComparison.OrdinalIgnoreCase);
        Assert.Equal(64, f.Sha256.Length);
        Assert.True(f.Sha256.All(Uri.IsHexDigit));
        Assert.InRange(f.Size, 1, Updater.MaxDownloadBytes);
        Assert.NotNull(UpdateVersion.Parse(f.Version));
    }

    [Fact]
    public void FeedParserAcceptsOnlyValidFiles()
    {
        Assert.NotNull(Updater.ParseFeed(GoodFeed).Toolbox);
        var r = new Random(1605);
        byte[] good = Encoding.UTF8.GetBytes(GoodFeed);
        int accepted = 0;
        for (int i = 0; i < N; i++)
        {
            string json = r.Next(3) == 0 ? Text(r, 300) : Encoding.UTF8.GetString(Mutate(r, good));
            UpdateFeed f;
            try { f = Updater.ParseFeed(json); }
            catch (JsonException) { continue; }          // the documented failure: the caller treats it as "no feed"
            catch (ArgumentException) { continue; }      // invalid UTF-16 from a mutation: also caught by the caller
            if (f.Toolbox != null || f.Playbook != null) accepted++;
            AssertValidFile(f.Toolbox);
            AssertValidFile(f.Playbook);
        }
        Assert.True(accepted > 0, "the mutations never produced an accepted feed: the test checks nothing");
    }

    [Fact]
    public void FeedFieldsOfWrongTypesAreRefused()
    {
        string[] bad =
        {
            "{\"toolbox\":{\"version\":1,\"url\":\"https://a/b\",\"sha256\":\"" + new string('a', 64) + "\",\"size\":1}}",
            "{\"toolbox\":{\"version\":\"1.0.0\",\"url\":\"http://a/b\",\"sha256\":\"" + new string('a', 64) + "\",\"size\":1}}",
            "{\"toolbox\":{\"version\":\"1.0.0\",\"url\":\"https://a/b\",\"sha256\":\"" + new string('g', 64) + "\",\"size\":1}}",
            "{\"toolbox\":{\"version\":\"1.0.0\",\"url\":\"https://a/b\",\"sha256\":\"" + new string('a', 64) + "\",\"size\":0}}",
            "{\"toolbox\":{\"version\":\"1.0.0\",\"url\":\"https://a/b\",\"sha256\":\"" + new string('a', 64) + "\",\"size\":99999999999999}}",
            "{\"toolbox\":[]}", "{\"toolbox\":\"x\"}", "[]", "1",
        };
        foreach (string json in bad)
        {
            UpdateFeed f;
            try { f = Updater.ParseFeed(json); }
            catch (JsonException) { continue; }
            Assert.Null(f.Toolbox);
        }
    }

    // ---- tester key and sealed files ------------------------------------------------------------------------------

    [Fact]
    public void TesterKeyParsesOnlyWholeKeysAndRoundTrips()
    {
        var r = new Random(1606);
        for (int i = 0; i < N; i++)
        {
            byte[] k = TesterKey.Parse(Text(r, 120));
            Assert.True(k == null || k.Length == 32);
            byte[] key = RandomNumberGenerator.GetBytes(32);
            Assert.Equal(key, TesterKey.Parse(TesterKey.Format(key)));
            Assert.Equal(key, TesterKey.Parse(" " + TesterKey.Format(key).ToLowerInvariant().Replace("-", " ") + " "));
        }
    }

    [Fact]
    public void SealedFilesOpenOnlyUntouchedWithTheRightKey()
    {
        var r = new Random(1607);
        byte[] key = RandomNumberGenerator.GetBytes(32), other = RandomNumberGenerator.GetBytes(32);
        for (int i = 0; i < Math.Min(N, 1500); i++)
        {
            byte[] plain = Bytes(r, 300);
            byte[] box = TesterKey.Seal(plain, key);
            Assert.Equal(plain, TesterKey.Open(box, key));
            Assert.ThrowsAny<CryptographicException>(() => TesterKey.Open(box, other));
            byte[] bad = Mutate(r, box);
            if (bad.AsSpan().SequenceEqual(box)) continue;
            try
            {
                TesterKey.Open(bad, key);
                Assert.Fail("a changed sealed file was opened");
            }
            catch (CryptographicException) { }
            catch (InvalidDataException) { }
        }
    }

    // ---- files in the data folder (what another program could plant) ----------------------------------------------

    [Fact]
    public void GpuProfileFileOfAnyContentLoadsInsideTheLimits()
    {
        var r = new Random(1608);
        string file = Path.Combine(_dir, "gpu-profiles.json");
        var good = new GpuProfiles();
        good.Put("Daily", 100, 1000, 50, 10, -50, GpuOc.Nvidia);
        good.Put("Radeon", 50, 200, 0, -20, -100, GpuOc.Amd);
        good.SetAtStart("Daily");
        good.RecordCrash(good.Find("Daily"), DateTime.UtcNow);
        good.Save(file);
        byte[] seed = File.ReadAllBytes(file);
        for (int i = 0; i < N; i++)
        {
            File.WriteAllBytes(file, r.Next(4) == 0 ? Encoding.UTF8.GetBytes(Text(r, 400)) : Mutate(r, seed));
            var g = GpuProfiles.Load(file);                         // never throws
            Assert.NotNull(g.Profiles);
            Assert.NotNull(g.Crashes);
            Assert.True(g.Crashes.Count <= GpuProfiles.MaxCrashes);
            foreach (var p in g.Profiles)
            {
                Assert.NotNull(GpuProfiles.CleanName(p.Name));
                Assert.InRange(p.VoltPct, 0, 100);
                Assert.InRange(p.PowerPct, -100, 100);
                Assert.InRange(p.VoltMv, -500, 0);                   // an AMD voltage offset is only ever lowered
            }
            Assert.True(g.AtStart.Length == 0 || g.Find(g.AtStart) != null);
            Assert.Equal(g.Profiles.Count, g.Profiles.Select(p => p.Name.ToUpperInvariant()).Distinct().Count());
        }
    }

    [Fact]
    public void ReservedCpuSetOriginalAcceptsOnlyItsFormat()
    {
        var r = new Random(1609);
        for (int i = 0; i < N; i++)
        {
            string s = Text(r, 80);
            if (Reservation.ValidOriginal(s))
                Assert.True(s == "absent" || (s.Length is > 0 and <= 64 && s.Length % 2 == 0 && s.All(Uri.IsHexDigit)), s);
            var dropped = new List<string>();
            string imported = Reservation.ImportBackup(s, dropped);
            Assert.True(imported == null || Reservation.ValidOriginal(imported.Split('|')[0]));
        }
    }

    // ---- the data folder's permissions (planting) -----------------------------------------------------------------

    static readonly string[] Sids = { "S-1-5-18", "S-1-5-32-544", "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
                                      "S-1-5-32-545", "S-1-1-0", "S-1-5-11", "S-1-5-21-1-2-3-1001", "S-1-5-4", "?" };
    static readonly HashSet<string> TrustedSids = new() { "S-1-5-18", "S-1-5-32-544", "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464" };

    [Fact]
    public void FolderAnyoneElseCanWriteIsNeverTrusted()
    {
        var r = new Random(1610);
        int trusted = 0;
        for (int i = 0; i < N * 3; i++)
        {
            string owner = Sids[r.Next(Sids.Length)];
            var aces = new List<StateDir.Ace>();
            int n = r.Next(0, 6);
            bool canonical = r.Next(4) == 0;
            if (canonical) aces.AddRange(StateDir.Canonical());
            for (int k = 0; k < n; k++)
                aces.Add(new StateDir.Ace(Sids[r.Next(Sids.Length)], r.Next(3) switch { 0 => 0x1200A9, 1 => StateDir.WriteMask, _ => r.Next() },
                    r.Next(3) > 0, r.Next(4), r.Next(5) == 0));
            bool present = r.Next(8) > 0, prot = r.Next(2) == 0;
            var t = StateDir.Judge(owner, present, prot, aces);
            bool someoneElseWrites = aces.Any(a => a.Allow && !TrustedSids.Contains(a.Sid) && (a.Mask & StateDir.WriteMask) != 0);
            if (!TrustedSids.Contains(owner) || !present || someoneElseWrites)
                Assert.Equal(StateDir.Trust.Untrusted, t);
            if (t == StateDir.Trust.Trusted)
            {
                trusted++;
                Assert.True(prot);
                Assert.DoesNotContain(aces, a => a.Inherited);
            }
        }
        Assert.Equal(StateDir.Trust.Trusted, StateDir.Judge("S-1-5-32-544", true, true, StateDir.Canonical()));
    }
}
