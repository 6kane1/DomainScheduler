namespace DomainScheduler;

/// <summary>
/// Anti-cheat and protection rules: a game under a kernel anti-cheat is never
/// opened (not even for reading), anti-cheat threads are never touched.
/// </summary>
public static class Guard
{
    public enum Op { Process, Thread }

    /// <summary>Games whose kernel anti-cheat forbids touching them: exe → anti-cheat.</summary>
    static readonly Dictionary<string, string> AcGames = new(StringComparer.OrdinalIgnoreCase)
    {
        ["VALORANT-Win64-Shipping.exe"] = "Vanguard",
        ["League of Legends.exe"] = "Vanguard",
        ["FortniteClient-Win64-Shipping.exe"] = "Easy Anti-Cheat",
        ["r5apex.exe"] = "Easy Anti-Cheat",
        ["r5apex_dx12.exe"] = "Easy Anti-Cheat",
        ["RustClient.exe"] = "Easy Anti-Cheat",
        ["EscapeFromTarkov.exe"] = "BattlEye",
        ["RainbowSix.exe"] = "BattlEye",
        ["RainbowSix_BE.exe"] = "BattlEye",
        ["TslGame.exe"] = "BattlEye",
        ["DayZ_x64.exe"] = "BattlEye",
        ["cod.exe"] = "Ricochet",
        ["destiny2.exe"] = "BattlEye",
        ["Overwatch.exe"] = "Defense Matrix",
        ["GTA5.exe"] = "BattlEye",
        ["eldenring.exe"] = "Easy Anti-Cheat",
        ["FallGuys_client_game.exe"] = "Easy Anti-Cheat",
        ["XDefiant.exe"] = "Ricochet",
        ["thefinals.exe"] = "Easy Anti-Cheat",
        ["deadlock.exe"] = "Valve",
    };

    /// <summary>Anti-cheat clients that protect a game they run with (CS2 on FACEIT / ESEA).</summary>
    static readonly Dictionary<string, string> AcClients = new(StringComparer.OrdinalIgnoreCase)
    {
        ["faceit.exe"] = "FACEIT", ["FACEITService.exe"] = "FACEIT", ["faceitclient.exe"] = "FACEIT",
        ["ESEAClient.exe"] = "ESEA", ["eseaclient.exe"] = "ESEA",
        ["vgc.exe"] = "Vanguard", ["vgtray.exe"] = "Vanguard",
    };

    /// <summary>Games that a running anti-cheat client protects.</summary>
    static readonly HashSet<string> ClientProtected = new(StringComparer.OrdinalIgnoreCase) { "cs2.exe", "csgo.exe" };

    static readonly string[] AcModules =
    {
        "easyanticheat", "eac_", "eosac", "beclient", "battleye", "bedaisy", "vgc", "vgk", "vanguard", "faceit",
        "gameguard", "xigncode", "nprotect", "ricochet", "mhyprot", "acebase", "ace-", "anticheat", "esea",
    };

    /// <summary>System-critical processes: never modified (read-only diagnostics only).</summary>
    static readonly HashSet<string> Critical = new(StringComparer.OrdinalIgnoreCase)
    {
        "System", "Registry", "Idle", "[System Process]", "Secure System", "Memory Compression", "smss.exe", "csrss.exe", "wininit.exe",
        "services.exe", "lsass.exe", "MsMpEng.exe", "MpDefenderCoreService.exe", "NisSrv.exe", "fontdrvhost.exe",
    };

    public static bool IsCritical(string exe) => Critical.Contains(exe ?? "");

    /// <summary>The built-in denylist: system-critical and integrity-sensitive processes.</summary>
    public static bool IsDenied(string exe) => IsCritical(exe) || IsAntiCheatProcess(exe) || AcGames.ContainsKey(exe ?? "");

    /// <summary>
    /// Section 10: a process may be changed only when it is on the watchlist and not on the
    /// denylist. Returns the reason it may not, null when it may.
    /// </summary>
    public static string Eligibility(string exe, ICollection<string> watch, IEnumerable<string> runningExes = null)
    {
        if (string.IsNullOrEmpty(exe)) return L.T("No process");
        string why = Gate(exe, Op.Process, false, runningExes);
        if (why != null) return why;
        if (watch == null || !watch.Contains(exe)) return L.F("{0} is not on the watchlist", exe);
        return null;
    }

    /// <summary>Anti-cheat processes and services: never opened, never lowered.</summary>
    public static bool IsAntiCheatProcess(string exe)
    {
        string e = exe ?? "";
        return AcClients.ContainsKey(e) || AcModules.Any(m => e.Contains(m, StringComparison.OrdinalIgnoreCase));
    }

    public static bool IsAntiCheat(string module) =>
        !string.IsNullOrEmpty(module) && AcModules.Any(m => module.Contains(m, StringComparison.OrdinalIgnoreCase));

    /// <summary>
    /// Why the program must not touch a process; null = it may. runningExes are the images
    /// running now (an anti-cheat client protects the games it runs with).
    /// </summary>
    public static string Gate(string exe, Op op, bool isProtected, IEnumerable<string> runningExes = null)
    {
        if (string.IsNullOrEmpty(exe)) return null;
        if (isProtected) return L.F("{0} is a protected process", exe);
        if (IsCritical(exe)) return L.F("{0} is a system-critical process", exe);
        if (AcGames.TryGetValue(exe, out var ac)) return L.F("{0} runs under {1}", exe, ac);
        if (IsAntiCheatProcess(exe)) return L.F("{0} is an anti-cheat", exe);
        if (ClientProtected.Contains(exe) && runningExes != null)
            foreach (var r in runningExes)
                if (AcClients.TryGetValue(r, out var client)) return L.F("{0} runs under {1}", exe, client);
        return null;
    }
}
