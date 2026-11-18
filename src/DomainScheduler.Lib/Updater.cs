using System.Net.Http;
using System.Reflection;
using System.Security.Cryptography;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// The version of this build and the update channel (1.15.0). Stable: only the stable releases. Test: the test
/// builds and the stable releases, whichever is newer. The test channel is on while a valid tester key is saved
/// (Settings → Tester); the test builds are encrypted with that key, so without it there is nothing to read.
/// </summary>
public static class AppVersion
{
    static string _full;

    /// <summary>"1.15.0" or "1.15.0-test1.1" (the informational version without the "+commit" tail).</summary>
    public static string Full => _full ??= Read();

    /// <summary>"stable" or "test".</summary>
    public static string Channel => TesterKey.Current() != null ? "test" : "stable";

    public static bool IsTest => Channel == "test";

    /// <summary>What the window shows: "1.15.0" or "1.15.0 Test 1.1".</summary>
    public static string Display => Describe(Full);

    public static string Describe(string v)
    {
        var p = UpdateVersion.Parse(v);
        if (p == null) return v ?? "";
        return p.Value.Test == null ? p.Value.Core.ToString(3) : $"{p.Value.Core.ToString(3)} Test {p.Value.Test}";
    }

    static string Read()
    {
        var a = Assembly.GetEntryAssembly() ?? typeof(AppVersion).Assembly;
        string v = a.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
                   ?? a.GetName().Version?.ToString(3) ?? "0.0.0";
        int plus = v.IndexOf('+');
        return plus >= 0 ? v[..plus] : v;
    }

    /// <summary>For tests.</summary>
    public static void Override(string full) { _full = full; }
}

/// <summary>
/// The tester key (1.15.0 Test 1.1): 32 random bytes shown as "DOST-XXXX-XXXX-…" (base32). Test builds and the test
/// feed are sealed with AES-256-GCM under it; the key is handed out only in the closed testers channel. A wrong or
/// replaced key opens nothing (GCM refuses it), so there is no way to read a test build without the current key.
/// </summary>
public static class TesterKey
{
    const string Abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    static readonly byte[] Magic = "DOSE1"u8.ToArray();

    /// <summary>The saved key (Settings "TesterKey"), null when none or unreadable.</summary>
    public static byte[] Current() => Parse(Settings.Get("TesterKey"));

    public static byte[] Parse(string text)
    {
        if (string.IsNullOrWhiteSpace(text)) return null;
        string t = text.Trim().ToUpperInvariant();
        if (t.StartsWith("DOST")) t = t[4..];
        t = new string(t.Where(c => c != '-' && c != ' ').ToArray());
        int bits = 0, acc = 0; var outp = new List<byte>();
        foreach (char c in t)
        {
            int v = Abc.IndexOf(c);
            if (v < 0) return null;
            acc = (acc << 5) | v; bits += 5;
            if (bits >= 8) { outp.Add((byte)(acc >> (bits - 8))); bits -= 8; acc &= (1 << bits) - 1; }
        }
        return outp.Count == 32 ? outp.ToArray() : null;
    }

    public static string Format(byte[] key)
    {
        var sb = new System.Text.StringBuilder(); int bits = 0, acc = 0;
        foreach (byte b in key)
        {
            acc = (acc << 8) | b; bits += 8;
            while (bits >= 5) { sb.Append(Abc[(acc >> (bits - 5)) & 31]); bits -= 5; }
            acc &= (1 << bits) - 1;
        }
        if (bits > 0) sb.Append(Abc[(acc << (5 - bits)) & 31]);
        string raw = sb.ToString();
        return "DOST-" + string.Join("-", Enumerable.Range(0, (raw.Length + 3) / 4).Select(i => raw.Substring(i * 4, Math.Min(4, raw.Length - i * 4))));
    }

    /// <summary>"DOSE1" | nonce 12 | tag 16 | ciphertext. Throws CryptographicException for a wrong key or a changed file.</summary>
    public static byte[] Open(byte[] sealedData, byte[] key)
    {
        if (sealedData.Length < 33 || !sealedData.AsSpan(0, 5).SequenceEqual(Magic)) throw new InvalidDataException("Not a sealed Domain OS file.");
        var nonce = sealedData.AsSpan(5, 12); var tag = sealedData.AsSpan(17, 16); var ct = sealedData.AsSpan(33);
        var plain = new byte[ct.Length];
        using var gcm = new AesGcm(key, 16);
        gcm.Decrypt(nonce, ct, tag, plain);
        return plain;
    }

    public static byte[] Seal(byte[] plain, byte[] key)
    {
        var nonce = RandomNumberGenerator.GetBytes(12); var tag = new byte[16]; var ct = new byte[plain.Length];
        using var gcm = new AesGcm(key, 16);
        gcm.Encrypt(nonce, plain, ct, tag);
        return Magic.Concat(nonce).Concat(tag).Concat(ct).ToArray();
    }
}

/// <summary>"1.15.0", "1.15.0-test1.0", "1.15.0-test1.1": a stable release is newer than every test build of the same number.</summary>
public static class UpdateVersion
{
    public readonly record struct V(Version Core, string Test, int[] TestParts);

    public static V? Parse(string s)
    {
        if (string.IsNullOrWhiteSpace(s)) return null;
        s = s.Trim().TrimStart('v', 'V');
        int plus = s.IndexOf('+'); if (plus >= 0) s = s[..plus];
        string core = s, test = null;
        int dash = s.IndexOf('-');
        if (dash >= 0)
        {
            core = s[..dash];
            string tail = s[(dash + 1)..];
            if (!tail.StartsWith("test", StringComparison.OrdinalIgnoreCase)) return null;
            test = tail[4..].TrimStart('.', '-');
            if (test.Length == 0) test = "0";
        }
        if (!Version.TryParse(core, out var ver)) return null;
        if (ver.Build < 0) ver = new Version(ver.Major, ver.Minor, 0);
        int[] parts = null;
        if (test != null)
        {
            var bits = test.Split('.');
            parts = new int[bits.Length];
            for (int i = 0; i < bits.Length; i++)
                if (!int.TryParse(bits[i], out parts[i])) return null;
        }
        return new V(new Version(ver.Major, ver.Minor, ver.Build), test, parts);
    }

    /// <summary>&lt;0 when a is older than b, 0 the same, &gt;0 newer. Unreadable versions count as the oldest.</summary>
    public static int Compare(string a, string b)
    {
        var x = Parse(a); var y = Parse(b);
        if (x == null || y == null) return (x == null ? 0 : 1) - (y == null ? 0 : 1);
        int c = x.Value.Core.CompareTo(y.Value.Core);
        if (c != 0) return c;
        if (x.Value.Test == null || y.Value.Test == null) return (x.Value.Test == null ? 1 : 0) - (y.Value.Test == null ? 1 : 0);
        var p = x.Value.TestParts; var q = y.Value.TestParts;
        for (int i = 0; i < Math.Max(p.Length, q.Length); i++)
        {
            int u = i < p.Length ? p[i] : 0, w = i < q.Length ? q[i] : 0;
            if (u != w) return u.CompareTo(w);
        }
        return 0;
    }
}

/// <summary>One file in an update feed: Toolbox or the playbook. Sealed: the file is encrypted with the tester key (Sha256/Size are of the encrypted file).</summary>
public sealed record UpdateFile(string Version, string Url, string Sha256, long Size, string NotesEn, string NotesRu, bool Sealed = false);

/// <summary>update/stable.json or update/test.json on the site.</summary>
public sealed record UpdateFeed(UpdateFile Toolbox, UpdateFile Playbook);

public sealed record UpdateCheck(UpdateFile Toolbox, UpdateFile Playbook, string Error)
{
    public bool Any => Toolbox != null || Playbook != null;
}

/// <summary>
/// The update check (1.15.0). It reads one or two small JSON files from the Domain OS site and nothing else is sent:
/// no ID, no hardware, only a plain User-Agent with the version. Downloads are checked against their SHA-256 and size
/// before anything uses them. Installing Toolbox is done by the app (it has to exit); the playbook is never installed
/// here: its file is downloaded, checked and shown, AME Wizard installs it.
/// </summary>
public static class Updater
{
    public const string FeedBase = "https://6kane1.github.io/DomainOS/update/";

    /// <summary>
    /// The public half of the Domain OS update key (ECDSA P-256). Every feed is signed with the private half, which
    /// exists only on the author's PC; a feed without a valid signature is refused. The feed lists each file's SHA-256,
    /// so the signature covers the downloads too: someone who takes over the site or the GitHub account still cannot
    /// make Toolbox install anything.
    /// </summary>
    public const string PublicKey = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAENxIGlvWT392aI8r24XWshofv30+lyxs2sEMlNVslbKJh1UNZzJEu5Jx1h0EFzmnaNv1fGMBTHHU5mp/rz8gcWg==";

    /// <summary>True when sigBase64 is a valid signature of exactly these bytes (DER ECDSA, SHA-256).</summary>
    public static bool Verify(byte[] data, string sigBase64, string publicKey = PublicKey)
    {
        try
        {
            using var ec = ECDsa.Create();
            ec.ImportSubjectPublicKeyInfo(Convert.FromBase64String(publicKey), out _);
            return ec.VerifyData(data, Convert.FromBase64String((sigBase64 ?? "").Trim()), HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence);
        }
        catch (Exception) { return false; }
    }

    public static string Dir => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "Domain OS", "Updates");

    /// <summary>The playbook version applied last on this PC (history.log written by the playbook), null when none.</summary>
    public static string InstalledPlaybook(string historyFile = null)
    {
        try
        {
            historyFile ??= Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "Domain OS", "history.log");
            if (!File.Exists(historyFile)) return null;
            string last = null;
            foreach (var line in File.ReadAllLines(historyFile))
            {
                var t = line.Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
                if (t.Length >= 3 && t[2] == "applied") last = t.Length >= 4 ? t[3] : null;
                else if (t.Length >= 3 && t[2].StartsWith("undo", StringComparison.OrdinalIgnoreCase)) last = null;
            }
            return last;
        }
        catch (Exception) { return null; }
    }

    public static UpdateFeed ParseFeed(string json)
    {
        using var doc = JsonDocument.Parse(json);
        return new UpdateFeed(File(doc.RootElement, "toolbox"), File(doc.RootElement, "playbook"));

        static UpdateFile File(JsonElement root, string name)
        {
            if (!root.TryGetProperty(name, out var e) || e.ValueKind != JsonValueKind.Object) return null;
            string S(string k) => e.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
            long size = e.TryGetProperty("size", out var z) && z.ValueKind == JsonValueKind.Number ? z.GetInt64() : 0;
            bool sealedFile = e.TryGetProperty("sealed", out var se) && se.ValueKind == JsonValueKind.True;
            var f = new UpdateFile(S("version"), S("url"), S("sha256")?.ToLowerInvariant(), size, S("notes_en") ?? "", S("notes_ru") ?? "", sealedFile);
            bool ok = UpdateVersion.Parse(f.Version) != null && f.Url != null && f.Url.StartsWith("https://", StringComparison.OrdinalIgnoreCase)
                      && f.Sha256 is { Length: 64 } && f.Size > 0;
            return ok ? f : null;
        }
    }

    /// <summary>
    /// What to offer. A stable build looks at the stable feed only; a test build at both, taking the newest Toolbox.
    /// Only versions newer than what runs (Toolbox) or what was applied (playbook) are offered.
    /// </summary>
    public static UpdateCheck Choose(string current, string channel, string playbook, UpdateFeed stable, UpdateFeed test)
    {
        IEnumerable<UpdateFeed> feeds = channel == "test" ? new[] { stable, test } : new[] { stable };
        UpdateFile tb = null, pb = null;
        foreach (var f in feeds.Where(f => f != null))
        {
            if (f.Toolbox != null && UpdateVersion.Compare(f.Toolbox.Version, current) > 0 && (tb == null || UpdateVersion.Compare(f.Toolbox.Version, tb.Version) > 0)) tb = f.Toolbox;
            if (f.Playbook != null && playbook != null && UpdateVersion.Compare(f.Playbook.Version, playbook) > 0 && (pb == null || UpdateVersion.Compare(f.Playbook.Version, pb.Version) > 0)) pb = f.Playbook;
        }
        return new UpdateCheck(tb, pb, null);
    }

    static HttpClient Client(TimeSpan timeout)
    {
        var c = new HttpClient(new HttpClientHandler { AllowAutoRedirect = true }) { Timeout = timeout };
        c.DefaultRequestHeaders.UserAgent.ParseAdd("DomainOS-Toolbox/" + AppVersion.Full.Replace('+', '.'));
        return c;
    }

    public static async Task<UpdateCheck> CheckAsync(CancellationToken ct = default)
    {
        try
        {
            using var http = Client(TimeSpan.FromSeconds(20));
            async Task<UpdateFeed> Get(string name)
            {
                byte[] body = await Fetch(http, name, ct);
                return body == null ? null : ParseFeed(System.Text.Encoding.UTF8.GetString(body));
            }
            var stable = await Get("stable.json");
            UpdateFeed test = null;
            byte[] key = TesterKey.Current();
            if (key != null)
            {
                byte[] box = await Fetch(http, "test.bin", ct);
                if (box != null)
                {
                    try { test = ParseFeed(System.Text.Encoding.UTF8.GetString(TesterKey.Open(box, key))); }
                    catch (CryptographicException) { return new UpdateCheck(null, null, KeyRefused); }
                }
            }
            return Choose(AppVersion.Full, AppVersion.Channel, InstalledPlaybook(), stable, test);
        }
        catch (Exception e) when (e is not OperationCanceledException || !ct.IsCancellationRequested)
        {
            return new UpdateCheck(null, null, e.Message);
        }
    }

    public static string KeyRefused => L.N("The tester key does not open the test builds (it may have been replaced). Take the current key from the testers channel.");

    /// <summary>Checks a key against the published test feed: null when it opens it, otherwise why not.</summary>
    public static async Task<string> TryKeyAsync(byte[] key, CancellationToken ct = default)
    {
        try
        {
            using var http = Client(TimeSpan.FromSeconds(20));
            byte[] box = await Fetch(http, "test.bin", ct);
            if (box == null) return L.N("There are no test builds right now.");
            ParseFeed(System.Text.Encoding.UTF8.GetString(TesterKey.Open(box, key)));
            return null;
        }
        catch (CryptographicException) { return KeyRefused; }
        catch (Exception e) { return e.Message; }
    }

    /// <summary>A feed file and its signature; null when it is not there (404). Throws when the signature is missing or wrong.</summary>
    static async Task<byte[]> Fetch(HttpClient http, string name, CancellationToken ct)
    {
        string q = "?t=" + DateTime.UtcNow.Ticks / TimeSpan.TicksPerMinute;
        byte[] body;
        try { body = await http.GetByteArrayAsync(FeedBase + name + q, ct); }
        catch (HttpRequestException e) when (e.StatusCode == System.Net.HttpStatusCode.NotFound) { return null; }
        string sig;
        try { sig = await http.GetStringAsync(FeedBase + name + ".sig" + q, ct); }
        catch (HttpRequestException e) when (e.StatusCode == System.Net.HttpStatusCode.NotFound) { sig = null; }
        if (!Verify(body, sig)) throw new InvalidDataException("The update file " + name + " has no valid Domain OS signature: it was ignored.");
        return body;
    }

    public static string Sha256(string path)
    {
        using var s = File.OpenRead(path);
        return Convert.ToHexString(SHA256.HashData(s)).ToLowerInvariant();
    }

    /// <summary>Downloads to Dir and checks size and SHA-256. Returns the checked file; a bad file is deleted and an error thrown.</summary>
    public static async Task<string> DownloadAsync(UpdateFile f, string fileName, IProgress<double> progress = null, CancellationToken ct = default)
    {
        Directory.CreateDirectory(Dir);
        string final = Path.Combine(Dir, fileName), part = final + ".part";
        if (!f.Sealed && File.Exists(final) && new FileInfo(final).Length == f.Size && Sha256(final) == f.Sha256) return final;
        using var http = Client(TimeSpan.FromMinutes(30));
        using (var resp = await http.GetAsync(f.Url, HttpCompletionOption.ResponseHeadersRead, ct))
        {
            resp.EnsureSuccessStatusCode();
            await using var src = await resp.Content.ReadAsStreamAsync(ct);
            await using var dst = File.Create(part);
            var buf = new byte[1 << 16]; long got = 0; int n;
            while ((n = await src.ReadAsync(buf, ct)) > 0)
            {
                await dst.WriteAsync(buf.AsMemory(0, n), ct);
                got += n;
                if (got > f.Size) throw new InvalidDataException("The file is larger than the update says.");
                progress?.Report((double)got / f.Size);
            }
        }
        long len = new FileInfo(part).Length;
        string sum = Sha256(part);
        if (len != f.Size || sum != f.Sha256)
        {
            try { File.Delete(part); } catch (Exception) { }
            throw new InvalidDataException($"The downloaded file does not match (size {len}, SHA-256 {sum[..12]}…): it was deleted and nothing was changed.");
        }
        if (File.Exists(final)) File.Delete(final);
        if (f.Sealed)
        {
            byte[] key = TesterKey.Current() ?? throw new InvalidDataException(KeyRefused);
            byte[] plain;
            try { plain = TesterKey.Open(File.ReadAllBytes(part), key); }
            catch (CryptographicException) { throw new InvalidDataException(KeyRefused); }
            finally { try { File.Delete(part); } catch (Exception) { } }
            File.WriteAllBytes(final, plain);
            return final;
        }
        File.Move(part, final);
        return final;
    }

    /// <summary>
    /// Swaps the program file: the running exe is renamed to "&lt;exe&gt;.prev" (Windows allows renaming a running
    /// program) and the checked new file takes its name. On any failure the old file is put back.
    /// </summary>
    public static void Swap(string exe, string newFile)
    {
        string prev = exe + ".prev";
        if (File.Exists(prev)) File.Delete(prev);
        File.Move(exe, prev);
        try { File.Copy(newFile, exe, overwrite: false); }
        catch (Exception)
        {
            try { if (File.Exists(exe)) File.Delete(exe); } catch (Exception) { }
            File.Move(prev, exe);
            throw;
        }
    }

    /// <summary>Puts the previous program file back (after a failed start of the new one).</summary>
    public static void SwapBack(string exe)
    {
        string prev = exe + ".prev";
        if (!File.Exists(prev)) throw new FileNotFoundException("There is no previous version to return to.", prev);
        string bad = exe + ".bad";
        if (File.Exists(bad)) File.Delete(bad);
        if (File.Exists(exe)) File.Move(exe, bad);
        File.Move(prev, exe);
    }
}
