using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>INative over the Windows API (Native).</summary>
public sealed class WinNative : INative
{
    [StructLayout(LayoutKind.Sequential)]
    struct PowerThrottling
    {
        public int Version;
        public uint ControlMask;
        public uint StateMask;
    }

    public bool EnableDebugPrivilege() => Native.EnableDebugPrivilege();
    public IntPtr OpenProcessGraceful(uint pid, out int level) => Native.OpenProcessGraceful(pid, out level);
    public long ProcessCreated(IntPtr process) => Native.ProcessCreated(process);
    public bool IsProtected(IntPtr process) => Native.IsProtected(process);

    public void Close(IntPtr handle)
    {
        if (handle != IntPtr.Zero) Native.CloseHandle(handle);
    }

    public bool GetPriorityClass(IntPtr process, out uint cls)
    {
        cls = Native.GetPriorityClass(process);
        return cls != 0;
    }

    public bool SetPriorityClass(IntPtr process, uint cls) => Native.SetPriorityClass(process, cls);

    public bool GetAffinity(IntPtr process, out ulong mask)
    {
        bool ok = Native.GetProcessAffinityMask(process, out UIntPtr pm, out _);
        mask = (ulong)pm;
        return ok;
    }

    public bool SetAffinity(IntPtr process, ulong mask) => Native.SetProcessAffinityMask(process, (UIntPtr)mask);

    public bool GetCpuSets(IntPtr process, out uint[] ids)
    {
        ids = Array.Empty<uint>();
        try
        {
            var buf = new uint[512];
            if (!Native.GetProcessDefaultCpuSets(process, buf, (uint)buf.Length, out uint n)) return false;
            ids = buf.Take((int)Math.Min(n, (uint)buf.Length)).ToArray();
            return true;
        }
        catch (EntryPointNotFoundException) { return false; }
    }

    public bool SetCpuSets(IntPtr process, uint[] ids)
    {
        try { return ids == null || ids.Length == 0 ? Native.SetProcessDefaultCpuSets(process, null, 0) : Native.SetProcessDefaultCpuSets(process, ids, (uint)ids.Length); }
        catch (EntryPointNotFoundException) { return false; }
    }

    public bool GetIo(IntPtr process, out uint value)
    {
        bool ok = Native.NtQueryInformationProcess(process, Native.ProcessIoPriority, out value, 4, IntPtr.Zero) == 0;
        if (ok && value > 4) value = 2;
        return ok;
    }

    public bool SetIo(IntPtr process, uint value) => Native.NtSetInformationProcess(process, Native.ProcessIoPriority, ref value, 4) == 0;

    public bool GetMem(IntPtr process, out uint value)
    {
        value = 5;
        IntPtr p = Marshal.AllocHGlobal(4);
        try
        {
            if (!Native.GetProcessInformation(process, Native.ProcessMemoryPriority, p, 4)) return false;
            value = (uint)Marshal.ReadInt32(p);
            return true;
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    public bool SetMem(IntPtr process, uint value)
    {
        IntPtr p = Marshal.AllocHGlobal(4);
        try
        {
            Marshal.WriteInt32(p, (int)value);
            return Native.SetProcessInformation(process, Native.ProcessMemoryPriority, p, 4);
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    public bool GetEco(IntPtr process, out uint control, out uint state)
    {
        control = state = 0;
        IntPtr p = Marshal.AllocHGlobal(12);
        try
        {
            Marshal.StructureToPtr(new PowerThrottling { Version = 1 }, p, false);
            if (!Native.GetProcessInformation(process, Native.ProcessPowerThrottling, p, 12)) return false;
            var s = Marshal.PtrToStructure<PowerThrottling>(p);
            control = s.ControlMask;
            state = s.StateMask;
            return true;
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    public bool SetEco(IntPtr process, uint control, uint state)
    {
        IntPtr p = Marshal.AllocHGlobal(12);
        try
        {
            Marshal.StructureToPtr(new PowerThrottling { Version = 1, ControlMask = control, StateMask = state }, p, false);
            return Native.SetProcessInformation(process, Native.ProcessPowerThrottling, p, 12);
        }
        finally { Marshal.FreeHGlobal(p); }
    }

    public bool GetBoostDisabled(IntPtr process, out bool disabled) => Native.GetProcessPriorityBoost(process, out disabled);
    public bool SetBoostDisabled(IntPtr process, bool disabled) => Native.SetProcessPriorityBoost(process, disabled);
    public List<ModRange> Modules(IntPtr process) => Native.Modules(process);
    public IntPtr OpenThreadOf(uint pid, uint tid) => Native.OpenThreadOf(pid, tid);
    public bool ThreadCycles(IntPtr thread, out ulong cycles) => Native.QueryThreadCycleTime(thread, out cycles);

    public bool ThreadTimes(IntPtr thread, out long created, out ulong kernelUser)
    {
        bool ok = Native.GetThreadTimes(thread, out created, out _, out long k, out long u);
        kernelUser = ok ? (ulong)(k + u) : 0;
        return ok;
    }

    public ulong ThreadStart(IntPtr thread)
    {
        try { return Native.NtQueryInformationThread(thread, Native.ThreadQuerySetWin32StartAddress, out ulong a, 8, IntPtr.Zero) == 0 ? a : 0; }
        catch (EntryPointNotFoundException) { return 0; }
    }

    public string ThreadDesc(IntPtr thread) => Native.ThreadDescription(thread);

    public bool GetThreadPrio(IntPtr thread, out int prio)
    {
        prio = Native.GetThreadPriority(thread);
        return prio != 0x7fffffff;
    }

    public bool SetThreadPrio(IntPtr thread, int prio) => Native.SetThreadPriority(thread, prio);
    public bool GetThreadBoostDisabled(IntPtr thread, out bool disabled) => Native.GetThreadPriorityBoost(thread, out disabled);
    public bool SetThreadBoostDisabled(IntPtr thread, bool disabled) => Native.SetThreadPriorityBoost(thread, disabled);
    public bool SetThreadIdeal(IntPtr thread, uint lp) => Native.SetThreadIdealProcessor(thread, lp) != 0xFFFFFFFF;

    public bool GetThreadIdeal(IntPtr thread, out uint lp)
    {
        // PROCESSOR_NUMBER { WORD Group; BYTE Number; BYTE Reserved } packed into 4 bytes.
        lp = 0;
        if (!Native.GetThreadIdealProcessorEx(thread, out uint pn)) return false;
        lp = (pn >> 16) & 0xFF;
        return (pn & 0xFFFF) == 0;      // group 0 only
    }

    public bool CpuTimes(out long[] idle, out long[] total)
    {
        // SystemProcessorPerformanceInformation (8): 48 bytes per processor — Idle, Kernel (includes idle), User, Dpc, Interrupt, count.
        idle = total = Array.Empty<long>();
        try
        {
            var buf = new byte[48 * 64];
            if (Native.NtQuerySystemInformation(8, buf, buf.Length, out int got) != 0 || got < 48) return false;
            int n = got / 48;
            idle = new long[n];
            total = new long[n];
            for (int i = 0; i < n; i++)
            {
                idle[i] = BitConverter.ToInt64(buf, i * 48);
                total[i] = BitConverter.ToInt64(buf, i * 48 + 8) + BitConverter.ToInt64(buf, i * 48 + 16);
            }
            return true;
        }
        catch (Exception) { return false; }
    }

    public bool GetThreadCpuSets(IntPtr thread, out uint[] ids)
    {
        ids = Array.Empty<uint>();
        try
        {
            var buf = new uint[512];
            if (!Native.GetThreadSelectedCpuSets(thread, buf, (uint)buf.Length, out uint n)) return false;
            ids = buf.Take((int)Math.Min(n, (uint)buf.Length)).ToArray();
            return true;
        }
        catch (EntryPointNotFoundException) { return false; }
    }

    public bool SetThreadCpuSets(IntPtr thread, uint[] ids)
    {
        try { return ids == null || ids.Length == 0 ? Native.SetThreadSelectedCpuSets(thread, null, 0) : Native.SetThreadSelectedCpuSets(thread, ids, (uint)ids.Length); }
        catch (EntryPointNotFoundException) { return false; }
    }

    public bool TimerQuery(out uint coarsest, out uint finest, out uint current)
    {
        // The API names the coarsest interval "minimum resolution" and the finest "maximum resolution".
        coarsest = finest = current = 0;
        try
        {
            if (Native.NtQueryTimerResolution(out uint a, out uint b, out current) != 0) return false;
            coarsest = Math.Max(a, b);
            finest = Math.Min(a, b);
            return finest != 0;
        }
        catch (EntryPointNotFoundException) { return false; }
    }

    public bool TimerSet(uint resolution, bool set, out uint actual)
    {
        actual = 0;
        try { return Native.NtSetTimerResolution(resolution, set, out actual) == 0; }
        catch (EntryPointNotFoundException) { return false; }
    }
    public Dictionary<uint, List<uint>> ThreadsByPid() => Native.ThreadsByPid();
    public Dictionary<uint, (string Name, uint Parent)> Processes() => Native.Processes();
    public int LastError() => Marshal.GetLastWin32Error();
}
