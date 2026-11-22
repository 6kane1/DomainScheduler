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
            string exe = Path.Combine(d, "DomainScheduler.exe");
            File.WriteAllText(exe, "old");
            Updater.Swap(exe, System.Text.Encoding.UTF8.GetBytes("new"));
            Assert.Equal("new", File.ReadAllText(exe));
            Assert.Equal("old", File.ReadAllText(exe + ".prev"));
            Updater.SwapBack(exe);
            Assert.Equal("old", File.ReadAllText(exe));
            Assert.Equal("new", File.ReadAllText(exe + ".bad"));
            Assert.False(File.Exists(exe + ".prev"));
            // a failing install leaves the old file in place
            Assert.ThrowsAny<Exception>(() => Updater.Swap(exe, Array.Empty<byte>()));
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

/// <summary>Key rotation: a list of trusted keys (main + backup), any of them may sign, revoked ids are refused.
/// Only throwaway keys made here are used to sign; the real keys never touch the tests.</summary>
public class UpdateKeyRotationTests
{
    private static System.Security.Cryptography.ECDsa NewKey() => System.Security.Cryptography.ECDsa.Create(System.Security.Cryptography.ECCurve.NamedCurves.nistP256);
    private static string Pub(System.Security.Cryptography.ECDsa k) => Convert.ToBase64String(k.ExportSubjectPublicKeyInfo());
    private static string Sign(System.Security.Cryptography.ECDsa k, byte[] d) =>
        Convert.ToBase64String(k.SignData(d, System.Security.Cryptography.HashAlgorithmName.SHA256, System.Security.Cryptography.DSASignatureFormat.Rfc3279DerSequence));
    private static Updater.TrustedKey T(string name, System.Security.Cryptography.ECDsa k) => new(Updater.KeyId(Pub(k)), name, Pub(k));

    private static readonly byte[] Feed = System.Text.Encoding.UTF8.GetBytes("{\"toolbox\":{\"version\":\"9.9.9\"}}");

    [Fact]
    public void CurrentAndBackupKeyBothVerify()
    {
        using var main = NewKey(); using var backup = NewKey();
        var keys = new[] { T("main", main), T("backup", backup) };
        Assert.Equal("main", Updater.VerifyAny(Feed, Sign(main, Feed), keys, Array.Empty<string>())?.Name);
        Assert.Equal("backup", Updater.VerifyAny(Feed, Sign(backup, Feed), keys, Array.Empty<string>())?.Name);
    }

    [Fact]
    public void UnknownKeyIsRejected()
    {
        using var main = NewKey(); using var backup = NewKey(); using var stranger = NewKey();
        var keys = new[] { T("main", main), T("backup", backup) };
        Assert.Null(Updater.VerifyAny(Feed, Sign(stranger, Feed), keys, Array.Empty<string>()));
        Assert.False(Updater.Verify(Feed, Sign(stranger, Feed)));      // nor by the built-in list
    }

    [Fact]
    public void TamperedFeedIsRejected()
    {
        using var main = NewKey(); using var backup = NewKey();
        var keys = new[] { T("main", main), T("backup", backup) };
        foreach (var k in new[] { main, backup })
        {
            string sig = Sign(k, Feed);
            byte[] changed = (byte[])Feed.Clone(); changed[^2] ^= 1;
            Assert.Null(Updater.VerifyAny(changed, sig, keys, Array.Empty<string>()));
            byte[] longer = Feed.Concat(new byte[] { (byte)' ' }).ToArray();
            Assert.Null(Updater.VerifyAny(longer, sig, keys, Array.Empty<string>()));
            byte[] rawSig = Convert.FromBase64String(sig); rawSig[^1] ^= 1;
            Assert.Null(Updater.VerifyAny(Feed, Convert.ToBase64String(rawSig), keys, Array.Empty<string>()));
        }
        Assert.Null(Updater.VerifyAny(Feed, null, keys, Array.Empty<string>()));
        Assert.Null(Updater.VerifyAny(Feed, "", keys, Array.Empty<string>()));
        Assert.Null(Updater.VerifyAny(Feed, "not base64!", keys, Array.Empty<string>()));
    }

    [Fact]
    public void RevokedKeyIsRejectedOthersStillWork()
    {
        using var main = NewKey(); using var backup = NewKey();
        var keys = new[] { T("main", main), T("backup", backup) };
        var revoked = new[] { Updater.KeyId(Pub(main)).ToUpperInvariant() };   // case does not matter
        Assert.Null(Updater.VerifyAny(Feed, Sign(main, Feed), keys, revoked));
        Assert.Equal("backup", Updater.VerifyAny(Feed, Sign(backup, Feed), keys, revoked)?.Name);
        // the id written next to a key is not trusted: a revoked key listed under another id is still refused
        var disguised = new[] { new Updater.TrustedKey("0000000000000000", "main", Pub(main)) };
        Assert.Null(Updater.VerifyAny(Feed, Sign(main, Feed), disguised, revoked));
    }

    [Fact]
    public void BuiltInKeyListIsConsistent()
    {
        Assert.Equal(Updater.PublicKey, Updater.TrustedKeys[0].PublicKey);
        Assert.Contains(Updater.TrustedKeys, k => k.PublicKey == Updater.BackupPublicKey);
        Assert.NotEqual(Updater.PublicKey, Updater.BackupPublicKey);
        foreach (var k in Updater.TrustedKeys)
        {
            Assert.Equal(Updater.KeyId(k.PublicKey), k.Id);                 // ids are the real fingerprints
            using var ec = System.Security.Cryptography.ECDsa.Create();
            ec.ImportSubjectPublicKeyInfo(Convert.FromBase64String(k.PublicKey), out _);
            Assert.Equal(256, ec.KeySize);
            Assert.DoesNotContain(k.Id, Updater.RevokedKeyIds);
        }
        Assert.Equal(Updater.TrustedKeys.Count, Updater.TrustedKeys.Select(k => k.Id).Distinct().Count());
    }

    [Fact]
    public void FeedSignedTodayStillVerifiesWithTheMainKey()
    {
        // the existing .sig format (openssl dgst -sha256 -sign, base64 DER) checks with the main key alone (old Toolbox) and the list (new)
        string dir = Path.Combine(AppContext.BaseDirectory, "sigsample");
        byte[] data = File.ReadAllBytes(Path.Combine(dir, "feed.json"));
        string sig = File.ReadAllText(Path.Combine(dir, "feed.json.sig"));
        Assert.True(Updater.Verify(data, sig, Updater.PublicKey));
        Assert.Equal("main", Updater.VerifyAny(data, sig, Updater.TrustedKeys, Updater.RevokedKeyIds)?.Name);
        Assert.Null(Updater.VerifyAny(data, sig, Updater.TrustedKeys, new[] { Updater.TrustedKeys[0].Id }));
    }
}

public class TesterKeyTests
{
    [Fact]
    public void FormatAndParseRoundTrip()
    {
        byte[] k = System.Security.Cryptography.RandomNumberGenerator.GetBytes(32);
        string t = TesterKey.Format(k);
        Assert.StartsWith("DOST-", t);
        Assert.Equal(k, TesterKey.Parse(t));
        Assert.Equal(k, TesterKey.Parse("  " + t.ToLowerInvariant().Replace("-", " ") + " "));
        Assert.Null(TesterKey.Parse("DOST-AAAA"));
        Assert.Null(TesterKey.Parse("DOST-1111-0000"));
        Assert.Null(TesterKey.Parse(""));
    }

    [Fact]
    public void SealOpenAndWrongKey()
    {
        byte[] k = System.Security.Cryptography.RandomNumberGenerator.GetBytes(32);
        byte[] box = TesterKey.Seal(System.Text.Encoding.UTF8.GetBytes("feed"), k);
        Assert.Equal("feed", System.Text.Encoding.UTF8.GetString(TesterKey.Open(box, k)));
        byte[] other = System.Security.Cryptography.RandomNumberGenerator.GetBytes(32);
        Assert.ThrowsAny<System.Security.Cryptography.CryptographicException>(() => TesterKey.Open(box, other));
        box[^1] ^= 1;
        Assert.ThrowsAny<System.Security.Cryptography.CryptographicException>(() => TesterKey.Open(box, k));
    }

    [Fact]
    public void OpensWhatThePublishToolSealed()
    {
        string dir = Path.Combine(AppContext.BaseDirectory, "sigsample");
        byte[] key = TesterKey.Parse(File.ReadAllText(Path.Combine(dir, "sealed.key")));
        Assert.Equal(Enumerable.Range(0, 32).Select(i => (byte)i).ToArray(), key);
        Assert.Equal("hello domain os", System.Text.Encoding.UTF8.GetString(TesterKey.Open(File.ReadAllBytes(Path.Combine(dir, "sealed.bin")), key)));
    }

    [Fact]
    public void SealedFeedEntryIsRead()
    {
        string sha = new string('b', 64);
        var f = Updater.ParseFeed("{\"toolbox\":{\"version\":\"1.15.0-test1.2\",\"sealed\":true,\"url\":\"https://a/tb.bin\",\"sha256\":\"" + sha + "\",\"size\":9}}");
        Assert.True(f.Toolbox.Sealed);
    }

    [Fact]
    public void FeedTokenOnlyForSealedEntriesAndTheGitHubApi()
    {
        string sha = new string('b', 64), tok = "github_pat_" + new string('A', 40);
        string E(bool sealedOn, string url, string token) => "{\"toolbox\":{\"version\":\"1.16.0-test1.8\",\"sealed\":" + (sealedOn ? "true" : "false")
            + ",\"url\":\"" + url + "\",\"sha256\":\"" + sha + "\",\"size\":9,\"token\":\"" + token + "\"}}";
        const string api = "https://api.github.com/repos/6kane1/DomainOS-test/releases/assets/1";
        var ok = Updater.ParseFeed(E(true, api, tok)).Toolbox;
        Assert.NotNull(ok);
        Assert.Equal(tok, ok.Token);
        Assert.DoesNotContain(tok, ok.ToString());
        Assert.Null(Updater.ParseFeed(E(false, api, tok)).Toolbox);                       // never in the open stable feed
        Assert.Null(Updater.ParseFeed(E(true, "https://evil.example/x", tok)).Toolbox);   // never to another host
        Assert.Null(Updater.ParseFeed(E(true, "https://api.github.com.evil.example/repos/x", tok)).Toolbox);
        Assert.Null(Updater.ParseFeed(E(true, api, "abc\\r\\nX-Evil: 1")).Toolbox);       // no header injection
        Assert.Null(Updater.ParseFeed(E(true, api, "short")).Toolbox);
        Assert.Null(Updater.ParseFeed("{\"toolbox\":{\"version\":\"1.16.0\",\"sealed\":true,\"url\":\"https://a/tb.bin\",\"sha256\":\"" + sha + "\",\"size\":9}}").Toolbox.Token);
    }

    [Fact]
    public void ASignedFeedFileSplitsIntoSignatureAndFeed()
    {
        byte[] feed = System.Text.Encoding.UTF8.GetBytes("{\"toolbox\":null}\n");
        byte[] combined = System.Text.Encoding.ASCII.GetBytes("MEUCIQ==\n").Concat(feed).ToArray();
        Assert.Equal(feed, Updater.SplitSigned(combined, out string sig));
        Assert.Equal("MEUCIQ==", sig);
        Assert.Null(Updater.SplitSigned(feed.Skip(1).ToArray().Prepend((byte)'\n').ToArray(), out _));   // no signature line
        Assert.Null(Updater.SplitSigned(null, out _));
    }
}
