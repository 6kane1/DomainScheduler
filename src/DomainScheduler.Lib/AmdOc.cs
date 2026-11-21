using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>
/// The AMD driver's display library (ADL, atiadlxx.dll) as the overclock needs it. Structures travel as byte blocks in
/// the driver's own layout (documented at each call) so the parsing and building in AmdOc can be tested with a fake.
/// A return value of 0 is ADL_OK; negative values are ADL error codes (-8 = not supported; the wrapper returns it too
/// when the driver does not export a function).
/// </summary>
public interface IAdl
{
    /// <summary>Loads atiadlxx.dll and creates a context. False with a reason when the library or a basic call is missing.</summary>
    bool Open(out string error);
    int AdapterCount(out int count);
    /// <summary>ADL2_Adapter_AdapterInfo_Get into count × AmdOc.AdapterInfoSize bytes.</summary>
    int AdapterInfo(byte[] buffer);
    int Active(int adapter, out int status);
    int OverdriveCaps(int adapter, out int supported, out int enabled, out int version);
    int AsicType(int adapter, out int types, out int valids);
    /// <summary>ADL2_Overdrive8_Init_SettingX2_Get: the capability bits and count × 16 bytes (featureID, min, max, default).</summary>
    int Od8Init(int adapter, out int caps, out byte[] list);
    /// <summary>ADL2_Overdrive8_Current_SettingX2_Get: one value per setting.</summary>
    int Od8Current(int adapter, out int[] values);
    /// <summary>ADL2_Overdrive8_Setting_Set with an ADLOD8SetSetting block and room for the ADLOD8CurrentSetting answer.</summary>
    int Od8Set(int adapter, byte[] set, byte[] current);
    /// <summary>ADL2_OverdriveN_CapabilitiesX2_Get (x2 = true, 208 bytes) or ADL2_OverdriveN_Capabilities_Get (164 bytes).</summary>
    int OdnCaps(int adapter, out byte[] caps, out bool x2);
    /// <summary>ADL2_OverdriveN_{System|Memory}Clocks[X2]_{Get|Set} on an ADLODNPerformanceLevels[X2] block.</summary>
    int OdnClocks(int adapter, bool memory, bool set, bool x2, byte[] levels);
    /// <summary>ADL2_OverdriveN_PowerLimit_{Get|Set} on an ADLODNPowerLimitSetting block (12 bytes).</summary>
    int OdnPower(int adapter, bool set, byte[] power);
    int OdnTemperature(int adapter, out int value);
    /// <summary>ADL2_New_QueryPMLogData_Get into an ADLPMLogDataOutput block (4 + 256 × 8 bytes).</summary>
    int PmLog(int adapter, byte[] output);
}

/// <summary>One adjustable value of an AMD card, in the driver's absolute terms: MHz, percent or millivolts.</summary>
public sealed class AmdDomain
{
    public bool Editable;
    /// <summary>What may be written: the driver's range, raised to the clock of the state below and cut by our own rules.</summary>
    public int Min, Max;
    /// <summary>The driver's stock value (the offsets in the profiles count from it) and the value in force now.</summary>
    public int Default, Current;
    /// <summary>The driver's step in the same unit (1 when it reports none).</summary>
    public int Step = 1;
    public int Offset => Current - Default;
}

/// <summary>What the driver reports for the chosen AMD card.</summary>
public sealed class AmdState
{
    /// <summary>8 = Overdrive8 (RX 5000 and newer, Vega 20), 7 = OverdriveN (Polaris RX 400/500, Vega 10).</summary>
    public int Version;
    public int Adapter;
    public string Name = "";
    public AmdDomain Core = new(), Mem = new(), Power = new(), Volt = new();
    // Overdrive8: the capability bits and the per-setting tables (index = ADLOD8SettingId).
    public int Caps;
    public int[] Min = Array.Empty<int>(), Max = Array.Empty<int>(), Def = Array.Empty<int>(), Cur = Array.Empty<int>();
    // OverdriveN: the blocks as read (written back with only the top clock changed), the unit and the flags.
    public bool X2;
    public int Flags, CoreScale = 1, MemScale = 1, PowerDefault;
    public byte[] SysLevels, MemLevels, SysDefault, MemDefault, PowerRaw;
    public int SysCount, MemCount;
    public string Kind => Version == 8 ? "Overdrive8" : "OverdriveN";
}

/// <summary>One adapter as ADL lists it.</summary>
public sealed class AdlAdapter
{
    public int Index, Bus, Vendor;
    public string Name = "";
    public bool Active;
    public int OdSupported, OdEnabled, OdVersion;
    public bool AsicKnown;
    public int AsicTypes;
}

/// <summary>
/// Overclocking for AMD Radeon cards through the AMD driver's own interface (ADL), the counterpart of NvOc:
///  - Overdrive8 (version 8 in ADL2_Overdrive_Caps: RX 5000/6000/7000/9000): the maximum GPU clock, the maximum memory
///    clock, the power limit in percent and (where the driver offers it) the voltage offset — only lowering;
///  - OverdriveN (version 7: Polaris RX 470/480/570/580/590, Vega 56/64): the clock of the TOP performance state of
///    the GPU and of the memory and the power limit; lower states and every voltage are written back as read;
///  - every value inside the range the driver reports, on its step, and inside the same caps as NVIDIA
///    (+300 MHz core, +1500 MHz memory, never below −300 MHz); the profiles store offsets from the driver's stock
///    values (read from the driver every time), so a profile means the same on both makers;
///  - every write is read back; "Reset to stock" uses the driver's own defaults (Overdrive8: the reset flag with the
///    default values; OverdriveN: the default state table read from the driver, power limit back to automatic) and
///    does not depend on anything this program remembers, so it also works after a failed or partial apply.
/// Only active adapters of a physical AMD card are used (one per PCI bus, integrated Radeon graphics left out).
/// </summary>
public static class AmdOc
{
    public const int AdapterInfoSize = 1572, NameAt = 280, NameLen = 256, BusAt = 264, VendorAt = 276;
    public const int Od8SingleSize = 16, Od8SetEntry = 12;
    /// <summary>OD8_COUNT of the ADL SDK headers this code follows; the driver's own count is used when it reports one.</summary>
    public const int Od8Count = 77;
    // ADLOD8SettingId
    public const int GfxMax = 0, GfxMin = 1, UclkMax = 8, PowerPct = 9, UclkMin = 34, PowerMode = 36, OdVoltage = 37;
    // ADLOD8FeatureControl bits
    public const int CapGfxLimits = 1 << 0, CapGfxCurve = 1 << 1, CapUclkMax = 1 << 2, CapPower = 1 << 3, CapPowerMode = 1 << 16, CapVoltage = 1 << 17;
    /// <summary>OD8_OPTIMZED_POWER_MODE value "manual" (from the SDK sample): the driver takes manual values only in it.</summary>
    public const int PowerModeManual = 3;
    // ADLODNFeatureControl bits, ADLODNDPMMaskType, ADLODNControlType
    public const int OdnSclk = 1 << 0, OdnMclk = 1 << 1, OdnPower = 1 << 9, OdnMclkUnderclock = 1 << 14;
    public const int DpmClock = 1 << 0;
    public const int ModeCurrent = 0, ModeDefault = 1, ModeAuto = 2, ModeManual = 3;
    public const int OdnRange = 20, OdnCapsX2 = 208, OdnCaps = 164, PowerSize = 12, PmLogSize = 4 + 256 * 8, PmEdge = 8;

    static int I(byte[] b, int at) => BitConverter.ToInt32(b, at);
    static void W(byte[] b, int at, int v) => BitConverter.GetBytes(v).CopyTo(b, at);

    static readonly object _gate = new();
    static IAdl _native;
    static bool _tried, _changed;
    static AdlAdapter _adapter;
    public static string Error { get; private set; } = "";
    public static string Name => _adapter?.Name ?? "";

    /// <summary>The driver interface in use (the real one unless a test put in a fake).</summary>
    public static IAdl Native
    {
        get { lock (_gate) return _native ??= new AdlNative(); }
    }

    /// <summary>Test hook: a fake driver; forgets the chosen card.</summary>
    public static void Use(IAdl native)
    {
        lock (_gate)
        {
            _native = native;
            _tried = false;
            _changed = false;
            _adapter = null;
            Error = "";
        }
    }

    // ---- choosing the card ---------------------------------------------------------------------------

    /// <summary>The adapters of the AdapterInfo block (count × 1572 bytes): index, bus, vendor and name.</summary>
    public static List<AdlAdapter> ParseAdapters(byte[] info, int count)
    {
        var list = new List<AdlAdapter>();
        if (info == null) return list;
        for (int i = 0; i < count && (i + 1) * AdapterInfoSize <= info.Length; i++)
        {
            int at = i * AdapterInfoSize;
            var name = System.Text.Encoding.ASCII.GetString(info, at + NameAt, NameLen);
            int z = name.IndexOf('\0');
            list.Add(new AdlAdapter
            {
                Index = I(info, at + 4), Bus = I(info, at + BusAt), Vendor = I(info, at + VendorAt),
                Name = (z >= 0 ? name[..z] : name).Trim(),
            });
        }
        return list;
    }

    /// <summary>
    /// The card to work on: an active adapter of a physical AMD GPU (vendor 1002, a PCI bus), one per bus, not an
    /// integrated Radeon, whose driver reports Overdrive8 or OverdriveN. The lowest bus wins. Null with a reason otherwise.
    /// </summary>
    public static (AdlAdapter Adapter, string Why) Pick(IEnumerable<AdlAdapter> all)
    {
        var amd = (all ?? Enumerable.Empty<AdlAdapter>())
            .Where(a => (a.Vendor == 1002 || a.Vendor == 0x1002) && a.Bus > -1 && a.Active)
            .GroupBy(a => a.Bus).Select(g => g.OrderBy(a => a.Index).First())
            .Where(a => !a.AsicKnown || (a.AsicTypes & 1) != 0 || (a.AsicTypes & 2) == 0)   // ADL_ASIC_DISCRETE / _INTEGRATED
            .OrderBy(a => a.Bus).ToList();
        if (amd.Count == 0) return (null, L.T("No active AMD Radeon graphics card was found"));
        var od = amd.FirstOrDefault(a => a.OdSupported != 0 && (a.OdVersion == 7 || a.OdVersion == 8));
        if (od != null) return (od, "");
        var first = amd[0];
        return (null, first.OdSupported == 0
            ? L.T("The AMD driver reports no overclocking support for this card")
            : L.F("The AMD driver offers an older overclocking interface (version {0}) that is not supported", first.OdVersion));
    }

    /// <summary>Finds the AMD card. False (with Error) when there is none or the driver does not answer.</summary>
    public static bool Open()
    {
        lock (_gate)
        {
            if (_adapter != null) return true;
            if (_tried) return false;
            _tried = true;
            try
            {
                var n = Native;
                if (!n.Open(out string why)) { Error = why; return false; }
                if (n.AdapterCount(out int count) != 0 || count < 1 || count > 256) { Error = L.T("No active AMD Radeon graphics card was found"); return false; }
                var info = new byte[count * AdapterInfoSize];
                if (n.AdapterInfo(info) != 0) { Error = L.T("The AMD driver did not list its graphics cards"); return false; }
                var all = ParseAdapters(info, count);
                foreach (var a in all)
                {
                    if (!(a.Vendor == 1002 || a.Vendor == 0x1002) || a.Bus < 0) continue;
                    a.Active = n.Active(a.Index, out int st) == 0 && st != 0;
                    if (!a.Active) continue;
                    if (n.OverdriveCaps(a.Index, out int sup, out int en, out int ver) == 0) { a.OdSupported = sup; a.OdEnabled = en; a.OdVersion = ver; }
                    if (n.AsicType(a.Index, out int types, out int valids) == 0) { a.AsicKnown = true; a.AsicTypes = valids != 0 ? types & valids : types; }
                }
                var (pick, reason) = Pick(all);
                if (pick == null) { Error = reason; return false; }
                _adapter = pick;
                Error = "";
                return true;
            }
            catch (Exception e) { Error = e.Message; return false; }
        }
    }

    // ---- the pure part: ranges, targets, blocks -------------------------------------------------------

    /// <summary>A domain as the NVIDIA clock shape the sliders use: stock, offset and the allowed offsets in kHz.</summary>
    public static NvClock AsClock(AmdDomain d, int domain)
    {
        if (d == null) return null;
        return new NvClock
        {
            Domain = domain, Editable = d.Editable, BaseKHz = d.Default * 1000, DeltaKHz = (d.Current - d.Default) * 1000,
            MinDeltaKHz = Math.Min(0, d.Min - d.Default) * 1000, MaxDeltaKHz = Math.Max(0, d.Max - d.Default) * 1000,
        };
    }

    /// <summary>
    /// The adjustable core and memory clocks in the NVIDIA shape (domains 0 and 4; a clock the driver does not let us
    /// change is left out, so the page says so instead of showing it); null for a null state.
    /// </summary>
    public static List<NvClock> Clocks(AmdState s) =>
        s == null ? null : new[] { (s.Core, NvOc.DomainCore), (s.Mem, NvOc.DomainMemory) }
            .Where(x => x.Item1.Editable).Select(x => AsClock(x.Item1, x.Item2)).ToList();

    /// <summary>
    /// The absolute value that will be written for an offset: the offset cut by our caps (clocks: NvOc's), the result
    /// inside the domain's range and on the driver's step counted from the range's low end (rounded inward).
    /// </summary>
    public static int Target(AmdDomain d, int offset, int capLo, int capHi)
    {
        long off = Math.Clamp((long)offset, Math.Min(0, capLo), Math.Max(0, capHi));
        int lo = Math.Min(d.Min, d.Default), hi = Math.Max(d.Max, d.Default);
        int v = (int)Math.Clamp(d.Default + off, lo, hi);
        if (v == d.Default) return v;          // stock is always a valid value, whatever the grid
        int step = Math.Max(1, d.Step);
        // On the driver's grid (counted from the low end of the range), rounded towards stock: never further than asked.
        int r = v > d.Default
            ? lo + (v - lo) / step * step
            : lo + (v - lo + step - 1) / step * step;
        if (v > d.Default && r < d.Default) r = d.Default;
        if (v < d.Default && r > d.Default) r = d.Default;
        return Math.Clamp(r, lo, hi);
    }

    public static int CoreTarget(AmdDomain d, int offMHz) => Target(d, offMHz, NvOc.FloorKHz / 1000, NvOc.CapCoreKHz / 1000);
    public static int MemTarget(AmdDomain d, int offMHz) => Target(d, offMHz, NvOc.FloorKHz / 1000, NvOc.CapMemoryKHz / 1000);
    public static int PowerTarget(AmdDomain d, int offPct) => Target(d, offPct, int.MinValue / 2, int.MaxValue / 2);
    /// <summary>The voltage offset only goes down (undervolt): raising it is never written.</summary>
    public static int VoltTarget(AmdDomain d, int offMv) => Target(d, Math.Min(0, offMv), int.MinValue / 2, 0);

    /// <summary>The Overdrive8 domains from the driver's tables (index = setting id).</summary>
    public static void FillOd8(AmdState s)
    {
        int n = Math.Min(s.Cur.Length, s.Def.Length);
        bool Has(int id) => id < n && s.Min[id] <= s.Max[id];
        bool In(int id, int v) => Has(id) && v >= s.Min[id] && v <= s.Max[id];
        AmdDomain Dom(int id, int cap, int lo, int hi)
        {
            var d = new AmdDomain();
            if ((s.Caps & cap) == 0 || !Has(id)) return d;
            d.Min = s.Min[id]; d.Max = s.Max[id]; d.Default = s.Def[id]; d.Current = s.Cur[id];
            // Plausible values only: the stock value inside the reported range, and the range inside what a card can be.
            d.Editable = d.Default >= d.Min && d.Default <= d.Max && d.Min >= lo && d.Max <= hi;
            return d;
        }
        s.Core = Dom(GfxMax, CapGfxLimits, 100, 10_000);
        s.Mem = Dom(UclkMax, CapUclkMax, 50, 20_000);
        s.Power = Dom(PowerPct, CapPower, -100, 300);
        s.Volt = Dom(OdVoltage, CapVoltage, -1000, 1000);
        // The maximum clock stays at or above the minimum clock in force.
        if (s.Core.Editable && In(GfxMin, s.Cur[GfxMin])) s.Core.Min = Math.Min(Math.Max(s.Core.Min, s.Cur[GfxMin]), s.Core.Default);
        if (s.Mem.Editable && In(UclkMin, s.Cur[UclkMin])) s.Mem.Min = Math.Min(Math.Max(s.Mem.Min, s.Cur[UclkMin]), s.Mem.Default);
        if (s.Volt.Editable) s.Volt.Max = Math.Min(s.Volt.Max, s.Volt.Default);
    }

    /// <summary>An ADLOD8SetSetting block: count, then per setting (value, requested, reset).</summary>
    public static byte[] BuildOd8Set(int count, IEnumerable<(int Id, int Value, bool Reset)> items)
    {
        var b = new byte[4 + Math.Max(count, 1) * Od8SetEntry];
        W(b, 0, count);
        foreach (var (id, value, reset) in items)
        {
            if (id < 0 || id >= count) continue;
            int at = 4 + id * Od8SetEntry;
            W(b, at, value);
            W(b, at + 4, 1);
            W(b, at + 8, reset ? 1 : 0);
        }
        return b;
    }

    /// <summary>
    /// The settings one Overdrive8 apply requests: each changed domain with its target, and the minimum clocks that pair
    /// with a changed maximum with their values in force (as the SDK sample does). Unchanged domains are not sent.
    /// </summary>
    public static List<(int Id, int Value, bool Reset)> Od8Items(AmdState s, int core, int mem, int power, int volt)
    {
        var list = new List<(int, int, bool)>();
        int n = s.Cur.Length;
        bool Keep(int id) => id < n && id < s.Min.Length && s.Cur[id] >= s.Min[id] && s.Cur[id] <= s.Max[id];
        if (s.Core.Editable && core != s.Core.Current)
        {
            list.Add((GfxMax, core, false));
            if (Keep(GfxMin)) list.Add((GfxMin, s.Cur[GfxMin], false));
        }
        if (s.Mem.Editable && mem != s.Mem.Current)
        {
            list.Add((UclkMax, mem, false));
            if (Keep(UclkMin) && s.Max[UclkMin] > 0) list.Add((UclkMin, s.Cur[UclkMin], false));
        }
        if (s.Power.Editable && power != s.Power.Current) list.Add((PowerPct, power, false));
        if (s.Volt.Editable && volt != s.Volt.Current) list.Add((OdVoltage, volt, false));
        return list;
    }

    /// <summary>Overdrive8 back to stock: every setting this program writes, with the reset flag and the default value.</summary>
    public static List<(int Id, int Value, bool Reset)> Od8ResetItems(AmdState s)
    {
        var list = new List<(int, int, bool)>();
        int n = Math.Min(s.Def.Length, s.Cur.Length);
        void Add(int id, int cap) { if ((s.Caps & cap) != 0 && id < n) list.Add((id, s.Def[id], true)); }
        Add(GfxMax, CapGfxLimits);
        Add(GfxMin, CapGfxLimits);
        Add(UclkMax, CapUclkMax);
        if (UclkMin < n && s.Max[UclkMin] > 0) Add(UclkMin, CapUclkMax);
        Add(PowerPct, CapPower);
        Add(OdVoltage, CapVoltage);
        Add(PowerMode, CapPowerMode);
        return list;
    }

    /// <summary>The power-mode switch to "manual" Overdrive8 needs before manual values count; null when not needed.</summary>
    public static List<(int Id, int Value, bool Reset)> Od8ManualMode(AmdState s)
    {
        if ((s.Caps & CapPowerMode) == 0 || PowerMode >= s.Cur.Length || s.Cur[PowerMode] == PowerModeManual) return null;
        if (PowerModeManual < s.Min[PowerMode] || PowerModeManual > s.Max[PowerMode]) return null;
        return new List<(int, int, bool)> { (PowerMode, PowerModeManual, false) };
    }

    /// <summary>One ADLODNParameterRange of a capabilities block: (min, max, step, default).</summary>
    public static (int Min, int Max, int Step, int Def) OdnRangeAt(byte[] caps, bool x2, int k)
    {
        int at = (x2 ? 8 : 4) + k * OdnRange;
        return (I(caps, at + 4), I(caps, at + 8), I(caps, at + 12), I(caps, at + 16));
    }

    public static int LevelSize(bool x2) => x2 ? 16 : 12;
    public static int LevelsBytes(bool x2, int count) => 12 + LevelSize(x2) * count;

    /// <summary>An empty ADLODNPerformanceLevels[X2] block asking for the given mode.</summary>
    public static byte[] NewLevels(bool x2, int count, int mode)
    {
        var b = new byte[LevelsBytes(x2, count)];
        W(b, 0, b.Length);
        W(b, 4, mode);
        W(b, 8, count);
        return b;
    }

    public static int Clock(byte[] levels, bool x2, int i) => I(levels, 12 + i * LevelSize(x2));
    public static int Vddc(byte[] levels, bool x2, int i) => I(levels, 12 + i * LevelSize(x2) + 4);
    public static int Control(byte[] levels, bool x2, int i) => x2 ? I(levels, 12 + i * 16 + 12) : DpmClock;

    /// <summary>The unit of OverdriveN clocks: values above 10000 are 10 kHz units (Wattman's own), else MHz.</summary>
    public static int Scale(int rangeMax) => rangeMax > 10_000 ? 100 : 1;

    /// <summary>One OverdriveN clock domain: the top state of the levels, the range of the capabilities.</summary>
    public static AmdDomain OdnClock(byte[] cur, byte[] def, int count, bool x2, (int Min, int Max, int Step, int Def) range, int scale, bool feature, bool floorAtStock)
    {
        var d = new AmdDomain();
        if (cur == null || count < 1) return d;
        int top = count - 1;
        d.Min = range.Min / scale;
        d.Max = range.Max / scale;
        d.Step = Math.Max(1, range.Step / scale);
        d.Current = Clock(cur, x2, top) / scale;
        int stock = def != null ? Clock(def, x2, top) : 0;
        if (stock < range.Min || stock > range.Max) stock = range.Def;          // the range's own default when the table is empty
        d.Default = stock / scale;
        bool plausible = range.Min > 0 && range.Min < range.Max && stock >= range.Min && stock <= range.Max && d.Max <= 20_000 && d.Default >= 100;
        d.Editable = feature && plausible && (Control(cur, x2, top) & DpmClock) != 0;
        if (!d.Editable) return d;
        // The top state never goes below the state under it; memory never below stock unless the driver allows underclocking.
        if (top > 0) d.Min = Math.Max(d.Min, Clock(cur, x2, top - 1) / scale);
        if (floorAtStock) d.Min = Math.Max(d.Min, d.Default);
        d.Min = Math.Min(d.Min, d.Default);
        return d;
    }

    /// <summary>
    /// The levels block to write for a new top clock: the block as read, mode "manual", each state's enable flag as
    /// 0/1 (as the SDK sample does), the top clock changed; every other clock and every voltage as read.
    /// </summary>
    public static byte[] OdnWrite(byte[] cur, bool x2, int count, int topClockRaw)
    {
        var b = (byte[])cur.Clone();
        W(b, 4, ModeManual);
        for (int i = 0; i < count; i++)
        {
            int at = 12 + i * LevelSize(x2) + 8;
            W(b, at, I(b, at) > 0 ? 1 : 0);
        }
        W(b, 12 + (count - 1) * LevelSize(x2), topClockRaw);
        return b;
    }

    /// <summary>
    /// The voltages written back must be what the driver reports: inside its voltage range or equal to its stock table.
    /// False when a state's voltage is neither (then nothing is written).
    /// </summary>
    public static bool VoltagesSafe(byte[] cur, byte[] def, bool x2, int count, (int Min, int Max, int Step, int Def) vddc)
    {
        for (int i = 0; i < count; i++)
        {
            int v = Vddc(cur, x2, i);
            bool inRange = vddc.Min <= vddc.Max && v >= vddc.Min && v <= vddc.Max;
            bool stock = def != null && Vddc(def, x2, i) == v;
            if (!inRange && !stock) return false;
        }
        return true;
    }

    // ---- reading ------------------------------------------------------------------------------------

    /// <summary>The clocks, power limit and voltage as the driver reports them now; null (with Error) when they cannot be read.</summary>
    public static AmdState Read()
    {
        lock (_gate)
        {
            if (!Open()) return null;
            try
            {
                var s = new AmdState { Version = _adapter.OdVersion, Adapter = _adapter.Index, Name = _adapter.Name };
                bool ok = s.Version == 8 ? ReadOd8(s) : ReadOdn(s);
                if (!ok) return null;
                if (!s.Core.Editable && !s.Mem.Editable && !s.Power.Editable && !s.Volt.Editable)
                {
                    Error = L.T("The AMD driver reports no adjustable clocks or limits for this card");
                    return null;
                }
                return s;
            }
            catch (Exception e) { Error = e.Message; return null; }
        }
    }

    static bool ReadOd8(AmdState s)
    {
        var n = Native;
        int rc = n.Od8Init(s.Adapter, out int caps, out byte[] list);
        if (rc != 0 || list == null || list.Length < Od8SingleSize) { Error = L.F("The AMD driver did not report its overclocking ranges: {0}", StatusText(rc)); return false; }
        rc = n.Od8Current(s.Adapter, out int[] cur);
        if (rc != 0 || cur == null || cur.Length == 0) { Error = L.F("The AMD driver did not report its current settings: {0}", StatusText(rc)); return false; }
        int count = Math.Min(list.Length / Od8SingleSize, cur.Length);
        s.Caps = caps;
        s.Min = new int[count]; s.Max = new int[count]; s.Def = new int[count]; s.Cur = cur.Take(count).ToArray();
        for (int i = 0; i < count; i++)
        {
            s.Min[i] = I(list, i * 16 + 4);
            s.Max[i] = I(list, i * 16 + 8);
            s.Def[i] = I(list, i * 16 + 12);
        }
        FillOd8(s);
        return true;
    }

    static bool ReadOdn(AmdState s)
    {
        var n = Native;
        int rc = n.OdnCaps(s.Adapter, out byte[] caps, out bool x2);
        if (rc != 0 || caps == null || caps.Length < (x2 ? OdnCapsX2 : OdnCaps)) { Error = L.F("The AMD driver did not report its overclocking ranges: {0}", StatusText(rc)); return false; }
        int levels = I(caps, 0);
        if (levels < 1 || levels > 16) { Error = L.T("The AMD driver's clock table is not in the expected form: overclocking is not offered for this card"); return false; }
        s.X2 = x2;
        s.Flags = x2 ? I(caps, 4) : OdnSclk | OdnMclk | OdnPower;
        var eng = OdnRangeAt(caps, x2, 0);
        var mem = OdnRangeAt(caps, x2, 1);
        var vddc = OdnRangeAt(caps, x2, 2);
        var pow = OdnRangeAt(caps, x2, 3);
        s.CoreScale = Scale(eng.Max);
        s.MemScale = Scale(mem.Max);

        (byte[] Cur, byte[] Def, int Count) Levels(bool memory)
        {
            var cur = NewLevels(x2, levels, ModeCurrent);
            if (n.OdnClocks(s.Adapter, memory, false, x2, cur) != 0) return (null, null, 0);
            int c = Math.Clamp(I(cur, 8), 0, levels);
            var def = NewLevels(x2, levels, ModeDefault);
            if (n.OdnClocks(s.Adapter, memory, false, x2, def) != 0 || I(def, 8) != c) def = null;
            return (cur, def, c);
        }
        var sys = Levels(false);
        var mm = Levels(true);
        s.SysLevels = sys.Cur; s.SysDefault = sys.Def; s.SysCount = sys.Count;
        s.MemLevels = mm.Cur; s.MemDefault = mm.Def; s.MemCount = mm.Count;
        bool underclock = x2 && (s.Flags & OdnMclkUnderclock) != 0;
        s.Core = OdnClock(sys.Cur, sys.Def, sys.Count, x2, eng, s.CoreScale, (s.Flags & OdnSclk) != 0, false);
        s.Mem = OdnClock(mm.Cur, mm.Def, mm.Count, x2, mem, s.MemScale, (s.Flags & OdnMclk) != 0, !underclock);
        // Nothing is written when the voltages that would go back with the clocks are not what the driver reports.
        if (s.Core.Editable && !VoltagesSafe(sys.Cur, sys.Def, x2, sys.Count, vddc)) s.Core.Editable = false;
        if (s.Mem.Editable && !VoltagesSafe(mm.Cur, mm.Def, x2, mm.Count, vddc)) s.Mem.Editable = false;

        var p = new byte[PowerSize];
        if ((s.Flags & OdnPower) != 0 && n.OdnPower(s.Adapter, false, p) == 0 && pow.Min <= pow.Max && pow.Min >= -100 && pow.Max <= 300)
        {
            s.PowerRaw = p;
            s.PowerDefault = pow.Def;
            s.Power = new AmdDomain
            {
                Min = pow.Min, Max = pow.Max, Step = Math.Max(1, pow.Step), Current = I(p, 4),
                Default = pow.Def >= pow.Min && pow.Def <= pow.Max ? pow.Def : 0,
            };
            s.Power.Editable = s.Power.Default >= pow.Min && s.Power.Default <= pow.Max;
        }
        return true;
    }

    /// <summary>ADL status codes in words.</summary>
    public static string StatusText(int rc) => rc switch
    {
        -8 or -22 => L.T("this graphics card or driver does not support it"),
        -23 => L.T("administrator rights are needed"),
        -10 => L.T("the graphics card is disabled"),
        -3 or -4 => L.T("the driver refused the values"),
        _ => L.F("driver code {0}", rc),
    };

    // ---- writing ------------------------------------------------------------------------------------

    /// <summary>
    /// Writes the offsets (from the driver's stock values: core and memory MHz, power limit percent, voltage mV, only
    /// downwards) and reads back. Returns the state in force after the write, or null; Error says what failed or differs.
    /// </summary>
    public static AmdState Apply(int coreOffMHz, int memOffMHz, int powerOffPct, int voltOffMv)
    {
        lock (_gate)
        {
            if (Flags.Refuse(Flags.GpuOc, L.T("GPU overclock"))) { Error = L.T("The feature flag gpu-oc is closed"); return null; }
            var s = Read();
            if (s == null) return null;
            int core = s.Core.Editable ? CoreTarget(s.Core, coreOffMHz) : s.Core.Current;
            int mem = s.Mem.Editable ? MemTarget(s.Mem, memOffMHz) : s.Mem.Current;
            int power = s.Power.Editable ? PowerTarget(s.Power, powerOffPct) : s.Power.Current;
            int volt = s.Volt.Editable ? VoltTarget(s.Volt, voltOffMv) : s.Volt.Current;
            var errors = new List<string>();
            try
            {
                if (s.Version == 8) WriteOd8(s, core, mem, power, volt, errors);
                else WriteOdn(s, core, mem, power, errors);
            }
            catch (Exception e) { errors.Add(e.Message); }
            var after = Read();
            if (after == null) { Error = errors.Count > 0 ? string.Join("; ", errors) : Error; return null; }
            if (errors.Count == 0)
            {
                bool same = (!s.Core.Editable || after.Core.Current == core) && (!s.Mem.Editable || after.Mem.Current == mem)
                    && (!s.Power.Editable || after.Power.Current == power) && (!s.Volt.Editable || after.Volt.Current == volt);
                if (!same) errors.Add(L.T("The values read back differ from the request"));
            }
            Error = string.Join("; ", errors);
            return after;
        }
    }

    static void WriteOd8(AmdState s, int core, int mem, int power, int volt, List<string> errors)
    {
        var items = Od8Items(s, core, mem, power, volt);
        if (items.Count == 0) return;
        var n = Native;
        int count = s.Cur.Length;
        _changed = true;
        var mode = Od8ManualMode(s);
        if (mode != null)
        {
            int rm = n.Od8Set(s.Adapter, BuildOd8Set(count, mode), new byte[4 + count * 4]);
            if (rm != 0) { errors.Add(L.F("The driver refused the manual tuning mode: {0}", StatusText(rm))); return; }
        }
        int rc = n.Od8Set(s.Adapter, BuildOd8Set(count, items), new byte[4 + count * 4]);
        if (rc != 0) errors.Add(L.F("The driver refused the values: {0}", StatusText(rc)));
    }

    static void WriteOdn(AmdState s, int core, int mem, int power, List<string> errors)
    {
        var n = Native;
        if (s.Core.Editable && core != s.Core.Current)
        {
            _changed = true;
            int rc = n.OdnClocks(s.Adapter, false, true, s.X2, OdnWrite(s.SysLevels, s.X2, s.SysCount, core * s.CoreScale));
            if (rc != 0) errors.Add(L.F("Core clock: {0}", StatusText(rc)));
        }
        if (s.Mem.Editable && mem != s.Mem.Current)
        {
            _changed = true;
            int rc = n.OdnClocks(s.Adapter, true, true, s.X2, OdnWrite(s.MemLevels, s.X2, s.MemCount, mem * s.MemScale));
            if (rc != 0) errors.Add(L.F("Memory clock: {0}", StatusText(rc)));
        }
        if (s.Power.Editable && power != s.Power.Current && s.PowerRaw != null)
        {
            _changed = true;
            var p = (byte[])s.PowerRaw.Clone();
            W(p, 0, ModeManual);
            W(p, 4, power);
            int rc = n.OdnPower(s.Adapter, true, p);
            if (rc != 0) errors.Add(L.F("Power limit: {0}", StatusText(rc)));
        }
    }

    /// <summary>
    /// Clocks, power limit and voltage back to the driver's stock values. Only on the user's "Reset" (never by itself at
    /// exit). Reads everything fresh from the driver, so it works after a failed apply and after a restart of the program.
    /// </summary>
    public static bool Reset(bool force = false)
    {
        lock (_gate)
        {
            if (!_changed && !force) return false;
            if (!Open()) return false;
            var n = Native;
            bool ok = true;
            var errors = new List<string>();
            try
            {
                if (_adapter.OdVersion == 8)
                {
                    int rc = n.Od8Init(_adapter.Index, out int caps, out byte[] list);
                    int rc2 = n.Od8Current(_adapter.Index, out int[] cur);
                    if (rc != 0 || rc2 != 0 || list == null || cur == null) { errors.Add(StatusText(rc != 0 ? rc : rc2)); ok = false; }
                    else
                    {
                        int count = Math.Min(list.Length / Od8SingleSize, cur.Length);
                        var s = new AmdState { Caps = caps, Cur = cur.Take(count).ToArray(), Min = new int[count], Max = new int[count], Def = new int[count] };
                        for (int i = 0; i < count; i++) { s.Min[i] = I(list, i * 16 + 4); s.Max[i] = I(list, i * 16 + 8); s.Def[i] = I(list, i * 16 + 12); }
                        int rs = n.Od8Set(_adapter.Index, BuildOd8Set(count, Od8ResetItems(s)), new byte[4 + count * 4]);
                        if (rs != 0) { errors.Add(StatusText(rs)); ok = false; }
                    }
                }
                else
                {
                    int rc = n.OdnCaps(_adapter.Index, out byte[] caps, out bool x2);
                    if (rc != 0 || caps == null) { errors.Add(StatusText(rc)); ok = false; }
                    else
                    {
                        int levels = Math.Clamp(I(caps, 0), 1, 16);
                        foreach (bool memory in new[] { false, true })
                        {
                            // The driver's own stock table, written back as it is (SDK sample: Get with Default, then Set).
                            var def = NewLevels(x2, levels, ModeDefault);
                            int r = n.OdnClocks(_adapter.Index, memory, false, x2, def);
                            if (r == 0) { W(def, 4, ModeDefault); r = n.OdnClocks(_adapter.Index, memory, true, x2, def); }
                            if (r != 0) { errors.Add((memory ? L.T("Memory clock") : L.T("Core clock")) + ": " + StatusText(r)); ok = false; }
                        }
                        var pow = OdnRangeAt(caps, x2, 3);
                        var p = new byte[PowerSize];
                        if ((!x2 || (I(caps, 4) & OdnPower) != 0) && n.OdnPower(_adapter.Index, false, p) == 0)
                        {
                            W(p, 0, ModeAuto);
                            if (pow.Def >= pow.Min && pow.Def <= pow.Max) W(p, 4, pow.Def);
                            int r = n.OdnPower(_adapter.Index, true, p);
                            if (r != 0) { errors.Add(L.T("Power limit") + ": " + StatusText(r)); ok = false; }
                        }
                    }
                }
            }
            catch (Exception e) { errors.Add(e.Message); ok = false; }
            _changed = false;
            Error = string.Join("; ", errors);
            return ok;
        }
    }

    /// <summary>The GPU (edge) temperature in °C, or -1 when the driver does not say.</summary>
    public static int Temperature()
    {
        lock (_gate)
        {
            try
            {
                if (_adapter == null) return -1;
                int t;
                if (_adapter.OdVersion == 8)
                {
                    var b = new byte[PmLogSize];
                    W(b, 0, PmLogSize);
                    if (Native.PmLog(_adapter.Index, b) != 0 || I(b, 4 + PmEdge * 8) == 0) return -1;
                    t = I(b, 4 + PmEdge * 8 + 4);
                }
                else
                {
                    if (Native.OdnTemperature(_adapter.Index, out t) != 0) return -1;
                    if (t > 1000) t /= 1000;          // OverdriveN reports millidegrees
                }
                return t is > 0 and < 130 ? t : -1;
            }
            catch (Exception) { return -1; }
        }
    }
}

/// <summary>
/// The real ADL: atiadlxx.dll of the AMD driver, loaded on first use, functions looked up by name. Every structure is a
/// byte block in the layout of the ADL SDK headers (adl_structures.h); memory the driver allocates through our callback
/// is copied and freed here.
/// </summary>
public sealed class AdlNative : IAdl
{
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate IntPtr AllocFn(int size);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int CreateFn(AllocFn alloc, int enumConnected, out IntPtr context);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int CountFn(IntPtr ctx, out int count);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int InfoFn(IntPtr ctx, [In, Out] byte[] info, int size);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Out1Fn(IntPtr ctx, int adapter, out int a);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Out2Fn(IntPtr ctx, int adapter, out int a, out int b);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Out3Fn(IntPtr ctx, int adapter, out int a, out int b, out int c);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Od8InitFn(IntPtr ctx, int adapter, out int caps, ref int count, ref IntPtr list);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Od8CurFn(IntPtr ctx, int adapter, ref int count, ref IntPtr list);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Od8SetFn(IntPtr ctx, int adapter, [In, Out] byte[] set, [In, Out] byte[] current);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int BlockFn(IntPtr ctx, int adapter, [In, Out] byte[] block);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int TempFn(IntPtr ctx, int adapter, int type, out int value);

    // Kept for the life of the process: the driver calls it whenever it hands memory back.
    static readonly AllocFn Alloc = size => Marshal.AllocHGlobal(Math.Max(size, 1));
    IntPtr _lib, _ctx;

    T Fn<T>(string name) where T : Delegate =>
        _lib != IntPtr.Zero && NativeLibrary.TryGetExport(_lib, name, out var p) ? Marshal.GetDelegateForFunctionPointer<T>(p) : null;

    public bool Open(out string error)
    {
        error = "";
        if (_ctx != IntPtr.Zero) return true;
        try
        {
            if (!OperatingSystem.IsWindows() || !NativeLibrary.TryLoad("atiadlxx.dll", out _lib))
            {
                error = L.T("No AMD driver is installed (its interface atiadlxx.dll was not found)");
                return false;
            }
            var create = Fn<CreateFn>("ADL2_Main_Control_Create");
            if (create == null) { error = L.T("The AMD driver does not offer the needed functions"); return false; }
            int rc = create(Alloc, 1, out _ctx);
            if (rc != 0 || _ctx == IntPtr.Zero) { _ctx = IntPtr.Zero; error = L.T("The AMD driver did not start its interface"); return false; }
            return true;
        }
        catch (Exception e) { error = e.Message; return false; }
    }

    public int AdapterCount(out int count)
    {
        count = 0;
        var f = Fn<CountFn>("ADL2_Adapter_NumberOfAdapters_Get");
        return f == null ? -8 : f(_ctx, out count);
    }

    public int AdapterInfo(byte[] buffer)
    {
        var f = Fn<InfoFn>("ADL2_Adapter_AdapterInfo_Get");
        return f == null ? -8 : f(_ctx, buffer, buffer.Length);
    }

    public int Active(int adapter, out int status)
    {
        status = 0;
        var f = Fn<Out1Fn>("ADL2_Adapter_Active_Get");
        return f == null ? -8 : f(_ctx, adapter, out status);
    }

    public int OverdriveCaps(int adapter, out int supported, out int enabled, out int version)
    {
        supported = enabled = version = 0;
        var f = Fn<Out3Fn>("ADL2_Overdrive_Caps");
        return f == null ? -8 : f(_ctx, adapter, out supported, out enabled, out version);
    }

    public int AsicType(int adapter, out int types, out int valids)
    {
        types = valids = 0;
        var f = Fn<Out2Fn>("ADL2_Adapter_ASICFamilyType_Get");
        return f == null ? -8 : f(_ctx, adapter, out types, out valids);
    }

    public int Od8Init(int adapter, out int caps, out byte[] list)
    {
        caps = 0;
        list = null;
        var f = Fn<Od8InitFn>("ADL2_Overdrive8_Init_SettingX2_Get");
        if (f == null) return -8;
        int count = AmdOc.Od8Count;
        IntPtr p = IntPtr.Zero;
        try
        {
            int rc = f(_ctx, adapter, out caps, ref count, ref p);
            if (rc != 0) return rc;
            if (p == IntPtr.Zero || count < 1 || count > 1024) return -1;
            list = new byte[count * AmdOc.Od8SingleSize];
            Marshal.Copy(p, list, 0, list.Length);
            return 0;
        }
        finally { if (p != IntPtr.Zero) Marshal.FreeHGlobal(p); }
    }

    public int Od8Current(int adapter, out int[] values)
    {
        values = null;
        var f = Fn<Od8CurFn>("ADL2_Overdrive8_Current_SettingX2_Get");
        if (f == null) return -8;
        int count = AmdOc.Od8Count;
        IntPtr p = IntPtr.Zero;
        try
        {
            int rc = f(_ctx, adapter, ref count, ref p);
            if (rc != 0) return rc;
            if (p == IntPtr.Zero || count < 1 || count > 1024) return -1;
            values = new int[count];
            Marshal.Copy(p, values, 0, count);
            return 0;
        }
        finally { if (p != IntPtr.Zero) Marshal.FreeHGlobal(p); }
    }

    public int Od8Set(int adapter, byte[] set, byte[] current)
    {
        var f = Fn<Od8SetFn>("ADL2_Overdrive8_Setting_Set");
        if (f == null) return -8;
        // Room beyond the count in both blocks, in case the driver fills more than it was asked for.
        int count = BitConverter.ToInt32(set, 0);
        var s = new byte[Math.Max(set.Length, 4 + Math.Max(count, AmdOc.Od8Count) * AmdOc.Od8SetEntry) + 256];
        set.CopyTo(s, 0);
        var c = new byte[Math.Max(current.Length, 4 + Math.Max(count, AmdOc.Od8Count) * 4) + 256];
        BitConverter.GetBytes(count).CopyTo(c, 0);
        int rc = f(_ctx, adapter, s, c);
        Array.Copy(c, current, current.Length);
        return rc;
    }

    public int OdnCaps(int adapter, out byte[] caps, out bool x2)
    {
        var f2 = Fn<BlockFn>("ADL2_OverdriveN_CapabilitiesX2_Get");
        if (f2 != null && Fn<BlockFn>("ADL2_OverdriveN_SystemClocksX2_Get") != null)
        {
            caps = new byte[AmdOc.OdnCapsX2];
            x2 = true;
            int rc = f2(_ctx, adapter, caps);
            if (rc == 0) return 0;
        }
        x2 = false;
        caps = new byte[AmdOc.OdnCaps];
        var f = Fn<BlockFn>("ADL2_OverdriveN_Capabilities_Get");
        return f == null ? -8 : f(_ctx, adapter, caps);
    }

    public int OdnClocks(int adapter, bool memory, bool set, bool x2, byte[] levels)
    {
        string name = "ADL2_OverdriveN_" + (memory ? "MemoryClocks" : "SystemClocks") + (x2 ? "X2" : "") + (set ? "_Set" : "_Get");
        var f = Fn<BlockFn>(name);
        return f == null ? -8 : f(_ctx, adapter, levels);
    }

    public int OdnPower(int adapter, bool set, byte[] power)
    {
        var f = Fn<BlockFn>(set ? "ADL2_OverdriveN_PowerLimit_Set" : "ADL2_OverdriveN_PowerLimit_Get");
        return f == null ? -8 : f(_ctx, adapter, power);
    }

    public int OdnTemperature(int adapter, out int value)
    {
        value = 0;
        var f = Fn<TempFn>("ADL2_OverdriveN_Temperature_Get");
        return f == null ? -8 : f(_ctx, adapter, 1, out value);   // 1 = the GPU edge sensor
    }

    public int PmLog(int adapter, byte[] output)
    {
        var f = Fn<BlockFn>("ADL2_New_QueryPMLogData_Get");
        return f == null ? -8 : f(_ctx, adapter, output);
    }
}

/// <summary>Which maker's overclocking is in use: NVIDIA first (as before), AMD when no NVIDIA card answers.</summary>
public static class GpuOc
{
    public const string Nvidia = "nvidia", Amd = "amd";
    /// <summary>Test hook: replaces the detection.</summary>
    public static Func<string> DetectHook;

    /// <summary>"nvidia", "amd" or "" (no card whose driver offers overclocking).</summary>
    public static string Detect()
    {
        if (DetectHook != null) return DetectHook() ?? "";
        if (NvOc.Open()) return Nvidia;
        if (AmdOc.Open()) return Amd;
        return "";
    }

    public static string Error(string vendor) => vendor == Amd ? AmdOc.Error : NvOc.Error;
}
