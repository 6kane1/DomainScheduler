//! Programs the background layer never changes (port of `BackgroundDeny.cs`): audio, voice, streaming and capture,
//! VR. They run next to a game on purpose and are timing-sensitive: a lowered priority, EcoQoS or an ignored timer
//! request is heard as crackling sound, seen as a stuttering stream or felt as judder in a headset. They are never
//! lowered, never put on EcoQoS and their timer requests are never ignored, in the user's session and as services
//! alike. This is the one place of the list; matching is by exe name, case-insensitive. A name ending in "*" matches
//! every exe that starts with it (Voicemeeter and Dolby ship several builds).

use crate::base::ci::{self, CiSet, CiSetExt};
use std::sync::OnceLock;

const NAMES: [&str; 28] = [
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
];

fn exact() -> &'static CiSet {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| ci::set_of(NAMES.iter().copied().filter(|n| !n.ends_with('*'))))
}

fn prefixes() -> &'static Vec<&'static str> {
    static V: OnceLock<Vec<&'static str>> = OnceLock::new();
    V.get_or_init(|| NAMES.iter().filter(|n| n.ends_with('*')).map(|n| n.trim_end_matches('*')).collect())
}

/// `BackgroundDeny` (static class).
pub struct BackgroundDeny;

impl BackgroundDeny {
    /// The list as written (the Report and tests).
    pub fn all() -> &'static [&'static str] {
        &NAMES
    }

    pub fn contains(exe: &str) -> bool {
        if exe.is_empty() {
            return false;
        }
        if exact().ci_contains(exe) {
            return true;
        }
        prefixes().iter().any(|p| ci::starts_with(exe, p) && ci::ends_with(exe, ".exe"))
    }
}
