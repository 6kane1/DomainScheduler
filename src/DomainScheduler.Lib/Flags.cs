namespace DomainScheduler;

/// <summary>
/// Feature flags (section 22.12): the subkey Flags of the settings, one bool per key, all closed
/// by default except "core". A closed flag hides the control in the UI and makes the engine
/// refuse the action with a warning.
/// </summary>
public static class Flags
{
    public const string Sub = "Flags";
    public const string Core = "core", DevicesWrite = "devices-write", SoftClose = "softclose", EtwIrq = "etwirq", TimerRes = "timerres";
    public static readonly string[] FlagKeys = { Core, DevicesWrite, SoftClose, EtwIrq, TimerRes };
    static readonly HashSet<string> _warned = new();

    public static bool Open(string key)
    {
        if (!FlagKeys.Contains(key)) return false;
        if (Settings.Backend.TryGet(Sub, key, out var v) && !string.IsNullOrEmpty(v)) return v.Equals("true", StringComparison.OrdinalIgnoreCase);
        return key == Core;
    }

    public static void Set(string key, bool open)
    {
        if (!FlagKeys.Contains(key)) return;
        Settings.Backend.Set(Sub, key, open ? "true" : "false");
        lock (_warned) _warned.Remove(key);
        AppLog.Add(open ? L.F("Feature flag {0}: open", key) : L.F("Feature flag {0}: closed", key));
    }

    /// <summary>True when the action must be refused; the refusal is logged (once per flag until it changes).</summary>
    public static bool Refuse(string key, string action)
    {
        if (Open(key)) return false;
        bool first;
        lock (_warned) first = _warned.Add(key);
        if (first) AppLog.Add(L.F("{0}: refused, the feature flag {1} is closed", action, key), AppLog.Sev.Warn);
        return true;
    }
}
