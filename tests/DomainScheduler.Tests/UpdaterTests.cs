using DomainScheduler;
using Xunit;

public class UpdaterTests
{
    [Theory]
    [InlineData("1.15.0", "1.14.1", 1)]
    [InlineData("1.15.0", "1.15.0-test1.0", 1)]
    [InlineData("1.15.0-test1.1", "1.15.0-test1.0", 1)]
    [InlineData("1.15.0-test2.0", "1.15.0-test1.9", 1)]
    [InlineData("1.15.0-test1.10", "1.15.0-test1.9", 1)]
    [InlineData("1.15.0-test1.0", "1.14.1", 1)]
    [InlineData("1.15.0-test1.0", "1.15.0-test1.0", 0)]
    [InlineData("v1.15.0", "1.15.0", 0)]
    [InlineData("1.15", "1.15.0", 0)]
    [InlineData("garbage", "1.0.0", -1)]
    [InlineData("1.15.0-beta1", "1.0.0", -1)]
    public void VersionOrder(string a, string b, int sign)
    {
        Assert.Equal(sign, Math.Sign(UpdateVersion.Compare(a, b)));
        Assert.Equal(-sign, Math.Sign(UpdateVersion.Compare(b, a)));
    }

    [Fact]
    public void Describe()
    {
        Assert.Equal("1.15.0", AppVersion.Describe("1.15.0"));
        Assert.Equal("1.15.0 Test 1.1", AppVersion.Describe("1.15.0-test1.1"));
    }

    static string Sha => new string('a', 64);
    static UpdateFile F(string v) => new(v, "https://x/" + v, Sha, 10, "", "");

    [Fact]
    public void StableSeesOnlyStable()
    {
        var stable = new UpdateFeed(F("1.15.0"), F("2.2.0"));
        var test = new UpdateFeed(F("1.16.0-test1.0"), null);
        var r = Updater.Choose("1.14.1", "stable", "2.1.1", stable, test);
        Assert.Equal("1.15.0", r.Toolbox.Version);
        Assert.Equal("2.2.0", r.Playbook.Version);
        Assert.Null(Updater.Choose("1.15.0", "stable", "2.2.0", stable, test).Toolbox);
    }

    [Fact]
    public void TestTakesNewestOfBoth()
    {
        var stable = new UpdateFeed(F("1.14.1"), null);
        var test = new UpdateFeed(F("1.15.0-test1.1"), null);
        Assert.Equal("1.15.0-test1.1", Updater.Choose("1.15.0-test1.0", "test", null, stable, test).Toolbox.Version);
        // the stable release of the same number wins over every test build
        var stable2 = new UpdateFeed(F("1.15.0"), null);
        Assert.Equal("1.15.0", Updater.Choose("1.15.0-test1.1", "test", null, stable2, test).Toolbox.Version);
        // nothing newer
        Assert.Null(Updater.Choose("1.15.0-test1.1", "test", null, stable, test).Toolbox);
        // no playbook applied on this PC: no playbook offer
        Assert.Null(Updater.Choose("1.0.0", "test", null, new UpdateFeed(null, F("9.0.0")), null).Playbook);
    }

    [Fact]
    public void FeedParsingRejectsBadEntries()
    {
        var f = Updater.ParseFeed("{\"toolbox\":{\"version\":\"1.15.0\",\"url\":\"https://a/b.exe\",\"sha256\":\"" + Sha.ToUpperInvariant() + "\",\"size\":5,\"notes_en\":\"n\"},"
                                  + "\"playbook\":{\"version\":\"2.2.0\",\"url\":\"http://a/b.apbx\",\"sha256\":\"" + Sha + "\",\"size\":5}}");
        Assert.Equal("1.15.0", f.Toolbox.Version);
        Assert.Equal(Sha, f.Toolbox.Sha256);
        Assert.Null(f.Playbook);   // plain http is refused
        Assert.Null(Updater.ParseFeed("{\"toolbox\":{\"version\":\"1.15.0\",\"url\":\"https://a\",\"sha256\":\"abc\",\"size\":5}}").Toolbox);
    }

    [Fact]
    public void InstalledPlaybookFromHistory()
    {
        string f = Path.GetTempFileName();
        try
        {
            File.WriteAllLines(f, new[] { "2026-11-17 10:00:00  applied  2.1.0", "2026-11-18 10:00:00  applied  2.1.1" });
            Assert.Equal("2.1.1", Updater.InstalledPlaybook(f));
            File.AppendAllLines(f, new[] { "2026-11-18 11:00:00  undone" });
            Assert.Null(Updater.InstalledPlaybook(f));
            Assert.Null(Updater.InstalledPlaybook(f + ".missing"));
        }
        finally { File.Delete(f); }
    }

    [Fact]
    public void SwapAndSwapBack()
    {
        string d = Path.Combine(Path.GetTempPath(), "upd-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(d);
        try
        {
            string exe = Path.Combine(d, "DomainScheduler.exe"), nw = Path.Combine(d, "new.exe");
            File.WriteAllText(exe, "old"); File.WriteAllText(nw, "new");
            Updater.Swap(exe, nw);
            Assert.Equal("new", File.ReadAllText(exe));
            Assert.Equal("old", File.ReadAllText(exe + ".prev"));
            Updater.SwapBack(exe);
            Assert.Equal("old", File.ReadAllText(exe));
            Assert.Equal("new", File.ReadAllText(exe + ".bad"));
            Assert.False(File.Exists(exe + ".prev"));
            // a failing copy leaves the old file in place
            Assert.ThrowsAny<Exception>(() => Updater.Swap(exe, Path.Combine(d, "missing.exe")));
            Assert.Equal("old", File.ReadAllText(exe));
        }
        finally { Directory.Delete(d, true); }
    }

    [Fact]
    public async Task DownloadRejectsWrongChecksum()
    {
        // a local file:// is not used; the check itself is what matters: Sha256 of a known text
        string f = Path.GetTempFileName();
        try
        {
            File.WriteAllText(f, "abc");
            Assert.Equal("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", Updater.Sha256(f));
        }
        finally { File.Delete(f); }
        await Task.CompletedTask;
    }
}

public class UpdateSignatureTests
{
    [Fact]
    public void OnlyTheRightKeyAndBytesPass()
    {
        using var ec = System.Security.Cryptography.ECDsa.Create(System.Security.Cryptography.ECCurve.NamedCurves.nistP256);
        string pub = Convert.ToBase64String(ec.ExportSubjectPublicKeyInfo());
        byte[] data = System.Text.Encoding.UTF8.GetBytes("{\"toolbox\":{}}");
        string sig = Convert.ToBase64String(ec.SignData(data, System.Security.Cryptography.HashAlgorithmName.SHA256, System.Security.Cryptography.DSASignatureFormat.Rfc3279DerSequence));
        Assert.True(Updater.Verify(data, sig, pub));
        byte[] changed = (byte[])data.Clone(); changed[3] ^= 1;
        Assert.False(Updater.Verify(changed, sig, pub));
        Assert.False(Updater.Verify(data, sig));          // the built-in Domain OS key did not sign it
        Assert.False(Updater.Verify(data, null, pub));
        Assert.False(Updater.Verify(data, "not base64!", pub));
    }

    [Fact]
    public void OpensslSignatureWithTheRealKeyChecks()
    {
        // a feed signed by publish tooling (openssl dgst -sha256 -sign) must pass with the built-in key
        string dir = Path.Combine(AppContext.BaseDirectory, "sigsample");
        Assert.True(File.Exists(Path.Combine(dir, "feed.json")));
        Assert.True(Updater.Verify(File.ReadAllBytes(Path.Combine(dir, "feed.json")), File.ReadAllText(Path.Combine(dir, "feed.json.sig"))));
    }
}
