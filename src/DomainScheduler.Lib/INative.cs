namespace DomainScheduler;

/// <summary>The Windows calls the engine makes (WinNative in the product, a fake in tests).</summary>
public interface INative
{
    bool EnableDebugPrivilege();
    IntPtr OpenProcessGraceful(uint pid, out int level);
    long ProcessCreated(IntPtr process);
    bool IsProtected(IntPtr process);
    void Close(IntPtr handle);

    bool GetPriorityClass(IntPtr process, out uint cls);
    bool SetPriorityClass(IntPtr process, uint cls);
    bool GetAffinity(IntPtr process, out ulong mask);
    bool SetAffinity(IntPtr process, ulong mask);
    bool GetCpuSets(IntPtr process, out uint[] ids);           // empty: none
    bool SetCpuSets(IntPtr process, uint[] ids);               // empty: clear
    bool GetIo(IntPtr process, out uint value);
    bool SetIo(IntPtr process, uint value);
    bool GetMem(IntPtr process, out uint value);
    bool SetMem(IntPtr process, uint value);
    bool GetEco(IntPtr process, out uint control, out uint state);
    bool SetEco(IntPtr process, uint control, uint state);
    bool GetBoostDisabled(IntPtr process, out bool disabled);
    bool SetBoostDisabled(IntPtr process, bool disabled);
    List<ModRange> Modules(IntPtr process);

    IntPtr OpenThreadOf(uint pid, uint tid);
    bool ThreadCycles(IntPtr thread, out ulong cycles);
    bool ThreadTimes(IntPtr thread, out long created, out ulong kernelUser);
    ulong ThreadStart(IntPtr thread);
    string ThreadDesc(IntPtr thread);
    bool GetThreadPrio(IntPtr thread, out int prio);
    bool SetThreadPrio(IntPtr thread, int prio);
    bool GetThreadBoostDisabled(IntPtr thread, out bool disabled);
    bool SetThreadBoostDisabled(IntPtr thread, bool disabled);
    bool SetThreadIdeal(IntPtr thread, uint lp);
    bool GetThreadIdeal(IntPtr thread, out uint lp);
    /// <summary>Per logical processor (group 0): idle and total time in 100 ns units since boot.</summary>
    bool CpuTimes(out long[] idle, out long[] total);
    bool GetThreadCpuSets(IntPtr thread, out uint[] ids);      // empty: none
    bool SetThreadCpuSets(IntPtr thread, uint[] ids);          // empty: clear

    /// <summary>System timer resolution in 100 ns units: the coarsest, the finest and the current one.</summary>
    bool TimerQuery(out uint coarsest, out uint finest, out uint current);
    /// <summary>Requests (set = true) or releases (set = false) a timer resolution for this process.</summary>
    bool TimerSet(uint resolution, bool set, out uint actual);

    /// <summary>Every thread of every process (one toolhelp snapshot).</summary>
    Dictionary<uint, List<uint>> ThreadsByPid();
    /// <summary>Every process: image name and parent.</summary>
    Dictionary<uint, (string Name, uint Parent)> Processes();
    /// <summary>The last Win32 error of the calling thread.</summary>
    int LastError();
}
