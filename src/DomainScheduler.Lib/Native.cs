using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace DomainScheduler;

/// <summary>Windows calls (section 2). Every handle is closed in a finally block by its caller.</summary>
public static class Native
{
    public const uint P_FULL = 13840, P_NOVM = 13824, P_LIMITED = 12288;   // 0x3610 / 0x3600 / 0x3000
    public const uint T_FULL = 3168, T_LIMITED = 3072;                     // 0x0C60 / 0x0C00
    public const uint TH32CS_SNAPTHREAD = 4, TH32CS_SNAPPROCESS = 2;
    public const uint LIST_MODULES_ALL = 3;
    public const int ProcessIoPriority = 33, ThreadIoPriority = 22;
    public const int ProcessMemoryPriority = 0, ThreadMemoryPriority = 0;
    public const int ProcessPowerThrottling = 4;
    public const int ThreadQuerySetWin32StartAddress = 9;
    public const int ProcessProtectionInformation = 61;
    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;

    // ---- kernel32 ----
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetSystemCpuSetInformation(IntPtr info, uint len, out uint returned, IntPtr process, uint flags);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetLogicalProcessorInformationEx(int relation, IntPtr buffer, ref uint len);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern uint GetProcessIdOfThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern uint GetPriorityClass(IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetPriorityClass(IntPtr process, uint cls);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetProcessAffinityMask(IntPtr process, out UIntPtr procMask, out UIntPtr sysMask);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetProcessAffinityMask(IntPtr process, UIntPtr mask);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetProcessDefaultCpuSets(IntPtr process, [Out] uint[] ids, uint count, out uint required);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetProcessDefaultCpuSets(IntPtr process, uint[] ids, uint count);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetProcessInformation(IntPtr process, int cls, IntPtr info, uint size);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetProcessInformation(IntPtr process, int cls, IntPtr info, uint size);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetProcessPriorityBoost(IntPtr process, out bool disabled);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetProcessPriorityBoost(IntPtr process, bool disable);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern int GetThreadPriority(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetThreadPriority(IntPtr thread, int prio);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetThreadPriorityBoost(IntPtr thread, out bool disabled);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetThreadPriorityBoost(IntPtr thread, bool disable);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern uint SetThreadIdealProcessor(IntPtr thread, uint lp);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetThreadSelectedCpuSets(IntPtr thread, [Out] uint[] ids, uint count, out uint required);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetThreadSelectedCpuSets(IntPtr thread, uint[] ids, uint count);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetThreadIdealProcessorEx(IntPtr thread, out uint processorNumber);
    [DllImport("ntdll.dll")] public static extern int NtQuerySystemInformation(int cls, [Out] byte[] buffer, int length, out int returned);
    [DllImport("ntdll.dll")] public static extern int NtQueryTimerResolution(out uint minimum, out uint maximum, out uint current);
    [DllImport("ntdll.dll")] public static extern int NtSetTimerResolution(uint desired, [MarshalAs(UnmanagedType.U1)] bool set, out uint actual);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool QueryThreadCycleTime(IntPtr thread, out ulong cycles);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetThreadTimes(IntPtr thread, out long creation, out long exit, out long kernel, out long user);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetProcessTimes(IntPtr process, out long creation, out long exit, out long kernel, out long user);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern int GetThreadDescription(IntPtr thread, out IntPtr desc);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr LocalFree(IntPtr mem);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern uint GetActiveProcessorCount(ushort group);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern ulong GetTickCount64();
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool Thread32First(IntPtr snap, ref THREADENTRY32 e);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool Thread32Next(IntPtr snap, ref THREADENTRY32 e);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern bool Process32FirstW(IntPtr snap, ref PROCESSENTRY32W e);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern bool Process32NextW(IntPtr snap, ref PROCESSENTRY32W e);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool K32EnumProcessModulesEx(IntPtr process, [Out] IntPtr[] modules, uint cb, out uint needed, uint filter);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool K32GetModuleInformation(IntPtr process, IntPtr module, out MODULEINFO info, uint cb);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern uint K32GetModuleBaseNameW(IntPtr process, IntPtr module, [Out] char[] name, uint size);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags, [Out] char[] name, ref uint size);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool ProcessIdToSessionId(uint pid, out uint session);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetThreadInformation(IntPtr thread, int cls, IntPtr info, uint size);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetThreadInformation(IntPtr thread, int cls, IntPtr info, uint size);

    // ---- ntdll ----
    [DllImport("ntdll.dll", SetLastError = true)] public static extern int NtSetInformationProcess(IntPtr process, int cls, ref uint value, uint size);
    [DllImport("ntdll.dll", SetLastError = true)] public static extern int NtQueryInformationProcess(IntPtr process, int cls, out uint value, uint size, IntPtr ret);
    [DllImport("ntdll.dll", SetLastError = true)] public static extern int NtQueryInformationProcess(IntPtr process, int cls, out byte value, uint size, IntPtr ret);
    [DllImport("ntdll.dll", SetLastError = true)] public static extern int NtSetInformationThread(IntPtr thread, int cls, ref uint value, uint size);
    [DllImport("ntdll.dll", SetLastError = true)] public static extern int NtQueryInformationThread(IntPtr thread, int cls, out ulong value, uint size, IntPtr ret);

    // ---- advapi32 ----
    [DllImport("advapi32.dll", SetLastError = true)] public static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern bool LookupPrivilegeValue(string system, string name, out long luid);
    [DllImport("advapi32.dll", SetLastError = true)] public static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TOKEN_PRIVILEGES state, uint len, IntPtr prev, IntPtr retLen);

    // ---- user32 ----
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", SetLastError = true)] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr param);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr param);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr GetWindow(IntPtr hwnd, uint cmd);

    [StructLayout(LayoutKind.Sequential)]
    public struct THREADENTRY32
    {
        public uint dwSize, cntUsage, th32ThreadID, th32OwnerProcessID;
        public int tpBasePri, tpDeltaPri;
        public uint dwFlags;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct PROCESSENTRY32W
    {
        public uint dwSize, cntUsage, th32ProcessID;
        public IntPtr th32DefaultHeapID;
        public uint th32ModuleID, cntThreads, th32ParentProcessID;
        public int pcPriClassBase;
        public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szExeFile;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MODULEINFO
    {
        public IntPtr lpBaseOfDll;
        public uint SizeOfImage;
        public IntPtr EntryPoint;
    }

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    public struct TOKEN_PRIVILEGES
    {
        public uint PrivilegeCount;
        public long Luid;
        public uint Attributes;
    }

    // ---- helpers -------------------------------------------------------------------------------

    /// <summary>Opens a process with the most rights it allows: 2 full, 1 without VM read, 0 limited, -1 refused.</summary>
    public static IntPtr OpenProcessGraceful(uint pid, out int level)
    {
        IntPtr h = OpenProcess(P_FULL, false, pid);
        if (h != IntPtr.Zero) { level = 2; return h; }
        h = OpenProcess(P_NOVM, false, pid);
        if (h != IntPtr.Zero) { level = 1; return h; }
        h = OpenProcess(P_LIMITED, false, pid);
        if (h != IntPtr.Zero) { level = 0; return h; }
        level = -1;
        return IntPtr.Zero;
    }

    /// <summary>Opens a thread and checks that it belongs to pid; Zero otherwise.</summary>
    public static IntPtr OpenThreadOf(uint pid, uint tid)
    {
        IntPtr h = OpenThread(T_FULL, false, tid);
        if (h == IntPtr.Zero) h = OpenThread(T_LIMITED, false, tid);
        if (h == IntPtr.Zero) return IntPtr.Zero;
        bool ok = false;
        try
        {
            ok = GetProcessIdOfThread(h) == pid;
            return ok ? h : IntPtr.Zero;
        }
        finally
        {
            if (!ok) CloseHandle(h);
        }
    }

    /// <summary>Turns SeDebugPrivilege on for this process (token 0x28, Attributes 2, len 16).</summary>
    public static bool EnableDebugPrivilege()
    {
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out IntPtr token)) return false;
        try
        {
            if (!LookupPrivilegeValue(null, "SeDebugPrivilege", out long luid)) return false;
            var tp = new TOKEN_PRIVILEGES { PrivilegeCount = 1, Luid = luid, Attributes = 2 };
            if (!AdjustTokenPrivileges(token, false, ref tp, 16, IntPtr.Zero, IntPtr.Zero)) return false;
            return Marshal.GetLastWin32Error() == 0;
        }
        finally { CloseHandle(token); }
    }

    /// <summary>The raw GetSystemCpuSetInformation buffer (two-phase), empty on failure.</summary>
    public static byte[] CpuSetBuffer()
    {
        try
        {
            GetSystemCpuSetInformation(IntPtr.Zero, 0, out uint len, GetCurrentProcess(), 0);
            if (len == 0 || len > 1 << 22) return Array.Empty<byte>();
            IntPtr buf = Marshal.AllocHGlobal((int)len);
            try
            {
                if (!GetSystemCpuSetInformation(buf, len, out uint got, GetCurrentProcess(), 0)) return Array.Empty<byte>();
                var b = new byte[Math.Min(len, got == 0 ? len : got)];
                Marshal.Copy(buf, b, 0, b.Length);
                return b;
            }
            finally { Marshal.FreeHGlobal(buf); }
        }
        catch (EntryPointNotFoundException) { return Array.Empty<byte>(); }
    }

    /// <summary>The raw GetLogicalProcessorInformationEx(RelationCache) buffer (two-phase), empty on failure.</summary>
    public static byte[] CacheBuffer()
    {
        uint len = 0;
        GetLogicalProcessorInformationEx(2, IntPtr.Zero, ref len);
        if (len == 0 || len > 1 << 22) return Array.Empty<byte>();
        IntPtr buf = Marshal.AllocHGlobal((int)len);
        try
        {
            if (!GetLogicalProcessorInformationEx(2, buf, ref len)) return Array.Empty<byte>();
            var b = new byte[len];
            Marshal.Copy(buf, b, 0, b.Length);
            return b;
        }
        finally { Marshal.FreeHGlobal(buf); }
    }

    static string _cpuName;

    /// <summary>ProcessorNameString of processor 0, cached for the life of the process.</summary>
    public static string CpuName()
    {
        if (_cpuName != null) return _cpuName;
        string name = "";
        try
        {
            using var k = Registry.LocalMachine.OpenSubKey(@"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
            name = (k?.GetValue("ProcessorNameString") as string ?? "").Trim();
        }
        catch (Exception) { name = ""; }
        return _cpuName = name;
    }

    /// <summary>GetActiveProcessorCount(ALL_PROCESSOR_GROUPS), fallback Environment.ProcessorCount.</summary>
    public static int ActiveLpCount()
    {
        try
        {
            uint n = GetActiveProcessorCount(0xFFFF);
            if (n > 0) return (int)n;
        }
        catch (EntryPointNotFoundException) { }
        return Environment.ProcessorCount;
    }

    /// <summary>The thread's name (GetThreadDescription, freed with LocalFree).</summary>
    public static string ThreadDescription(IntPtr thread)
    {
        IntPtr p = IntPtr.Zero;
        try
        {
            if (GetThreadDescription(thread, out p) < 0 || p == IntPtr.Zero) return "";
            return Marshal.PtrToStringUni(p) ?? "";
        }
        catch (EntryPointNotFoundException) { return ""; }
        finally
        {
            if (p != IntPtr.Zero) LocalFree(p);
        }
    }

    /// <summary>Process creation FILETIME, 0 when unknown.</summary>
    public static long ProcessCreated(IntPtr process) => GetProcessTimes(process, out long c, out _, out _, out _) ? c : 0;

    /// <summary>Thread creation FILETIME, 0 when unknown.</summary>
    public static long ThreadCreated(IntPtr thread) => GetThreadTimes(thread, out long c, out _, out _, out _) ? c : 0;

    /// <summary>The image name of a process ("cs2.exe"), "" when unknown.</summary>
    public static string ImageName(uint pid)
    {
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == IntPtr.Zero) return "";
        try
        {
            var buf = new char[1024];
            uint size = (uint)buf.Length;
            if (!QueryFullProcessImageNameW(h, 0, buf, ref size)) return "";
            return Path.GetFileName(new string(buf, 0, (int)size));
        }
        finally { CloseHandle(h); }
    }

    /// <summary>The full image path of a process (for its icon), "" when unknown. Read-only access.</summary>
    public static string ImagePath(uint pid)
    {
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == IntPtr.Zero) return "";
        try
        {
            var buf = new char[1024];
            uint size = (uint)buf.Length;
            return QueryFullProcessImageNameW(h, 0, buf, ref size) ? new string(buf, 0, (int)size) : "";
        }
        finally { CloseHandle(h); }
    }

    /// <summary>Modules of a process (up to 2048), names up to 260 characters.</summary>
    public static List<ModRange> Modules(IntPtr process)
    {
        var list = new List<ModRange>();
        var mods = new IntPtr[2048];
        if (!K32EnumProcessModulesEx(process, mods, (uint)(mods.Length * IntPtr.Size), out uint needed, LIST_MODULES_ALL)) return list;
        int n = Math.Min(mods.Length, (int)(needed / (uint)IntPtr.Size));
        var name = new char[260];
        for (int i = 0; i < n; i++)
        {
            if (!K32GetModuleInformation(process, mods[i], out MODULEINFO mi, (uint)Marshal.SizeOf<MODULEINFO>())) continue;
            uint len = K32GetModuleBaseNameW(process, mods[i], name, (uint)name.Length);
            if (len == 0) continue;
            list.Add(new ModRange { Base = (ulong)mi.lpBaseOfDll.ToInt64(), Size = mi.SizeOfImage, Name = new string(name, 0, (int)len) });
        }
        return list;
    }

    /// <summary>Protected process (PPL / PP).</summary>
    public static bool IsProtected(IntPtr process)
    {
        try { return NtQueryInformationProcess(process, ProcessProtectionInformation, out byte prot, 1, IntPtr.Zero) == 0 && prot != 0; }
        catch (EntryPointNotFoundException) { return false; }
    }

    /// <summary>Every thread of every process, from one CreateToolhelp32Snapshot.</summary>
    public static Dictionary<uint, List<uint>> ThreadsByPid()
    {
        var d = new Dictionary<uint, List<uint>>();
        IntPtr snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if (snap == IntPtr.Zero || snap == new IntPtr(-1)) return d;
        try
        {
            var e = new THREADENTRY32 { dwSize = (uint)Marshal.SizeOf<THREADENTRY32>() };
            if (!Thread32First(snap, ref e)) return d;
            do
            {
                if (!d.TryGetValue(e.th32OwnerProcessID, out var l)) d[e.th32OwnerProcessID] = l = new List<uint>();
                l.Add(e.th32ThreadID);
                e.dwSize = (uint)Marshal.SizeOf<THREADENTRY32>();
            } while (Thread32Next(snap, ref e));
        }
        finally { CloseHandle(snap); }
        return d;
    }

    /// <summary>Every process: pid -> (image name, parent pid), from one snapshot.</summary>
    public static Dictionary<uint, (string Name, uint Parent)> Processes()
    {
        var d = new Dictionary<uint, (string, uint)>();
        IntPtr snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if (snap == IntPtr.Zero || snap == new IntPtr(-1)) return d;
        try
        {
            var e = new PROCESSENTRY32W { dwSize = (uint)Marshal.SizeOf<PROCESSENTRY32W>() };
            if (!Process32FirstW(snap, ref e)) return d;
            do
            {
                d[e.th32ProcessID] = (e.szExeFile ?? "", e.th32ParentProcessID);
                e.dwSize = (uint)Marshal.SizeOf<PROCESSENTRY32W>();
            } while (Process32NextW(snap, ref e));
        }
        finally { CloseHandle(snap); }
        return d;
    }
}
