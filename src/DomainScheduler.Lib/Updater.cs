using System.Net.Http;
using System.Reflection;
using System.Security.Cryptography;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// The version of this build and the update channel built into it (1.15.0). A stable build follows only the stable
/// releases; a test build ("1.15.0-test1.0") follows the test builds and the stable releases, whichever is newer.
/// The channel comes from the build itself (the csproj property UpdateChannel), not from a setting or a login.
/// </summary>
public static class AppVersion
{
    static string _full, _channel;

    /// <summary>"1.15.0" or "1.15.0-test1.0" (the informational version without the "+commit" tail).</summary>
    public static string Full => _full ??= Read();

    /// <summary>"stable" or "test".</summary>
    public static string Channel => _channel ??= ReadChannel();

    public static bool IsTest => Channel == "test";

    /// <summary>What the window shows: "1.15.0" or "1.15.0 Test 1.0".</summary>
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

    static string ReadChannel()
    {
        var a = Assembly.GetEntryAssembly() ?? typeof(AppVersion).Assembly;
        string c = a.GetCustomAttributes<AssemblyMetadataAttribute>().FirstOrDefault(m => m.Key == "UpdateChannel")?.Value;
        if (string.IsNullOrWhiteSpace(c)) c = Full.Contains("-test", StringComparison.OrdinalIgnoreCase) ? "test" : "stable";
        return c.Trim().ToLowerInvariant() == "test" ? "test" : "stable";
    }

    /// <summary>For tests.</summary>
    public static void Override(string full, string channel) { _full = full; _channel = channel; }
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

/// <summary>One file in an update feed: Toolbox or the playbook.</summary>
public sealed record UpdateFile(string Version, string Url, string Sha256, long Size, string NotesEn, string NotesRu);

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
            var f = new UpdateFile(S("version"), S("url"), S("sha256")?.ToLowerInvariant(), size, S("notes_en") ?? "", S("notes_ru") ?? "");
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
                string q = "?t=" + DateTime.UtcNow.Ticks / TimeSpan.TicksPerMinute;
                byte[] body;
                try { body = await http.GetByteArrayAsync(FeedBase + name + q, ct); }
                catch (HttpRequestException e) when (e.StatusCode == System.Net.HttpStatusCode.NotFound) { return null; }
                string sig;
                try { sig = await http.GetStringAsync(FeedBase + name + ".sig" + q, ct); }
                catch (HttpRequestException e) when (e.StatusCode == System.Net.HttpStatusCode.NotFound) { sig = null; }
                if (!Verify(body, sig)) throw new InvalidDataException("The update file " + name + " has no valid Domain OS signature: it was ignored.");
                return ParseFeed(System.Text.Encoding.UTF8.GetString(body));
            }
            var stable = await Get("stable.json");
            var test = AppVersion.IsTest ? await Get("test.json") : null;
            return Choose(AppVersion.Full, AppVersion.Channel, InstalledPlaybook(), stable, test);
        }
        catch (Exception e) when (e is not OperationCanceledException || !ct.IsCancellationRequested)
        {
            return new UpdateCheck(null, null, e.Message);
        }
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
        if (File.Exists(final) && new FileInfo(final).Length == f.Size && Sha256(final) == f.Sha256) return final;
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
