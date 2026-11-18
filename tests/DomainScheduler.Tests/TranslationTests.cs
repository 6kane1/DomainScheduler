using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>
/// 1.11.0: Toolbox speaks Russian and English only, and every text the sources hand to L.T / L.F / L.N
/// (L.N marks the catalog texts that are translated later through a variable) has a Russian entry.
/// </summary>
public class TranslationTests
{
    static readonly Regex Call = new(@"\bL\.([TFN])\(\s*(@?)""((?:[^""\\]|\\.|"""")*)""", RegexOptions.Compiled);

    static string Root()
    {
        string root = AppContext.BaseDirectory;
        while (root != null && !File.Exists(Path.Combine(root, "DomainScheduler.sln"))) root = Path.GetDirectoryName(root);
        return root;
    }

    static string Lang => Path.Combine(Root(), "src", "DomainScheduler.Lib", "Lang");

    static IEnumerable<string> Sources() =>
        Directory.GetFiles(Path.Combine(Root(), "src"), "*.cs", SearchOption.AllDirectories)
            .Where(x => !x.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                     && !x.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar));

    static string Unescape(string s, bool verbatim)
    {
        if (verbatim) return s.Replace("\"\"", "\"");
        var sb = new StringBuilder();
        for (int i = 0; i < s.Length; i++)
        {
            if (s[i] != '\\' || i + 1 >= s.Length) { sb.Append(s[i]); continue; }
            char c = s[++i];
            switch (c)
            {
                case 'n': sb.Append('\n'); break;
                case 't': sb.Append('\t'); break;
                case 'r': sb.Append('\r'); break;
                case '0': sb.Append('\0'); break;
                case 'u': sb.Append((char)Convert.ToInt32(s.Substring(i + 1, 4), 16)); i += 4; break;
                default: sb.Append(c); break;   // \" \\ \'
            }
        }
        return sb.ToString();
    }

    /// <summary>Every literal text passed to L.T / L.F / L.N, with the file it is in.</summary>
    static Dictionary<string, string> Literals()
    {
        var all = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (string f in Sources())
            foreach (Match m in Call.Matches(File.ReadAllText(f)))
                all.TryAdd(Unescape(m.Groups[3].Value, m.Groups[2].Value == "@"), Path.GetFileName(f));
        return all;
    }

    static Dictionary<string, string> Ru()
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(Lang, "ru.json")));
        return doc.RootElement.GetProperty("strings").EnumerateObject().ToDictionary(p => p.Name, p => p.Value.GetString(), StringComparer.Ordinal);
    }

    /// <summary>A text with no letters (only placeholders, numbers, punctuation) needs no translation.</summary>
    static bool FormatOnly(string s) => !Regex.Replace(s, @"\{[^}]*\}", "").Any(char.IsLetter);

    [Fact]
    public void EveryLiteralTextHasARussianEntry()
    {
        var ru = Ru();
        var lits = Literals();
        Assert.True(lits.Count > 500, $"only {lits.Count} texts found: the scan is broken");
        var missing = lits.Where(kv => !FormatOnly(kv.Key) && !(ru.TryGetValue(kv.Key, out var v) && !string.IsNullOrWhiteSpace(v)))
            .Select(kv => kv.Value + ": " + kv.Key).ToList();
        Assert.True(missing.Count == 0, "no Russian text for:\n" + string.Join("\n", missing));
    }

    [Fact]
    public void RussianEntriesKeepThePlaceholdersAndAreUsed()
    {
        var lits = Literals();
        foreach (var (en, ru) in Ru())
        {
            Assert.True(lits.ContainsKey(en), "not used in the sources: " + en);
            var want = Regex.Matches(en, @"\{\d+[^}]*\}").Select(m => m.Value).OrderBy(x => x).ToList();
            var got = Regex.Matches(ru, @"\{\d+[^}]*\}").Select(m => m.Value).OrderBy(x => x).ToList();
            Assert.True(want.SequenceEqual(got), "placeholders differ: " + en + " => " + ru);
        }
    }

    [Fact]
    public void OnlyRussianAndEnglish()
    {
        Assert.Equal(new[] { "en", "ru" }, L.Available);
        var files = Directory.GetFiles(Lang, "*.json").Select(Path.GetFileName).Where(n => n != "tables.json").OrderBy(n => n).ToArray();
        Assert.Equal(new[] { "en.json", "ru.json" }, files);
        Assert.Equal("English", L.NameOf("zh"));
    }
}
