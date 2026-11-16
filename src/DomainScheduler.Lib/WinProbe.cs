namespace DomainScheduler;

/// <summary>IGameProbe over the Windows API.</summary>
public sealed class WinGameProbe : IGameProbe
{
    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000, PROCESS_VM_READ = 0x0010, GW_OWNER = 4;
    static readonly string[] GraphicsModules = { "d3d11.dll", "d3d12.dll", "vulkan-1.dll", "opengl32.dll" };

    readonly Dictionary<(uint pid, long created), (bool has, DateTime at)> _graphics = new();

    public uint SelfPid { get; } = (uint)Environment.ProcessId;

    public uint ForegroundPid()
    {
        IntPtr w = Native.GetForegroundWindow();
        if (w == IntPtr.Zero) return 0;
        Native.GetWindowThreadProcessId(w, out uint pid);
        return pid;
    }

    public HashSet<uint> WindowPids()
    {
        var set = new HashSet<uint>();
        Native.EnumWindows((h, _) =>
        {
            if (Native.IsWindowVisible(h) && Native.GetWindow(h, GW_OWNER) == IntPtr.Zero)
            {
                Native.GetWindowThreadProcessId(h, out uint pid);
                if (pid != 0) set.Add(pid);
            }
            return true;
        }, IntPtr.Zero);
        return set;
    }

    /// <summary>A graphics API module is loaded; a positive answer is kept, a negative one is checked again after 10 s.</summary>
    public bool HasGraphicsModule(uint pid)
    {
        if (!CpuTime(pid, out long created, out _)) return false;
        if (_graphics.TryGetValue((pid, created), out var c) && (c.has || DateTime.UtcNow - c.at < TimeSpan.FromSeconds(10))) return c.has;
        bool has = false;
        IntPtr h = Native.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid);
        if (h != IntPtr.Zero)
            try { has = Native.Modules(h).Any(m => GraphicsModules.Contains(m.Name, StringComparer.OrdinalIgnoreCase)); }
            finally { Native.CloseHandle(h); }
        if (_graphics.Count > 512) _graphics.Clear();
        _graphics[(pid, created)] = (has, DateTime.UtcNow);
        return has;
    }

    public bool CpuTime(uint pid, out long created, out long cpu100ns)
    {
        created = cpu100ns = 0;
        IntPtr h = Native.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == IntPtr.Zero) return false;
        try
        {
            if (!Native.GetProcessTimes(h, out created, out _, out long k, out long u)) return false;
            cpu100ns = k + u;
            return true;
        }
        finally { Native.CloseHandle(h); }
    }

    public uint SessionOf(uint pid) => Native.ProcessIdToSessionId(pid, out uint s) ? s : uint.MaxValue;
}
