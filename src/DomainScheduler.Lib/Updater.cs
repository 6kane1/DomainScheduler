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

    /// <summary>
    /// "stable" or "test": test while a tester key is saved, unless the tester chose "stable releases only" (1.16.0:
    /// Settings "TestBuilds" = false keeps the key but asks for stable releases only).
    /// </summary>
    public static string Channel => ChannelFor(TesterKey.Current() != null, Settings.GetBool("TestBuilds", fallback: true));

    public static string ChannelFor(bool hasKey, bool wantsTest) => hasKey && wantsTest ? "test" : "stable";

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

/// <summary>
/// One file in an update feed: Toolbox or the playbook. Sealed: the file is encrypted with the tester key (Sha256/Size
/// are of the encrypted file). Token (1.16.0 Test 1.8): test builds live in a private repository that only the owner
/// and the testers can see; the sealed test feed carries a read-only GitHub token for that one repository, and the
/// download asks the GitHub API with it. Only a sealed entry may carry a token, and only for an api.github.com URL.
/// </summary>
public sealed record UpdateFile(string Version, string Url, string Sha256, long Size, string NotesEn, string NotesRu, bool Sealed = false, string Token = null)
{
    /// <summary>Never prints the token (logs, exceptions, debugger).</summary>
    public override string ToString() => $"{Version} {Url}";
}

/// <summary>update/stable.json or update/test.json on the site.</summary>
public sealed record UpdateFeed(UpdateFile Toolbox, UpdateFile Playbook);

public sealed record UpdateCheck(UpdateFile Toolbox, UpdateFile Playbook, string Error)
{
    public bool Any => Toolbox != null || Playbook != null;
}

/// <summary>
/// The update check (1.15.0). It reads one or two small JSON files from the Domain OS site and nothing else is sent:
/// no ID, no hardware, only a plain User-Agent with the version. Feeds are at most 1 MB and must carry a valid
/// signature; a version must look like "1.15.0" or "1.15.0-test1.3" before it is used anywhere.
/// Downloads (1.15.0 Test 1.3) go into memory and are checked there — size and SHA-256 of exactly the bytes that are
/// then installed (a sealed test build is opened in memory too). Toolbox's new program file is written once, straight
/// from those bytes, as the program file (nothing is read back from a folder another account could write to). A
/// playbook is written once into a new folder with a random name inside the admin-only state folder (FileMode.CreateNew:
/// an existing file is never reused). Installing Toolbox is done by the app (it has to exit); the playbook is never
/// installed here: its file is downloaded, checked and shown, AME Wizard installs it.
/// </summary>
public static class Updater
{
    public const string FeedBase = "https://6kane1.github.io/DomainOS/update/";
    /// <summary>The only place a feed token is ever sent to (a release asset of the private test repository).</summary>
    public const string GitHubApi = "https://api.github.com/repos/";

    /// <summary>
    /// The public half of the main Domain OS update key (ECDSA P-256). Every feed is signed with a private half, which
    /// exists only with the author (the main key on the author's PC, the backup key offline); a feed without a valid
    /// signature is refused. The feed lists each file's SHA-256, so the signature covers the downloads too: someone who
    /// takes over the site or the GitHub account still cannot make Toolbox install anything.
    /// </summary>
    public const string PublicKey = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAENxIGlvWT392aI8r24XWshofv30+lyxs2sEMlNVslbKJh1UNZzJEu5Jx1h0EFzmnaNv1fGMBTHHU5mp/rz8gcWg==";

    /// <summary>
    /// The public half of the backup update key (ECDSA P-256, made 2026-11-19). Its private half is kept offline and is
    /// used only when the main key leaks or is lost: the next feed is signed with it, and the Toolbox in that feed adds a
    /// new backup key and moves the old main key to <see cref="RevokedKeyIds"/> (rotation steps: README, "Update keys").
    /// </summary>
    public const string BackupPublicKey = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEqHD2RMemyHHX5CUG0TWCJfJEIkYzQClvrLb8rmC/BIO/oPJKJsxCoP5Dd4HwUAfdaj9jDeQIQcHPwUN1Qh84lw==";

    /// <summary>One trusted update key: Id (see <see cref="KeyId"/>), a short name for the log, the public key (base64 SPKI DER).</summary>
    public sealed record TrustedKey(string Id, string Name, string PublicKey);

    /// <summary>
    /// Every key a feed may be signed with: a feed is valid when its signature checks with ANY of them. The .sig file
    /// stays what it always was (base64 DER ECDSA over SHA-256, no key id in it), so feeds signed for older Toolbox
    /// versions, which know only the main key, keep working, and the new code just tries each key in turn.
    /// </summary>
    public static readonly IReadOnlyList<TrustedKey> TrustedKeys = new[]
    {
        new TrustedKey("824da223f01c0905", "main", PublicKey),
        new TrustedKey("a807a74083942dde", "backup", BackupPublicKey),
    };

    /// <summary>
    /// Ids of keys that must never be trusted again (a leaked key goes here in the next Toolbox). Checked by the key's
    /// own fingerprint, so a revoked key is refused even if it is still (or again) listed in <see cref="TrustedKeys"/>.
    /// </summary>
    public static readonly IReadOnlyCollection<string> RevokedKeyIds = Array.Empty<string>();

    /// <summary>Key id: the first 16 hex digits (lower case) of SHA-256 over the public key's SPKI DER bytes; null for bad base64.</summary>
    public static string KeyId(string publicKeyBase64)
    {
        try { return Convert.ToHexString(SHA256.HashData(Convert.FromBase64String(publicKeyBase64 ?? ""))).Substring(0, 16).ToLowerInvariant(); }
        catch (FormatException) { return null; }
    }

    /// <summary>True when sigBase64 is a valid signature of exactly these bytes by one of the built-in trusted, not revoked keys.</summary>
    public static bool Verify(byte[] data, string sigBase64) => VerifyAny(data, sigBase64, TrustedKeys, RevokedKeyIds) != null;

    /// <summary>
    /// The trusted key that signed these bytes, or null when none did. A key whose fingerprint is in revoked is skipped
    /// (the id listed next to a key is not trusted: the fingerprint is computed from the key itself).
    /// </summary>
    public static TrustedKey VerifyAny(byte[] data, string sigBase64, IEnumerable<TrustedKey> keys, IEnumerable<string> revoked)
    {
        if (data == null || string.IsNullOrWhiteSpace(sigBase64) || keys == null) return null;
        var bad = new HashSet<string>(revoked ?? Array.Empty<string>(), StringComparer.OrdinalIgnoreCase);
        foreach (var k in keys)
        {
            string id = k == null ? null : KeyId(k.PublicKey);
            if (id == null || bad.Contains(id)) continue;
            if (Verify(data, sigBase64, k.PublicKey)) return k;
        }
        return null;
    }

    /// <summary>True when sigBase64 is a valid signature of exactly these bytes with this one public key (DER ECDSA, SHA-256).</summary>
    public static bool Verify(byte[] data, string sigBase64, string publicKey)
    {
        try
        {
            using var ec = ECDsa.Create();
            ec.ImportSubjectPublicKeyInfo(Convert.FromBase64String(publicKey), out _);
            return ec.VerifyData(data, Convert.FromBase64String((sigBase64 ?? "").Trim()), HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence);
        }
        catch (Exception) { return false; }
    }

    /// <summary>Downloads: a folder in the admin-only state folder.</summary>
    public static string Dir => Path.Combine(Store.Dir, "Updates");

    /// <summary>"1.15.0" or "1.15.0-test1.3": the only version texts used (in names, in the log, in comparisons).</summary>
    public static readonly System.Text.RegularExpressions.Regex VersionRx = new(@"^\d+\.\d+\.\d+(-test\d+(\.\d+)*)?$", System.Text.RegularExpressions.RegexOptions.CultureInvariant);

    /// <summary>A feed or its signature larger than this is refused.</summary>
    public const long MaxFeedBytes = 1 << 20;
    /// <summary>A download larger than this is refused (Toolbox is far smaller).</summary>
    public const long MaxDownloadBytes = 512L << 20;

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
        // A feed that is not a JSON object (a number, a list …) offers nothing; found by the fuzz tests (1.16.0).
        if (doc.RootElement.ValueKind != JsonValueKind.Object) return new UpdateFeed(null, null);
        return new UpdateFeed(File(doc.RootElement, "toolbox"), File(doc.RootElement, "playbook"));

        static UpdateFile File(JsonElement root, string name)
        {
            if (!root.TryGetProperty(name, out var e) || e.ValueKind != JsonValueKind.Object) return null;
            string S(string k) => e.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
            // TryGetInt64: a fractional or huge size ("1.5", "1e30") is a bad entry, not an exception (fuzz tests, 1.16.0)
            long size = e.TryGetProperty("size", out var z) && z.ValueKind == JsonValueKind.Number && z.TryGetInt64(out long zl) ? zl : 0;
            bool sealedFile = e.TryGetProperty("sealed", out var se) && se.ValueKind == JsonValueKind.True;
            string token = S("token");
            var f = new UpdateFile(S("version"), S("url"), S("sha256")?.ToLowerInvariant(), size, S("notes_en") ?? "", S("notes_ru") ?? "", sealedFile, token);
            // A token goes only with a sealed (test) entry and only to the GitHub API; a token in the open stable feed, a
            // token for another host or a token with odd characters makes the whole entry bad.
            bool tokenOk = token == null || (sealedFile && token.Length is > 10 and < 512 && token.All(c => char.IsAsciiLetterOrDigit(c) || c == '_')
                                             && f.Url != null && f.Url.StartsWith(GitHubApi, StringComparison.OrdinalIgnoreCase));
            bool ok = tokenOk && f.Version != null && VersionRx.IsMatch(f.Version) && UpdateVersion.Parse(f.Version) != null
                      && f.Url != null && f.Url.StartsWith("https://", StringComparison.OrdinalIgnoreCase)
                      && f.Sha256 is { Length: 64 } && f.Sha256.All(Uri.IsHexDigit) && f.Size > 0 && f.Size <= MaxDownloadBytes;
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
        // MaxResponseContentBufferSize caps every buffered read (feeds, signatures); downloads are streamed and capped by their size.
        var c = new HttpClient(new HttpClientHandler { AllowAutoRedirect = true }) { Timeout = timeout, MaxResponseContentBufferSize = MaxFeedBytes };
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
            if (key != null && AppVersion.Channel == "test")
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

    public static string Sha256(byte[] data) => Convert.ToHexString(SHA256.HashData(data)).ToLowerInvariant();

    /// <summary>
    /// Downloads into memory and checks size and SHA-256 of those bytes; a sealed file is opened with the tester key,
    /// also in memory. Returns the checked (and opened) bytes — exactly what the caller writes. Throws on any mismatch.
    /// Progress is reported only when the whole percent changes.
    /// </summary>
    public static async Task<byte[]> DownloadBytesAsync(UpdateFile f, IProgress<double> progress = null, CancellationToken ct = default)
    {
        if (f == null || f.Size <= 0 || f.Size > MaxDownloadBytes) throw new InvalidDataException("The update entry has no valid size.");
        using var http = Client(TimeSpan.FromMinutes(30));
        byte[] buf = new byte[f.Size];
        string url = f.Url;
        if (f.Token != null) url = await ResolvePrivateAsset(f, ct);
        using (var resp = await http.GetAsync(url, HttpCompletionOption.ResponseHeadersRead, ct))
        {
            resp.EnsureSuccessStatusCode();
            if (resp.Content.Headers.ContentLength is long cl && cl != f.Size) throw new InvalidDataException("The file is not the size the update says.");
            await using var src = await resp.Content.ReadAsStreamAsync(ct);
            await ReadAll(src, buf, progress, ct);
        }
        return Check(buf, f);
    }

    /// <summary>
    /// A release asset of the private test repository: the GitHub API, asked with the token, answers with a redirect to a
    /// short-lived download address. Redirects are followed by hand so the token goes to api.github.com only and never
    /// to the download host; the file itself is then fetched without it.
    /// </summary>
    static async Task<string> ResolvePrivateAsset(UpdateFile f, CancellationToken ct)
    {
        if (!f.Url.StartsWith(GitHubApi, StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("A token may only be sent to the GitHub API.");
        using var api = new HttpClient(new HttpClientHandler { AllowAutoRedirect = false }) { Timeout = TimeSpan.FromMinutes(2) };
        api.DefaultRequestHeaders.UserAgent.ParseAdd("DomainOS-Toolbox/" + AppVersion.Full.Replace('+', '.'));
        using var req = new HttpRequestMessage(HttpMethod.Get, f.Url);
        req.Headers.Authorization = new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer", f.Token);
        req.Headers.Accept.ParseAdd("application/octet-stream");
        req.Headers.Add("X-GitHub-Api-Version", "2022-11-28");
        using var resp = await api.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct);
        if ((int)resp.StatusCode is >= 300 and < 400 && resp.Headers.Location is { } loc)
        {
            var to = loc.IsAbsoluteUri ? loc : new Uri(new Uri(f.Url), loc);
            if (to.Scheme != Uri.UriSchemeHttps) throw new InvalidDataException("The test download did not point to an https address.");
            return to.AbsoluteUri;
        }
        if (resp.StatusCode is System.Net.HttpStatusCode.NotFound or System.Net.HttpStatusCode.Unauthorized or System.Net.HttpStatusCode.Forbidden)
            throw new InvalidDataException("The test build could not be fetched: the testers' access to it was changed. Get the newest test build from the testers channel.");
        resp.EnsureSuccessStatusCode();
        throw new InvalidDataException("The GitHub API did not give a download address.");
    }

    /// <summary>Reads exactly buf.Length bytes; more data than that is refused.</summary>
    internal static async Task ReadAll(Stream src, byte[] buf, IProgress<double> progress, CancellationToken ct)
    {
        int got = 0, shown = -1;
        while (got < buf.Length)
        {
            int n = await src.ReadAsync(buf.AsMemory(got, buf.Length - got), ct);
            if (n == 0) throw new InvalidDataException($"The download ended early ({got} of {buf.Length} bytes): nothing was changed.");
            got += n;
            int pct = (int)(got * 100L / buf.Length);
            if (pct != shown) { shown = pct; progress?.Report(pct / 100.0); }
        }
        var one = new byte[1];
        if (await src.ReadAsync(one, ct) > 0) throw new InvalidDataException("The file is larger than the update says.");
    }

    /// <summary>Size and SHA-256 of these exact bytes, then (sealed) the tester key opens them in memory.</summary>
    internal static byte[] Check(byte[] data, UpdateFile f)
    {
        string sum = Sha256(data);
        if (data.LongLength != f.Size || !string.Equals(sum, f.Sha256, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException($"The downloaded file does not match (size {data.LongLength}, SHA-256 {sum[..12]}…): nothing was changed.");
        if (!f.Sealed) return data;
        byte[] key = TesterKey.Current() ?? throw new InvalidDataException(KeyRefused);
        try { return TesterKey.Open(data, key); }
        catch (CryptographicException) { throw new InvalidDataException(KeyRefused); }
    }

    /// <summary>A plain file name (no folder, no ".." and nothing but letters, digits, dot, dash and underscore).</summary>
    public static bool SafeFileName(string name) =>
        !string.IsNullOrEmpty(name) && name.Length <= 100 && name.All(c => char.IsAsciiLetterOrDigit(c) || c is '.' or '-' or '_') && !name.StartsWith('.') && !name.Contains("..");

    /// <summary>
    /// Writes checked bytes once into a new folder with a random name under dir (the downloads folder): the folder and
    /// the file are created new (an existing one is never reused), the file is opened without sharing while it is
    /// written. Older download folders are removed first. Returns the file.
    /// </summary>
    public static string WriteNew(byte[] data, string fileName, string dir = null)
    {
        if (!SafeFileName(fileName)) throw new InvalidDataException("Not a plain file name: " + fileName);
        dir ??= Dir;
        Directory.CreateDirectory(dir);
        foreach (var old in Directory.GetDirectories(dir)) { try { Directory.Delete(old, true); } catch (Exception) { } }
        foreach (var old in Directory.GetFiles(dir)) { try { File.Delete(old); } catch (Exception) { } }
        string sub = Path.Combine(dir, Convert.ToHexString(RandomNumberGenerator.GetBytes(8)).ToLowerInvariant());
        if (Directory.Exists(sub) || File.Exists(sub)) throw new IOException("The download folder already exists: " + sub);
        Directory.CreateDirectory(sub);
        string path = Path.Combine(sub, fileName);
        using (var fs = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None))
        {
            fs.Write(data);
            fs.Flush(true);
        }
        return path;
    }

    /// <summary>Test hook for the pauses between retries.</summary>
    public static Action<int> RetrySleep = Thread.Sleep;

    /// <summary>A file operation tried up to five times (an antivirus scan can hold a fresh exe for a moment).</summary>
    static void Retry(Action a)
    {
        for (int i = 0, wait = 200; ; i++, wait *= 2)
        {
            try { a(); return; }
            catch (Exception) when (i < 4) { RetrySleep(wait); }
        }
    }

    /// <summary>
    /// Installs the new program file: the running exe is renamed to "&lt;exe&gt;.prev" (Windows allows renaming a
    /// running program) and the checked bytes are written as the exe, created new and unshared while written. On any
    /// failure the old file is put back.
    /// </summary>
    public static void Swap(string exe, byte[] content)
    {
        if (content == null || content.Length == 0) throw new InvalidDataException("No program file to install.");
        string prev = exe + ".prev";
        if (File.Exists(prev)) Retry(() => File.Delete(prev));
        Retry(() => File.Move(exe, prev));
        try
        {
            using var fs = new FileStream(exe, FileMode.CreateNew, FileAccess.Write, FileShare.None);
            fs.Write(content);
            fs.Flush(true);
        }
        catch (Exception)
        {
            try { if (File.Exists(exe)) File.Delete(exe); } catch (Exception) { }
            Retry(() => File.Move(prev, exe));
            throw;
        }
    }

    /// <summary>
    /// Puts the previous program file back (after a failed start of the new one). The program file is never left
    /// missing: each move is retried, and if the previous file cannot take its place the new one goes back.
    /// </summary>
    public static void SwapBack(string exe) => Exchange(exe, exe + ".prev", exe + ".bad");

    /// <summary>"Return to the previous version": the previous file becomes the program, the current one "&lt;exe&gt;.next".</summary>
    public static void SwapToPrevious(string exe) => Exchange(exe, exe + ".prev", exe + ".next");

    /// <summary>The previous version did not start after a return to it: the newer one comes back, the previous is kept as ".prev".</summary>
    public static void UndoSwapToPrevious(string exe) => Exchange(exe, exe + ".next", exe + ".prev");

    /// <summary>The exe is moved to park, the file in from takes its place; on failure the exe comes back from park.</summary>
    static void Exchange(string exe, string from, string park)
    {
        if (!File.Exists(from)) throw new FileNotFoundException("There is no other version to switch to.", from);
        if (File.Exists(park)) Retry(() => File.Delete(park));
        if (File.Exists(exe)) Retry(() => File.Move(exe, park));
        try { Retry(() => File.Move(from, exe)); }
        catch (Exception)
        {
            if (!File.Exists(exe) && File.Exists(park)) Retry(() => File.Move(park, exe));
            throw;
        }
    }
}
