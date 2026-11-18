using System.Runtime.InteropServices;

namespace DomainScheduler;

/// <summary>One clock domain of an NVIDIA card: its base frequency, the current offset and the range the driver allows.</summary>
public sealed class NvClock
{
    public int Domain;                 // 0 = GPU core, 4 = memory
    public bool Editable;
    public int BaseKHz, DeltaKHz, MinDeltaKHz, MaxDeltaKHz;
}

/// <summary>
/// Clock offsets for NVIDIA cards through the driver's own performance-state table (the mechanism
/// overclocking tools use). Deliberately narrow:
///  - the core and memory clock OFFSET and the core voltage boost in percent (0–100); no power limit, no fan;
///  - clocks only inside the range the driver reports and inside a cap of our own;
///  - the table is sanity-checked after reading, every write is read back;
///  - nothing is reset when the program exits: the values stay in the driver until a restart, and
///    the application re-applies saved values at start only when the last session with them survived.
/// The table layout is fixed by the driver: 20 bytes of header, 16 performance states of 456 bytes,
/// each with 8 clocks of 44 bytes. The byte work is separated from the driver calls so it can be tested.
/// </summary>
public static class NvOc
{
    public const int Size = 7316, Version = Size | (1 << 16);
    /// <summary>Newer drivers use versions 2 and 3: the same body plus 100 bytes of over-voltage entries at the end.</summary>
    public const int SizeV2 = 7416, Version2 = SizeV2 | (2 << 16), Version3 = SizeV2 | (3 << 16);
    public const int VoltSize = 40, VoltVersion = VoltSize | (1 << 16);
    const int Header = 20, Pstate = 456, ClocksAt = 8, Clock = 44;
    public const int DomainCore = 0, DomainMemory = 4;
    /// <summary>Our own caps, tighter than what most drivers allow: +300 MHz core, +1500 MHz memory, never below −300 MHz.</summary>
    public const int CapCoreKHz = 300_000, CapMemoryKHz = 1_500_000, FloorKHz = -300_000;

    static int I(byte[] b, int at) => BitConverter.ToInt32(b, at);
    static void W(byte[] b, int at, int v) => BitConverter.GetBytes(v).CopyTo(b, at);

    /// <summary>Reads the clocks of the first performance state. Returns null when the table does not look like one.</summary>
    public static List<NvClock> Parse(byte[] table)
    {
        if (table == null || table.Length < Size) return null;
        int version = I(table, 0);
        if (version != Version && version != Version2 && version != Version3) return null;
        int pstates = I(table, 8), clocks = I(table, 12);
        if (pstates < 1 || pstates > 16 || clocks < 1 || clocks > 8) return null;
        var list = new List<NvClock>();
        for (int j = 0; j < clocks; j++)
        {
            int at = Header + ClocksAt + j * Clock;
            var c = new NvClock
            {
                Domain = I(table, at), Editable = (I(table, at + 8) & 1) != 0,
                DeltaKHz = I(table, at + 12), MinDeltaKHz = I(table, at + 16), MaxDeltaKHz = I(table, at + 20),
            };
            // typeId 0: one frequency; 1: a range (min, max) — the maximum is the frequency the offset moves.
            c.BaseKHz = I(table, at + 4) == 0 ? I(table, at + 24) : I(table, at + 28);
            // Plausibility: a real clock is between 100 MHz and 20 GHz and its offset range brackets zero.
            if (c.BaseKHz < 100_000 || c.BaseKHz > 20_000_000 || c.MinDeltaKHz > 0 || c.MaxDeltaKHz < 0 || c.MaxDeltaKHz > 3_000_000 || c.MinDeltaKHz < -3_000_000) return null;
            list.Add(c);
        }
        return list;
    }

    /// <summary>The offset that will really be requested: inside the driver's range and inside our cap.</summary>
    public static int Clamp(NvClock c, int wantKHz)
    {
        int cap = c.Domain == DomainMemory ? CapMemoryKHz : CapCoreKHz;
        int hi = Math.Min(c.MaxDeltaKHz, cap), lo = Math.Max(c.MinDeltaKHz, FloorKHz);
        return Math.Clamp(wantKHz, lo, hi);
    }

    /// <summary>A table that changes one clock offset of the first performance state and nothing else.</summary>
    public static byte[] BuildSet(int domain, int deltaKHz, int version = Version)
    {
        var b = new byte[version == Version ? Size : SizeV2];
        W(b, 0, version);
        W(b, 8, 1);      // one performance state
        W(b, 12, 1);     // one clock
        int at = Header + ClocksAt;
        W(b, at, domain);
        W(b, at + 12, deltaKHz);
        return b;
    }

    // ---- the driver ---------------------------------------------------------------------------------

    [DllImport("nvapi64.dll", EntryPoint = "nvapi_QueryInterface", CallingConvention = CallingConvention.Cdecl)]
    static extern IntPtr QueryInterface(uint id);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int InitFn();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int EnumFn([Out] IntPtr[] handles, out int count);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int TableFn(IntPtr gpu, [In, Out] byte[] table);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int NameFn(IntPtr gpu, [Out] byte[] name);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int ThermalFn(IntPtr gpu, int sensor, [In, Out] byte[] settings);

    static readonly object _gate = new();
    static IntPtr _gpu;
    static bool _tried, _changed;
    static TableFn _get, _set;
    static ThermalFn _thermal;
    public static string Error { get; private set; } = "";
    public static string Name { get; private set; } = "";
    /// <summary>Test hooks: replace the two driver calls.</summary>
    public static Func<byte[]> GetHook;
    public static Func<byte[], int> SetHook;

    static T Fn<T>(uint id) where T : Delegate
    {
        IntPtr p = QueryInterface(id);
        return p == IntPtr.Zero ? null : Marshal.GetDelegateForFunctionPointer<T>(p);
    }

    /// <summary>Finds the first NVIDIA card. False (with Error) when there is none or the driver does not answer.</summary>
    public static bool Open()
    {
        lock (_gate)
        {
            if (GetHook != null) return true;
            if (_gpu != IntPtr.Zero) return true;
            if (_tried) return false;
            _tried = true;
            try
            {
                var init = Fn<InitFn>(0x0150E828);
                var enumerate = Fn<EnumFn>(0xE5AC921F);
                _get = Fn<TableFn>(0x6FF81213);
                _set = Fn<TableFn>(0x0F4DAE6B);
                _thermal = Fn<ThermalFn>(0xE3640A56);
                var name = Fn<NameFn>(0xCEEE8E9F);
                if (init == null || enumerate == null || _get == null || _set == null) { Error = L.T("The NVIDIA driver does not offer the needed functions"); return false; }
                if (init() != 0) { Error = L.T("The NVIDIA driver did not start its interface"); return false; }
                var handles = new IntPtr[64];
                if (enumerate(handles, out int n) != 0 || n < 1) { Error = L.T("No NVIDIA graphics card was found"); return false; }
                _gpu = handles[0];
                var nm = new byte[64];
                if (name != null && name(_gpu, nm) == 0) Name = System.Text.Encoding.ASCII.GetString(nm).TrimEnd('\0');
                return true;
            }
            catch (DllNotFoundException) { Error = L.T("No NVIDIA driver is installed"); return false; }
            catch (Exception e) { Error = e.Message; return false; }
        }
    }

    static int _tableVersion = Version3;

    static byte[] ReadTable()
    {
        if (GetHook != null)
        {
            var hooked = GetHook();
            if (hooked != null && hooked.Length >= 4) _tableVersion = I(hooked, 0);
            return hooked;
        }
        // The driver says which version of the table it speaks: -9 means "another version".
        foreach (int version in new[] { _tableVersion, Version3, Version2, Version }.Distinct())
        {
            var b = new byte[version == Version ? Size : SizeV2];
            BitConverter.GetBytes(version).CopyTo(b, 0);
            int rc = _get(_gpu, b);
            if (rc == 0)
            {
                _tableVersion = version;
                return b;
            }
            if (rc != -9) { Error = StatusText(rc); return null; }
        }
        return null;
    }

    /// <summary>The driver's status codes in words.</summary>
    public static string StatusText(int rc) => rc switch
    {
        -3 or -104 or -136 => L.T("this graphics card or driver does not support it"),
        -137 => L.T("administrator rights are needed"),
        -9 => L.T("the driver uses another version of this interface"),
        _ => L.F("driver code {0}", rc),
    };

    /// <summary>The clocks as the driver reports them now; null when they cannot be read or do not pass the checks.</summary>
    public static List<NvClock> Read()
    {
        lock (_gate)
        {
            if (!Open()) return null;
            try
            {
                var list = Parse(ReadTable());
                if (list == null) Error = L.T("The driver's clock table is not in the expected form: overclocking is not offered for this card");
                return list;
            }
            catch (Exception e) { Error = e.Message; return null; }
        }
    }

    /// <summary>Requests an offset for one domain. Returns the offset in force after reading it back, or null (with Error).</summary>
    public static int? Set(int domain, int wantKHz)
    {
        lock (_gate)
        {
            if (Flags.Refuse(Flags.GpuOc, L.T("GPU clock offset"))) { Error = L.T("The feature flag gpu-oc is closed"); return null; }
            var clocks = Read();
            var c = clocks?.FirstOrDefault(x => x.Domain == domain);
            if (c == null) { if (clocks != null) Error = L.T("This card does not have that clock"); return null; }
            if (!c.Editable) { Error = L.T("The driver does not allow changing this clock"); return null; }
            int delta = Clamp(c, wantKHz);
            try
            {
                var table = BuildSet(domain, delta, _tableVersion);
                int rc = SetHook != null ? SetHook(table) : _set(_gpu, table);
                if (rc != 0) { Error = L.F("The driver refused the offset: {0}", StatusText(rc)); return null; }
                _changed = true;
                var after = Read()?.FirstOrDefault(x => x.Domain == domain);
                if (after == null || after.DeltaKHz != delta) { Error = L.T("The offset read back differs from the request"); return after?.DeltaKHz; }
                Error = "";
                return delta;
            }
            catch (Exception e) { Error = e.Message; return null; }
        }
    }

    // ---- core voltage boost, percent ----------------------------------------------------------------

    static TableFn _voltGet, _voltSet;
    public static Func<byte[]> VoltGetHook;
    public static Func<byte[], int> VoltSetHook;

    public static byte[] BuildVolt(int percent)
    {
        var b = new byte[VoltSize];
        W(b, 0, VoltVersion);
        W(b, 4, Math.Clamp(percent, 0, 100));
        return b;
    }

    /// <summary>The voltage boost in force (0–100), or null when the card or driver does not offer it.</summary>
    public static int? GetVoltage()
    {
        lock (_gate)
        {
            try
            {
                byte[] b;
                if (VoltGetHook != null) b = VoltGetHook();
                else
                {
                    if (!Open()) return null;
                    _voltGet ??= Fn<TableFn>(0x9DF23CA1);
                    if (_voltGet == null) return null;
                    b = BuildVolt(0);
                    if (_voltGet(_gpu, b) != 0) return null;
                }
                if (b == null || b.Length < VoltSize) return null;
                int p = I(b, 4);
                return p is >= 0 and <= 100 ? p : null;
            }
            catch (Exception) { return null; }
        }
    }

    /// <summary>Requests a voltage boost of 0–100 percent; returns what is in force after reading back, or null (with Error).</summary>
    public static int? SetVoltage(int percent)
    {
        lock (_gate)
        {
            if (Flags.Refuse(Flags.GpuOc, L.T("GPU voltage boost"))) { Error = L.T("The feature flag gpu-oc is closed"); return null; }
            percent = Math.Clamp(percent, 0, 100);
            if (GetVoltage() == null) { Error = L.T("This graphics card or driver does not offer a voltage boost"); return null; }
            try
            {
                var b = BuildVolt(percent);
                int rc;
                if (VoltSetHook != null) rc = VoltSetHook(b);
                else
                {
                    _voltSet ??= Fn<TableFn>(0xB9306D9B);
                    if (_voltSet == null) { Error = L.T("This graphics card or driver does not offer a voltage boost"); return null; }
                    rc = _voltSet(_gpu, b);
                }
                if (rc != 0) { Error = L.F("The driver refused the voltage boost: {0}", StatusText(rc)); return null; }
                _changed = true;
                int? after = GetVoltage();
                if (after != percent) { Error = L.T("The voltage boost read back differs from the request"); return after; }
                Error = "";
                return percent;
            }
            catch (Exception e) { Error = e.Message; return null; }
        }
    }

    /// <summary>Clock offsets and the voltage boost back to stock. Only on the user's "Reset" (never by itself at exit).</summary>
    public static bool Reset(bool force = false)
    {
        lock (_gate)
        {
            if (!_changed && !force) return false;
            bool ok = true;
            try
            {
                if (GetVoltage() is > 0)
                {
                    var v = BuildVolt(0);
                    ok &= (VoltSetHook != null ? VoltSetHook(v) : (_voltSet ??= Fn<TableFn>(0xB9306D9B))?.Invoke(_gpu, v) ?? -1) == 0;
                }
            }
            catch (Exception) { ok = false; }
            foreach (int domain in new[] { DomainCore, DomainMemory })
            {
                try
                {
                    var c = Read()?.FirstOrDefault(x => x.Domain == domain);
                    if (c == null || c.DeltaKHz == 0) continue;
                    var table = BuildSet(domain, 0, _tableVersion);
                    ok &= (SetHook != null ? SetHook(table) : _set(_gpu, table)) == 0;
                }
                catch (Exception) { ok = false; }
            }
            _changed = false;
            return ok;
        }
    }

    /// <summary>The GPU temperature in °C, or -1 when the driver does not say.</summary>
    public static int Temperature()
    {
        lock (_gate)
        {
            try
            {
                if (GetHook != null || !Open() || _thermal == null) return -1;
                // NV_GPU_THERMAL_SETTINGS v2: version, count, then sensors of 20 bytes (controller, min, max, current, target).
                var b = new byte[4 + 4 + 3 * 20];
                BitConverter.GetBytes(b.Length | (2 << 16)).CopyTo(b, 0);
                if (_thermal(_gpu, 15, b) != 0 || BitConverter.ToInt32(b, 4) < 1) return -1;
                int t = BitConverter.ToInt32(b, 8 + 12);
                return t is > 0 and < 130 ? t : -1;
            }
            catch (Exception) { return -1; }
        }
    }
}
