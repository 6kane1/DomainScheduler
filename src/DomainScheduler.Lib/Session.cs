using System.Text.Json;

namespace DomainScheduler;

public sealed class SessionProc
{
    public uint Pid { get; set; }
    public string Name { get; set; } = "";
    public long Created { get; set; }
    public bool Background { get; set; }
    public uint Prio { get; set; }
    public bool PrioSet { get; set; }
    public ulong Aff { get; set; }
    public bool AffSet { get; set; }
    public uint[] Sets { get; set; } = Array.Empty<uint>();
    public bool SetsSet { get; set; }
    public uint Io { get; set; }
    public bool IoSet { get; set; }
    public uint Mem { get; set; }
    public bool MemSet { get; set; }
    public uint EcoCtl { get; set; }
    public uint EcoState { get; set; }
    public bool EcoSet { get; set; }
    public bool BoostDisabled { get; set; }
    public bool BoostSet { get; set; }
}

public sealed class SessionThread
{
    public uint Pid { get; set; }
    public uint Tid { get; set; }
    public long Born { get; set; }
    public int Prio { get; set; }
    public bool PrioSet { get; set; }
    public bool BoostDisabled { get; set; }
    public bool BoostSet { get; set; }
    public uint[] Sets { get; set; } = Array.Empty<uint>();
    public bool SetsSet { get; set; }
    /// <summary>The ideal processor before the first change (crash restore puts it back).</summary>
    public uint Ideal { get; set; }
    public bool IdealSet { get; set; }
}

/// <summary>The crash-safe session file (section 12.10): baselines written before the first change.</summary>
public sealed class SessionData
{
    public DateTime StartedUtc { get; set; } = DateTime.UtcNow;
    public bool CleanShutdown { get; set; }
    /// <summary>The fine timer resolution was requested (section 22.5): released first thing after a crash.</summary>
    public bool TimerRequested { get; set; }
    public List<SessionProc> Procs { get; set; } = new();
    public List<SessionThread> Threads { get; set; } = new();
}

public static class SessionFile
{
    public static Func<string> PathHook;
    public static string FilePath => PathHook?.Invoke() ?? Path.Combine(Store.Dir, "session.json");

    static readonly JsonSerializerOptions Opts = new() { WriteIndented = false };

    public static void Write(SessionData s)
    {
        string p = FilePath;
        Directory.CreateDirectory(Path.GetDirectoryName(p)!);
        string tmp = p + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(s, Opts));
        File.Move(tmp, p, true);
    }

    public static SessionData Read()
    {
        try { return File.Exists(FilePath) ? JsonSerializer.Deserialize<SessionData>(File.ReadAllText(FilePath)) : null; }
        catch (Exception) { return null; }
    }

    public static void Delete()
    {
        try { File.Delete(FilePath); }
        catch (Exception) { }
    }
}
