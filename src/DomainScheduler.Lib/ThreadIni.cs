using System.Globalization;

namespace DomainScheduler;

/// <summary>
/// The built-in system presets (section 14): an embedded INI with [SETTINGS], [DISABLEBOOST]
/// and one section per process. Parsed into presets with Builtin = true; the sys layer applies
/// them independently of Auto.
/// </summary>
public static class ThreadIni
{
    public sealed class Row
    {
        public string Process = "";
        public bool Skipped;
        public string Note = "";
    }

    public sealed class Result
    {
        public List<Row> Rows = new();
        public Dictionary<string, Preset> Presets = new(StringComparer.OrdinalIgnoreCase);
        public int Sleep = DefaultSleep;
    }

    public const int DefaultSleep = 10000;

    /// <summary>Test hook: replaces the embedded text.</summary>
    public static Func<string> TextHook;
    static Result _cache;

    public static Result LoadBundled()
    {
        string text = TextHook?.Invoke();
        if (text != null) return Parse(text);
        if (_cache != null) return _cache;
        using var s = typeof(ThreadIni).Assembly.GetManifestResourceStream("threads.ini");
        if (s == null) return _cache = new Result();
        using var rd = new StreamReader(s);
        return _cache = Parse(rd.ReadToEnd());
    }

    public static string ExeOf(string name) =>
        name.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? name : name + ".exe";

    static Preset NewPreset(string exe) => new() { Exe = exe, Class = "", Io = "", Mem = "", Eco = "", Boost = "", Builtin = true };

    public static Result Parse(string text)
    {
        var r = new Result();
        var rows = new Dictionary<string, Row>(StringComparer.OrdinalIgnoreCase);
        string sec = "";
        Row row = null;
        foreach (var raw in (text ?? "").Split('\n'))
        {
            string line = raw.Trim().TrimStart('﻿');
            if (line.Length == 0 || line[0] is ';' or '#') continue;
            if (line[0] == '[' && line.EndsWith(']'))
            {
                sec = line[1..^1].Trim();
                row = null;
                if (sec.Length == 0 || IsList(sec)) continue;
                string exe = ExeOf(sec);
                if (!rows.TryGetValue(exe, out row))
                {
                    row = new Row { Process = exe };
                    rows[exe] = row;
                    r.Rows.Add(row);
                }
                if (!r.Presets.ContainsKey(exe)) r.Presets[exe] = NewPreset(exe);
                continue;
            }
            int eq = line.IndexOf('=');
            string key = (eq < 0 ? line : line[..eq]).Trim(), val = eq < 0 ? "" : line[(eq + 1)..].Trim();
            if (key.Length == 0) continue;
            if (sec.Equals("SETTINGS", StringComparison.OrdinalIgnoreCase))
            {
                if (key.Equals("sleep", StringComparison.OrdinalIgnoreCase) &&
                    int.TryParse(val.Split(';')[0].Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out int ms) && ms > 0)
                    r.Sleep = Math.Max(1000, ms);
                continue;
            }
            if (sec.Equals("DISABLEBOOST", StringComparison.OrdinalIgnoreCase))
            {
                string exe = ExeOf(key);
                if (!r.Presets.TryGetValue(exe, out var bp)) r.Presets[exe] = bp = NewPreset(exe);
                bp.Boost = "off";
                continue;
            }
            if (row == null) continue;
            var preset = r.Presets[row.Process];
            string kind = key.ToLowerInvariant();
            if (kind is not ("module" or "description"))
            {
                row.Note = L.F("unknown key: {0}", key);
                continue;
            }
            var f = val.Split(',').Select(x => x.Trim()).ToArray();
            if (f.Length < 2 || f[0].Length == 0)
            {
                row.Note = L.F("broken rule: {0}", line);
                continue;
            }
            var rule = new ThreadRule { Match = kind == "module" ? "module" : "desc", Pattern = f[0], Prio = f[1], Enabled = true };
            foreach (var extra in f.Skip(2))
            {
                string x = extra.Replace(" ", "").ToLowerInvariant();
                if (x == "disableboost") rule.Boost = "off";
                else if (x.StartsWith("tid=")) rule.Tid = x[4..];
                else row.Note = L.F("unknown option: {0}", extra);
            }
            preset.Threads.Add(rule);
        }
        return r;
    }

    static bool IsList(string sec) => sec.Equals("SETTINGS", StringComparison.OrdinalIgnoreCase) || sec.Equals("DISABLEBOOST", StringComparison.OrdinalIgnoreCase);

    public static bool ValidPrio(string p) =>
        int.TryParse(p, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out int n) && Engine.TPRIO.ContainsValue(n);

    static bool ValidTid(string t) =>
        t is "" or "first" or "last" || int.TryParse(t, NumberStyles.None, CultureInfo.InvariantCulture, out _);

    /// <summary>Checks the built-in presets; an empty list means OK. Problems never block the run.</summary>
    public static List<string> SelfTest() => SelfTest(LoadBundled());

    public static List<string> SelfTest(Result r)
    {
        var bad = new List<string>();
        foreach (var row in r.Rows)
        {
            if (row.Skipped && string.IsNullOrEmpty(row.Note)) bad.Add(L.F("{0}: skipped without a note", row.Process));
            else if (!string.IsNullOrEmpty(row.Note)) bad.Add(row.Process + ": " + row.Note);
        }
        foreach (var p in r.Presets.Values)
        {
            if (Guard.IsDenied(p.Exe)) bad.Add(L.F("{0}: a denylisted process is among the presets", p.Exe));
            if (p.Threads.Count == 0 && p.Boost.Length == 0) bad.Add(L.F("{0}: no rules", p.Exe));
            if (p.Boost is not ("" or "on" or "off")) bad.Add(L.F("{0}: bad boost", p.Exe));
            foreach (var t in p.Threads)
            {
                string id = p.Exe + "/" + t.Pattern;
                if (string.IsNullOrWhiteSpace(t.Pattern)) bad.Add(L.F("{0}: empty pattern", p.Exe));
                if (!ValidPrio(t.Prio)) bad.Add(L.F("{0}: bad priority", id));
                if (t.Boost is not ("" or "on" or "off")) bad.Add(L.F("{0}: bad boost", id));
                if (!(t.Ideal is "" or "auto" || int.TryParse(t.Ideal, NumberStyles.None, CultureInfo.InvariantCulture, out int n) && n is >= 0 and <= 63))
                    bad.Add(L.F("{0}: bad ideal processor", id));
                if (t.Match is not ("module" or "desc" or "tid")) bad.Add(L.F("{0}: bad match", id));
                if (!ValidTid(t.Tid)) bad.Add(L.F("{0}: bad tid filter", id));
            }
        }
        return bad;
    }
}
