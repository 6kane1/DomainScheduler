using System.Globalization;

namespace DomainScheduler;

/// <summary>One entry of GetSystemCpuSetInformation (Type 0).</summary>
public struct CpuSet
{
    public uint Id;        // system ID for the CPU Set APIs: NOT the logical processor index
    public ushort Group;
    public byte LP, Core, LLC, Numa, Eff, Flags;  // Flags: bit0 Parked, bit1 Allocated, bit2 ToTarget, bit3 RealTime
}

public sealed class L3Cache
{
    public ulong Size;
    public List<(ushort Group, ulong Mask)> Masks = new();
}

public struct ModRange
{
    public ulong Base;
    public ulong Size;
    public string Name;
}

public sealed class Topo
{
    public List<CpuSet> Sets;
    public int LogicalCount, PhysicalCount, Groups, Reported;
    public bool SMT;
    public List<byte> EffClasses, LLCGroups;              // distinct, asc
    public Dictionary<byte, ulong> L3ByLlc = new();
    public byte? BigCacheLlc;
    public string Name = "";
    SplitPlan _split;
    public bool Hybrid => EffClasses.Count > 1;
    public SplitPlan Split => _split ??= CoreSplit.Detect(this);
    public ulong FullMask() => LogicalCount >= 64 ? ulong.MaxValue : (1UL << LogicalCount) - 1;
    public byte FirstCore() => Sets.Min(s => s.Core);
    public static Func<Topo> LiveHook;
}

public sealed class SplitPlan
{
    public string Family = "none", Name = "", Sku = "", Code = "";
    public object[] Args = Array.Empty<object>();
    public ulong Game, NonGame, All, GameL3, NonGameL3;
    public int Probe, Reported, Expected, Cores, PCores, ECores;
    public byte TopEff;
    public bool SmtOn, UpperHalf;
    public List<string> Problems = new();
    public bool IsSplitFamily => Family is "x3d" or "hybrid";
    public bool CanSplit => IsSplitFamily && Problems.Count == 0 && Game != 0 && NonGame != 0;
    public string Label => Family switch { "x3d" => Sku, "hybrid" => L.T("Hybrid (P+E)"), _ => Family };
    public string Why() => Problems.Count > 0
        ? string.Format(L.T("Split problems: {0}"), string.Join(", ", Problems))
        : CoreSplit.Text(Code, Args);
}

public sealed class MaskSpec
{
    public string Base = "auto";
    public int Ccd = -1;
    public bool NoSmt, NoCore0;
    public override string ToString()
    {
        string s = Base == "ccd" ? "ccd" + Ccd.ToString(CultureInfo.InvariantCulture) : Base;
        if (NoSmt) s += "+nosmt";
        if (NoCore0) s += "+nocore0";
        return s;
    }
}

public sealed class MaskResult
{
    public string Name, Hex, LPs, Spec = "";
    public ulong Mask;
    public uint[] SetIds;
    public int Count, Domains;
    public bool Portable = true, Satisfied = true;
    public List<string> Notes = new();
}

public sealed class MaskOption
{
    public string Key, Label;
}

public sealed class GameMaskInfo
{
    public string BaseRule, Mode;
    public ulong Base, Add, Drop, Manual, Final, Irq, IrqDrop;
    public bool IrqIgnored, EditsStale;
    public List<string> Notes = new();
    public List<KeyValuePair<string, ulong>> IrqDevices = new(), IrqWide = new();
    public bool Adjusted => Final != Base;      // computed, not a field
}

public sealed class ThreadRule
{
    public string Match = "module", Pattern = "", Prio = "", Ideal = "", Boost = "", Tid = "";
    public bool Enabled = true;
    /// <summary>Section 22.4: after the ideal processor the thread is also limited to that processor's CPU Set.</summary>
    public bool Pin;
    public ThreadRule Clone() => (ThreadRule)MemberwiseClone();
}

public sealed class Preset
{
    public string Exe = "", Class = "normal", Io = "normal", Mem = "normal", Eco = "off", Boost = "";
    public bool Enabled = true, Builtin;
    /// <summary>Heat policy (section 22.6): "pin" | "ideal" | "none" for the hottest thread and for the next ones.</summary>
    public string HotFirst = "none", HotOthers = "none";
    public List<ThreadRule> Threads = new();
    public Preset Clone()
    {
        var o = (Preset)MemberwiseClone();
        o.Threads = Threads.Select(t => t.Clone()).ToList();
        return o;
    }
}

public sealed class ApplyOptions
{
    public string MaskName, BgMaskName;
    public bool DoMask, DoThreads, DoBackground, BgAffinity, DoAffRules;
    public bool Warmup, Sys, Game, AllowElevatedPriority;
}

public sealed class ApplyReport
{
    public int Pid, HandleLevel, NamedHits, LastThreads;
    public string Exe = "", MaskHex = "", MaskLps = "";
    public List<uint> CycleFallbackTids = new();
    public List<string> Failures = new();
}

public sealed class ThreadRow
{
    public uint Tid;
    public ulong Rate;
    public int Prio;
    public string Sym = "", Desc = "";
    public long Born;
}

public sealed class ThreadSnap
{
    public bool Ok;
    public int Level;
    public List<ThreadRow> Rows = new();
    public List<uint> CycleFallbackTids = new();
}

sealed class ProcBefore
{
    public uint Prio;
    public ulong Aff;
    public uint Io, Mem, EcoCtl, EcoState;
    public bool BoostDisabled;
    public uint[] Sets = Array.Empty<uint>();
    public long Created;
    public bool Have, PrioSet, AffSet, SetsSet, IoSet, MemSet, EcoSet, BoostSet;
}

sealed class ThrBefore
{
    public int Prio;
    public bool BoostDisabled, PrioSet, BoostSet, SetsSet, IdealSet;
    public uint Ideal;                             // the ideal processor before the dynamic mode moved the thread
    public uint[] Sets = Array.Empty<uint>();      // the thread's selected CPU Sets before the program touched them
    public long Born;
}

sealed class ThrPrevE
{
    public ulong C;
    public DateTime T;
}

sealed class ThrMetaE
{
    public string Sym = "", Desc = "";
    public long Born;
    public DateTime Seen;
}

sealed class ModCacheE
{
    public List<ModRange> M = new();
    public DateTime T;
}

struct Pn
{
    public int Id;
    public string Name;
}

public sealed class IrqConfig
{
    public bool Msi;
    public int MsgLimit;
    public ulong? Assign;
    public int? Priority;
}

public sealed class PciDevice
{
    public string Name = "", Pdo = "", Service = "", DriverKey = "", Class = "";   // Class: Display, USB, Net, …
    public uint DevInst;
    public string Location = "";
}
