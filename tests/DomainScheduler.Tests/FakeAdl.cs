namespace DomainScheduler.Tests;

/// <summary>
/// An in-memory AMD driver (ADL) for the AMD overclock tests: adapters, an Overdrive8 table and an OverdriveN state
/// table, all answered in the byte layouts of the ADL SDK. Like the real driver it refuses values outside its ranges.
/// </summary>
public sealed class FakeAdl : IAdl
{
    public sealed class Adapter
    {
        public int Index, Bus, Vendor = 1002, Supported = 1, Enabled = 1, Version = 8, Asic = 1;
        public string Name = "AMD Radeon";
        public bool Active = true, AsicMissing;
    }

    public bool Missing;
    public readonly List<Adapter> Adapters = new();
    public readonly List<string> Calls = new();
    public int Writes;

    // ---- Overdrive8 ----
    public int Od8Caps;
    public int[] Min = new int[AmdOc.Od8Count], Max = new int[AmdOc.Od8Count], Def = new int[AmdOc.Od8Count], Cur = new int[AmdOc.Od8Count];
    public readonly List<List<(int Id, int Value, bool Reset)>> Od8Sets = new();
    public int Od8SetRc;

    public void Od8(int id, int min, int max, int def, int? cur = null)
    {
        Min[id] = min; Max[id] = max; Def[id] = def; Cur[id] = cur ?? def;
    }

    // ---- OverdriveN ----
    public bool X2 = true;
    public int OdnFlags;
    public (int Min, int Max, int Step, int Def) Eng, MemR, Vddc, Pow;
    public int[] SysClk, SysVddc, SysDefClk, MemClk, MemVddc, MemDefClk;
    public int[] SysEnabled, MemEnabled, SysControl, MemControl;
    public int Tdp, TdpMode, MaxTemp = 85;
    public int FailSysSet, FailMemSet, FailPowerSet;
    public bool DefaultReadFails;
    public readonly List<(bool Memory, int Mode, int[] Clocks, int[] Vddc, int[] Enabled)> OdnSets = new();
    public readonly List<(int Mode, int Tdp)> PowerSets = new();

    static int I(byte[] b, int at) => BitConverter.ToInt32(b, at);
    static void W(byte[] b, int at, int v) => BitConverter.GetBytes(v).CopyTo(b, at);

    public bool Open(out string error)
    {
        Calls.Add("Open");
        error = Missing ? "No AMD driver is installed (its interface atiadlxx.dll was not found)" : "";
        return !Missing;
    }

    public int AdapterCount(out int count) { count = Adapters.Count; return 0; }

    public int AdapterInfo(byte[] buffer)
    {
        for (int i = 0; i < Adapters.Count; i++)
        {
            var a = Adapters[i];
            int at = i * AmdOc.AdapterInfoSize;
            W(buffer, at, AmdOc.AdapterInfoSize);
            W(buffer, at + 4, a.Index);
            W(buffer, at + AmdOc.BusAt, a.Bus);
            W(buffer, at + AmdOc.VendorAt, a.Vendor);
            System.Text.Encoding.ASCII.GetBytes(a.Name).CopyTo(buffer, at + AmdOc.NameAt);
        }
        return 0;
    }

    Adapter A(int index) => Adapters.First(a => a.Index == index);

    public int Active(int adapter, out int status) { status = A(adapter).Active ? 1 : 0; return 0; }

    public int OverdriveCaps(int adapter, out int supported, out int enabled, out int version)
    {
        var a = A(adapter);
        (supported, enabled, version) = (a.Supported, a.Enabled, a.Version);
        return 0;
    }

    public int AsicType(int adapter, out int types, out int valids)
    {
        var a = A(adapter);
        types = a.Asic;
        valids = 0xFF;
        return a.AsicMissing ? -8 : 0;
    }

    public int Od8Init(int adapter, out int caps, out byte[] list)
    {
        caps = Od8Caps;
        list = new byte[Min.Length * AmdOc.Od8SingleSize];
        for (int i = 0; i < Min.Length; i++)
        {
            W(list, i * 16 + 4, Min[i]);
            W(list, i * 16 + 8, Max[i]);
            W(list, i * 16 + 12, Def[i]);
        }
        return 0;
    }

    public int Od8Current(int adapter, out int[] values) { values = (int[])Cur.Clone(); return 0; }

    public int Od8Set(int adapter, byte[] set, byte[] current)
    {
        Writes++;
        int count = I(set, 0);
        var items = new List<(int, int, bool)>();
        for (int id = 0; id < count; id++)
        {
            int at = 4 + id * AmdOc.Od8SetEntry;
            if (I(set, at + 4) == 0) continue;
            items.Add((id, I(set, at), I(set, at + 8) != 0));
        }
        Od8Sets.Add(items);
        if (Od8SetRc != 0) return Od8SetRc;
        foreach (var (id, v, reset) in items)
            if (!reset && (v < Min[id] || v > Max[id])) return -3;   // out of range: nothing is taken
        foreach (var (id, v, reset) in items) Cur[id] = reset ? Def[id] : v;
        W(current, 0, count);
        for (int i = 0; i < count && 4 + i * 4 + 4 <= current.Length; i++) W(current, 4 + i * 4, Cur[i]);
        return 0;
    }

    public int OdnCaps(int adapter, out byte[] caps, out bool x2)
    {
        x2 = X2;
        caps = new byte[X2 ? AmdOc.OdnCapsX2 : AmdOc.OdnCaps];
        W(caps, 0, SysClk.Length);
        int bas = X2 ? 8 : 4;
        if (X2) W(caps, 4, OdnFlags);
        var ranges = new[] { Eng, MemR, Vddc, Pow };
        for (int k = 0; k < ranges.Length; k++)
        {
            int at = bas + k * AmdOc.OdnRange;
            W(caps, at + 4, ranges[k].Min);
            W(caps, at + 8, ranges[k].Max);
            W(caps, at + 12, ranges[k].Step);
            W(caps, at + 16, ranges[k].Def);
        }
        return 0;
    }

    public int OdnClocks(int adapter, bool memory, bool set, bool x2, byte[] levels)
    {
        int size = x2 ? 16 : 12;
        var clk = memory ? MemClk : SysClk;
        var vdd = memory ? MemVddc : SysVddc;
        var def = memory ? MemDefClk : SysDefClk;
        var en = (memory ? MemEnabled : SysEnabled) ?? Enumerable.Repeat(1, clk.Length).ToArray();
        var ctl = (memory ? MemControl : SysControl) ?? Enumerable.Repeat(7, clk.Length).ToArray();
        var range = memory ? MemR : Eng;
        if (!set)
        {
            int mode = I(levels, 4);
            if (mode == AmdOc.ModeDefault && DefaultReadFails) return -8;
            W(levels, 8, clk.Length);
            for (int i = 0; i < clk.Length; i++)
            {
                int at = 12 + i * size;
                W(levels, at, mode == AmdOc.ModeDefault ? def[i] : clk[i]);
                W(levels, at + 4, vdd[i]);
                W(levels, at + 8, en[i]);
                if (x2) W(levels, at + 12, ctl[i]);
            }
            return 0;
        }
        Writes++;
        int n = I(levels, 8), m = I(levels, 4);
        var c = new int[n]; var v = new int[n]; var e = new int[n];
        for (int i = 0; i < n; i++) { c[i] = I(levels, 12 + i * size); v[i] = I(levels, 12 + i * size + 4); e[i] = I(levels, 12 + i * size + 8); }
        OdnSets.Add((memory, m, c, v, e));
        int fail = memory ? FailMemSet : FailSysSet;
        if (fail != 0) return fail;
        if (c.Any(x => x < range.Min || x > range.Max)) return -3;
        if (m == AmdOc.ModeDefault) c = (int[])def.Clone();
        Array.Copy(c, clk, n);
        return 0;
    }

    public int OdnPower(int adapter, bool set, byte[] power)
    {
        if (!set)
        {
            W(power, 0, TdpMode);
            W(power, 4, Tdp);
            W(power, 8, MaxTemp);
            return 0;
        }
        Writes++;
        int mode = I(power, 0), tdp = I(power, 4);
        PowerSets.Add((mode, tdp));
        if (FailPowerSet != 0) return FailPowerSet;
        if (mode == AmdOc.ModeAuto) { TdpMode = mode; Tdp = Pow.Def; return 0; }
        if (tdp < Pow.Min || tdp > Pow.Max) return -3;
        TdpMode = mode;
        Tdp = tdp;
        return 0;
    }

    public int OdnTemperature(int adapter, out int value) { value = 65000; return 0; }

    public int PmLog(int adapter, byte[] output)
    {
        W(output, 4 + AmdOc.PmEdge * 8, 1);
        W(output, 4 + AmdOc.PmEdge * 8 + 4, 60);
        return 0;
    }

    // ---- cards ----------------------------------------------------------------------------------------

    /// <summary>
    /// An RX 580 8 GB as Wattman shows it (OverdriveN, X2 calls, clocks in 10 kHz units): 8 core states up to 1366 MHz,
    /// 3 memory states up to 2000 MHz, core range 300–2000 MHz, memory 300–2250 MHz, power limit −50…+50 %, no
    /// memory underclocking.
    /// </summary>
    public static FakeAdl Rx580(bool x2 = true, int scale = 100)
    {
        var f = new FakeAdl { X2 = x2 };
        f.Adapters.Add(new Adapter { Index = 0, Bus = 1, Version = 7, Name = "Radeon RX 580 Series" });
        f.Adapters.Add(new Adapter { Index = 1, Bus = 1, Version = 7, Name = "Radeon RX 580 Series", Active = false });   // second output of the same card
        f.OdnFlags = AmdOc.OdnSclk | AmdOc.OdnMclk | (1 << 2) | (1 << 3) | AmdOc.OdnPower;
        f.Eng = (300 * scale, 2000 * scale, scale, 1366 * scale);
        f.MemR = (300 * scale, 2250 * scale, scale, 2000 * scale);
        f.Vddc = (750, 1200, 1, 1150);
        f.Pow = (-50, 50, 1, 0);
        f.SysDefClk = new[] { 300, 600, 900, 1145, 1215, 1257, 1300, 1366 }.Select(x => x * scale).ToArray();
        f.SysClk = (int[])f.SysDefClk.Clone();
        f.SysVddc = new[] { 750, 769, 906, 1000, 1056, 1100, 1125, 1150 };
        f.SysEnabled = new[] { 1, 1, 1, 1, 1, 1, 1, 2 };   // a driver may report "enabled" as another non-zero value
        f.MemDefClk = new[] { 300, 1000, 2000 }.Select(x => x * scale).ToArray();
        f.MemClk = (int[])f.MemDefClk.Clone();
        f.MemVddc = new[] { 750, 800, 950 };
        return f;
    }

    /// <summary>
    /// An RX 6800 XT-like Overdrive8 table: max GPU clock 500–2800 MHz (stock 2250), min clock 500, max memory clock
    /// 1000–1150 MHz (stock 1000), power limit −10…+15 %, tuning mode 0–3 (stock 0). voltage: with an offset range when asked.
    /// </summary>
    public static FakeAdl Rx6800(bool voltage = false)
    {
        var f = new FakeAdl();
        f.Adapters.Add(new Adapter { Index = 0, Bus = 3, Version = 8, Name = "AMD Radeon RX 6800 XT" });
        f.Od8Caps = AmdOc.CapGfxLimits | AmdOc.CapUclkMax | AmdOc.CapPower | AmdOc.CapPowerMode | (voltage ? AmdOc.CapVoltage : 0);
        f.Od8(AmdOc.GfxMax, 500, 2800, 2250);
        f.Od8(AmdOc.GfxMin, 500, 2800, 500);
        f.Od8(AmdOc.UclkMax, 1000, 1150, 1000);
        f.Od8(AmdOc.PowerPct, -10, 15, 0);
        f.Od8(AmdOc.PowerMode, 0, 3, 0);
        if (voltage) f.Od8(AmdOc.OdVoltage, -200, 50, 0);
        return f;
    }
}
