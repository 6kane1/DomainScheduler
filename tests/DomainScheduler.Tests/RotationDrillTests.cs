using System.Security.Cryptography;
using System.Text;
using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>
/// The key-leak procedure as a drill (review 20.11.2026: "rehearse the rotation, do not only write it down").
/// Phases: (1) clients of today trust {main, backup}; (2) the main key leaks and an attacker signs a feed with it;
/// (3) the author ships the rotation feed signed with the BACKUP key: its Toolbox trusts {old backup as main, a new
/// backup} and revokes the leaked main key; (4) after updating, the attacker's feed is refused, the new keys work.
/// With DS_DRILL_DIR set (tools/key-rotation-drill.sh) the keys are made by openssl and the feeds are signed by the real
/// sign-feed.sh; otherwise the same drill runs with keys made here.
/// </summary>
public class RotationDrillTests
{
    sealed class Drill
    {
        public string Main, Backup, Next;                    // public keys, base64 SPKI DER
        public Dictionary<string, (byte[] Data, string Sig)> Feeds = new();
    }

    static Drill Load()
    {
        string dir = Environment.GetEnvironmentVariable("DS_DRILL_DIR");
        if (!string.IsNullOrEmpty(dir))
        {
            var d = new Drill
            {
                Main = File.ReadAllText(Path.Combine(dir, "main.pub.b64")).Trim(),
                Backup = File.ReadAllText(Path.Combine(dir, "backup.pub.b64")).Trim(),
                Next = File.ReadAllText(Path.Combine(dir, "next.pub.b64")).Trim(),
            };
            foreach (string f in new[] { "feed1", "attack", "rotation", "after" })
                d.Feeds[f] = (File.ReadAllBytes(Path.Combine(dir, f + ".json")), File.ReadAllText(Path.Combine(dir, f + ".json.sig")).Trim());
            return d;
        }
        using var main = ECDsa.Create(ECCurve.NamedCurves.nistP256);
        using var backup = ECDsa.Create(ECCurve.NamedCurves.nistP256);
        using var next = ECDsa.Create(ECCurve.NamedCurves.nistP256);
        string Pub(ECDsa k) => Convert.ToBase64String(k.ExportSubjectPublicKeyInfo());
        (byte[], string) Signed(ECDsa k, string json)
        {
            byte[] b = Encoding.UTF8.GetBytes(json);
            return (b, Convert.ToBase64String(k.SignData(b, HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence)));
        }
        var dd = new Drill { Main = Pub(main), Backup = Pub(backup), Next = Pub(next) };
        dd.Feeds["feed1"] = Signed(main, "{\"phase\":1}");
        dd.Feeds["attack"] = Signed(main, "{\"phase\":\"attacker\"}");
        dd.Feeds["rotation"] = Signed(backup, "{\"phase\":3}");
        dd.Feeds["after"] = Signed(next, "{\"phase\":4}");
        return dd;
    }

    [Fact]
    public void LeakedMainKeyIsShutOutByTheBackupSignedRotation()
    {
        var d = Load();
        bool Ok(IEnumerable<Updater.TrustedKey> keys, IEnumerable<string> revoked, string feed) =>
            Updater.VerifyAny(d.Feeds[feed].Data, d.Feeds[feed].Sig, keys, revoked) != null;

        // (1) today's clients
        var v1 = new[] { new Updater.TrustedKey(Updater.KeyId(d.Main), "main", d.Main), new Updater.TrustedKey(Updater.KeyId(d.Backup), "backup", d.Backup) };
        Assert.True(Ok(v1, Array.Empty<string>(), "feed1"));
        // (2) the documented window: until clients update, a feed signed with the leaked main key is still accepted
        Assert.True(Ok(v1, Array.Empty<string>(), "attack"));
        // (3) the rotation feed, signed with the backup key, is accepted by today's clients: they can update
        Assert.True(Ok(v1, Array.Empty<string>(), "rotation"));
        // (4) the rotated client: old backup is main, a new backup, the leaked key revoked by its fingerprint
        var v2 = new[] { new Updater.TrustedKey(Updater.KeyId(d.Backup), "main", d.Backup), new Updater.TrustedKey(Updater.KeyId(d.Next), "backup", d.Next) };
        var revoked = new[] { Updater.KeyId(d.Main) };
        Assert.False(Ok(v2, revoked, "attack"));
        Assert.False(Ok(v2, revoked, "feed1"));
        Assert.True(Ok(v2, revoked, "rotation"));
        Assert.True(Ok(v2, revoked, "after"));
        // the leaked key smuggled back into the list under any name stays refused
        var smuggled = v2.Append(new Updater.TrustedKey("0000000000000000", "main", d.Main)).ToArray();
        Assert.False(Ok(smuggled, revoked, "attack"));
    }
}
