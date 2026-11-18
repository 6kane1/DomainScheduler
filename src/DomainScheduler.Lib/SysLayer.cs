namespace DomainScheduler;

/// <summary>
/// The preset layer (sections 10, 11 and 14): on every pass the running processes that have an
/// enabled preset get it through Engine.ApplyAll. Built-in presets come from the INI; a user
/// preset applies only to a process on the watchlist. A process is opened again only when its
/// thread set changed, so a pass over an unchanged system opens nothing.
/// </summary>
public sealed class SysLayer
{
    readonly Engine _e;
    readonly object _lock = new();
    Dictionary<string, Preset> _presets = new(StringComparer.OrdinalIgnoreCase);
    HashSet<string> _watch = new(StringComparer.OrdinalIgnoreCase);
    Dictionary<string, string> _affRules = new(StringComparer.OrdinalIgnoreCase);
    readonly Dictionary<uint, ulong> _done = new();     // pid → signature of the thread set that was handled
    readonly HashSet<uint> _refused = new();            // pids that cannot be opened or are protected
    long _reverts = -1;
    bool _enabled = true;
    string _mask = "auto";

    public int LastTargets { get; private set; }
    public int Passes { get; private set; }

    public SysLayer(Engine engine) => _e = engine;

    /// <summary>New presets, watchlist, mask rules or switches: everything is applied again on the next pass.</summary>
    public void Configure(Dictionary<string, Preset> presets, IEnumerable<string> watch, Dictionary<string, string> affRules, string maskName, bool enabled)
    {
        lock (_lock)
        {
            _presets = new Dictionary<string, Preset>(presets ?? new(), StringComparer.OrdinalIgnoreCase);
            _watch = new HashSet<string>(watch ?? Array.Empty<string>(), StringComparer.OrdinalIgnoreCase);
            _affRules = new Dictionary<string, string>(affRules ?? new(), StringComparer.OrdinalIgnoreCase);
            _mask = string.IsNullOrEmpty(maskName) ? "auto" : maskName;
            _enabled = enabled;
            _done.Clear();
            _refused.Clear();
        }
    }

    public void Invalidate()
    {
        lock (_lock) _done.Clear();
    }

    static ulong Signature(List<uint> tids)
    {
        ulong h = 1469598103934665603UL;
        foreach (uint t in tids.OrderBy(x => x)) h = (h ^ t) * 1099511628211UL;
        return h ^ (ulong)tids.Count;
    }

    /// <summary>Whether the preset of this exe may be applied, and how.</summary>
    ApplyOptions OptionsFor(Preset p, string exe)
    {
        if (p == null || !p.Enabled) return null;
        if (Guard.IsDenied(exe)) return null;
        bool watched = _watch.Contains(exe);
        if (!p.Builtin && !watched) return null;          // a user preset needs the watchlist
        if (p.Builtin && !_enabled) return null;
        // Only a watched process gets a mask; system presets touch threads and boost only.
        return new ApplyOptions { MaskName = _mask, DoMask = watched && !p.Builtin, DoAffRules = watched && !p.Builtin, DoThreads = true, Sys = p.Builtin, Game = watched && !p.Builtin };
    }

    /// <summary>One pass; skip = the process Auto is laying out itself. Returns how many processes were applied.</summary>
    public int Pass(uint skip = 0)
    {
        lock (_lock)
        {
            Passes++;
            if (_e.Reverts != _reverts)
            {
                _done.Clear();           // something was reverted: what this layer set may be gone
                _reverts = _e.Reverts;
            }
            var reports = _e.ApplyAll((procs, tids) =>
            {
                var list = new List<Engine.Target>();
                foreach (uint gone in _done.Keys.Where(p => !procs.ContainsKey(p)).ToList()) _done.Remove(gone);
                _refused.RemoveWhere(p => !procs.ContainsKey(p));
                foreach (var (pid, pr) in procs)
                {
                    if (pid is 0 or 4 || pid == skip || _refused.Contains(pid)) continue;
                    if (!_presets.TryGetValue(pr.Name, out var preset))
                    {
                        // A watched process with only a mask rule: an empty preset carries the mask.
                        if (!_watch.Contains(pr.Name) || !_affRules.ContainsKey(pr.Name)) continue;
                        preset = new Preset { Exe = pr.Name, Class = "", Io = "", Mem = "", Eco = "", Boost = "" };
                    }
                    var o = OptionsFor(preset, pr.Name);
                    if (o == null) continue;
                    ulong sig = Signature(tids.TryGetValue(pid, out var l) ? l : new List<uint>());
                    if (_done.TryGetValue(pid, out ulong old) && old == sig) continue;
                    _done[pid] = sig;
                    list.Add(new Engine.Target { Pid = pid, Preset = preset, Options = o });
                }
                return list;
            }, _affRules);
            foreach (var r in reports)
                if (r.HandleLevel < 0 || r.Failures.Count > 0 && r.NamedHits == 0 && r.LastThreads == 0) _refused.Add((uint)r.Pid);
            _reverts = _e.Reverts;
            LastTargets = reports.Count;
            return reports.Count;
        }
    }
}
