namespace DomainScheduler;

/// <summary>
/// Programs the background layer never changes: audio, voice, streaming and capture, VR. They run next to a game on
/// purpose and are timing-sensitive — a lowered priority, EcoQoS or an ignored timer request is heard as crackling
/// sound, seen as a stuttering stream or felt as judder in a headset. They are never lowered, never put on EcoQoS
/// and their timer requests are never ignored, in the user's session and as services alike.
/// This is the one place of the list; matching is by exe name, case-insensitive. A name ending in "*" matches every
/// exe that starts with it (Voicemeeter and Dolby ship several builds).
/// </summary>
public static class BackgroundDeny
{
    static readonly string[] Names =
    {
        // streaming and capture
        "obs64.exe", "obs32.exe", "obs-browser-page.exe", "Streamlabs OBS.exe", "Streamlabs Desktop.exe",
        "NVIDIA Broadcast.exe", "nvsphelper64.exe", "medal.exe",
        // voice
        "Discord.exe", "ts3client_win64.exe", "TeamSpeak.exe",
        // audio
        "audiodg.exe", "voicemeeter*", "SteelSeriesSonar.exe", "SteelSeriesGG.exe", "SteelSeriesEngine.exe", "WaveLink.exe",
        "NahimicSvc64.exe", "NahimicSvc32.exe", "DolbyDAX*", "RtkAudUService64.exe",
        // VR
        "vrserver.exe", "vrcompositor.exe", "vrmonitor.exe", "vrdashboard.exe", "OVRServer_x64.exe", "OVRRedir.exe",
        "VirtualDesktop.Streamer.exe",
    };

    static readonly HashSet<string> Exact = new(Names.Where(n => !n.EndsWith('*')), StringComparer.OrdinalIgnoreCase);
    static readonly string[] Prefixes = Names.Where(n => n.EndsWith('*')).Select(n => n.TrimEnd('*')).ToArray();

    /// <summary>The list as written (the Report and tests).</summary>
    public static IReadOnlyList<string> All => Names;

    public static bool Contains(string exe)
    {
        if (string.IsNullOrEmpty(exe)) return false;
        if (Exact.Contains(exe)) return true;
        foreach (var p in Prefixes)
            if (exe.StartsWith(p, StringComparison.OrdinalIgnoreCase) && exe.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) return true;
        return false;
    }
}
