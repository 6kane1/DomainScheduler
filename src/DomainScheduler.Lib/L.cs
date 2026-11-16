using System.Globalization;
using System.Reflection;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// Localization: one JSON resource per language with an en → translation map.
/// T translates with a fallback to the key, N marks a string for extraction
/// without translating it, F formats a translated string.
/// </summary>
public static class L
{
    static Dictionary<string, string> _map = new();
    static List<string> _fonts = new();
    public static string Lang { get; private set; } = "en";

    /// <summary>Languages with a resource, by code ("en" is the keys themselves).</summary>
    public static IReadOnlyList<string> Available => new[] { "en", "ru", "zh" };

    /// <summary>The languages' own names (never translated).</summary>
    public static string NameOf(string code) => code switch { "ru" => "Русский", "zh" => "简体中文", _ => "English" };

    /// <summary>The first non-empty font of the current language ("" = the system default).</summary>
    public static string Family => _fonts.FirstOrDefault(f => !string.IsNullOrWhiteSpace(f)) ?? "";

    /// <summary>Raised after Load: the UI rebuilds its texts.</summary>
    public static event Action Changed;

    public static void Load(string lang)
    {
        lang = string.IsNullOrWhiteSpace(lang) ? Detect() : lang.Trim().ToLowerInvariant();
        var map = new Dictionary<string, string>(StringComparer.Ordinal);
        var fonts = new List<string>();
        {
            var asm = typeof(L).Assembly;
            string name = asm.GetManifestResourceNames().FirstOrDefault(n => n.EndsWith("." + lang + ".json", StringComparison.OrdinalIgnoreCase));
            if (name == null)
                lang = "en";
            else
            {
                using var s = asm.GetManifestResourceStream(name);
                using var doc = JsonDocument.Parse(s);
                if (doc.RootElement.TryGetProperty("fonts", out var f) && f.ValueKind == JsonValueKind.Array)
                    fonts.AddRange(f.EnumerateArray().Select(x => x.GetString()).Where(x => !string.IsNullOrWhiteSpace(x)));
                if (doc.RootElement.TryGetProperty("strings", out var st) && st.ValueKind == JsonValueKind.Object)
                    foreach (var p in st.EnumerateObject())
                        map[p.Name] = p.Value.GetString() ?? p.Name;
            }
        }
        _map = map;
        _fonts = fonts;
        Lang = lang;
        Changed?.Invoke();
    }

    /// <summary>The UI language of Windows when there is a resource for it, else English.</summary>
    public static string Detect()
    {
        string two = CultureInfo.CurrentUICulture.TwoLetterISOLanguageName.ToLowerInvariant();
        return Available.Contains(two) ? two : "en";
    }

    public static string T(string en) => en != null && _map.TryGetValue(en, out var t) && !string.IsNullOrEmpty(t) ? t : en;

    public static string N(string en) => en;

    public static string F(string en, params object[] args)
    {
        string fmt = T(en);
        try { return string.Format(CultureInfo.CurrentCulture, fmt, args); }
        catch (FormatException) { return string.Format(CultureInfo.InvariantCulture, en, args); }
    }
}
