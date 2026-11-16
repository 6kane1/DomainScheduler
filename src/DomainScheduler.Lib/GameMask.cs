using System.Globalization;

namespace DomainScheduler;

/// <summary>
/// The game's core mask with the IRQ exclusion (section 6): processors that a
/// device's interrupts are pinned to can be left out of the mask, but never
/// more than half of it.
/// </summary>
public static class GameMask
{
    public const string Off = "Off", ThreadMode = "Thread", CoreMode = "Core";
    /// <summary>The IRQ refinement mode of preset masks (the Masks page; Auto has its own policy).</summary>
    public static string Mode = CoreMode;

    /// <summary>Test hook for the pinned interrupts.</summary>
    public static Func<List<KeyValuePair<string, ulong>>> IrqHook;
    /// <summary>The real source (Devices.PinnedIrqs), set by the Windows layer.</summary>
    public static Func<List<KeyValuePair<string, ulong>>> IrqSource;

    static readonly object _gate = new();
    static List<KeyValuePair<string, ulong>> _irq = new();
    static DateTime _irqAt;

    /// <summary>Pinned interrupts (device, mask), cached for 30 s.</summary>
    public static List<KeyValuePair<string, ulong>> Pinned()
    {
        if (IrqHook != null) return IrqHook() ?? new();
        lock (_gate)
        {
            if (DateTime.UtcNow - _irqAt < TimeSpan.FromSeconds(30)) return _irq;
            _irqAt = DateTime.UtcNow;
            try { _irq = IrqSource?.Invoke() ?? new(); }
            catch (Exception) { _irq = new(); }
            return _irq;
        }
    }

    public static void InvalidateIrq()
    {
        lock (_gate) _irqAt = default;
    }

    public static ulong WholeCores(Topo t, ulong m) => t.Sets.Where(s => s.Group == 0).GroupBy(s => s.Core)
        .Where(g => g.Any(s => s.LP < 64 && (m >> s.LP & 1) != 0)).Aggregate(0UL, (a, g) => a | TopoBuilder.Mask(g));

    public static bool Dedicated(Topo t, ulong pin) => pin != 0 && t.Sets
        .Where(s => s.Group == 0 && s.LP < 64 && (pin >> s.LP & 1) != 0).Select(s => s.Core).Distinct().Count() == 1;

    public const string WideNote = "wide-irq: partial";

    public static ulong Adjust(Topo t, ulong baseM, ulong add, ulong drop, ulong irq, string mode, out ulong irqDrop, out bool irqIgnored) =>
        Adjust(t, baseM, add, drop, irq, 0, mode, out irqDrop, out irqIgnored, null);

    /// <summary>
    /// Section 22.8: wide = pins that cover more than one core. They never cut whole cores: only
    /// the pinned processors themselves leave the mask (the "Thread" behaviour) and the note
    /// "wide-irq: partial" is added. Dedicated pins keep the mode's behaviour. The "never more
    /// than half of the mask" guard covers all cuts together.
    /// </summary>
    public static ulong Adjust(Topo t, ulong baseM, ulong add, ulong drop, ulong irq, ulong wide, string mode, out ulong irqDrop, out bool irqIgnored, List<string> notes)
    {
        irqDrop = 0;
        irqIgnored = false;
        ulong full = t.FullMask();
        ulong m = (baseM | add) & ~drop & full;
        if (m == 0) m = baseM & full;
        if (m == 0) m = full;
        if (mode == Off || (irq | wide) == 0) return m;
        ulong wideCut = wide & m;
        ulong cut = ((mode == CoreMode ? WholeCores(t, irq) : irq) & m) | wideCut;
        if (wideCut != 0 && notes != null && !notes.Contains(WideNote)) notes.Add(WideNote);
        if (cut == 0) return m;
        ulong kept = m & ~cut;
        if (kept == 0 || TopoBuilder.PopCount(kept) * 2 < TopoBuilder.PopCount(m))
        {
            irqIgnored = true;
            return m;
        }
        irqDrop = cut;
        return kept;
    }

    /// <summary>Section 22.9: processors that are busy with DPC/ISR right now (null = the live source is off).</summary>
    public static Func<ulong> NoisyHook;

    static ulong WideMask(Topo t, GameMaskInfo info)
    {
        // A pin that covers every processor is no pin at all: there is nothing to step around.
        ulong full = t.FullMask();
        return info.IrqWide.Where(p => (p.Value & full) != full).Aggregate(0UL, (a, p) => a | p.Value) & full;
    }

    /// <summary>Every pin; the ones spread over several cores also in IrqWide.</summary>
    public static GameMaskInfo Info(Topo t)
    {
        var info = new GameMaskInfo();
        ulong noisy = (NoisyHook?.Invoke() ?? 0) & t.FullMask();
        for (int lp = 0; lp < 64 && noisy != 0; lp++)
            if ((noisy >> lp & 1) != 0)
            {
                info.IrqDevices.Add(new(L.F("live DPC/ISR on processor {0}", lp), 1UL << lp));
                info.Irq |= 1UL << lp;
            }
        foreach (var p in Pinned())
        {
            if (Dedicated(t, p.Value))
            {
                info.IrqDevices.Add(p);
                info.Irq |= p.Value;
            }
            else info.IrqWide.Add(p);
        }
        return info;
    }

    /// <summary>The base mask with the IRQ exclusion of dedicated pins only.</summary>
    public static GameMaskInfo ForRule(Topo t, ulong baseMask, string mode)
    {
        var info = Info(t);
        info.Mode = mode;
        info.Base = baseMask;
        info.Manual = baseMask;
        info.Final = Adjust(t, baseMask, 0, 0, info.Irq, WideMask(t, info), mode, out info.IrqDrop, out info.IrqIgnored, info.Notes);
        return info;
    }

    public static string CpuSig(Topo t) =>
        string.Create(CultureInfo.InvariantCulture, $"{t.LogicalCount}|{t.PhysicalCount}|{t.LLCGroups.Count}|{(t.Hybrid ? 1 : 0)}");

    /// <summary>Manual edits of a mask, bound to the hardware signature they were made on.</summary>
    public sealed class Edits
    {
        public ulong Add, Drop;
        public string Sig = "";
    }

    /// <summary>The full mask of a rule: base spec, manual edits (dropped on another processor), IRQ exclusion.</summary>
    public static GameMaskInfo Build(Topo t, string baseRule, Edits edits, string mode)
    {
        var r = Masks.Resolve(t, baseRule);
        var info = Info(t);
        info.BaseRule = baseRule;
        info.Mode = mode;
        info.Base = r.Mask;
        ulong add = edits?.Add ?? 0, drop = edits?.Drop ?? 0;
        if ((add != 0 || drop != 0) && edits.Sig != CpuSig(t))
        {
            add = drop = 0;
            info.EditsStale = true;
        }
        info.Add = add;
        info.Drop = drop;
        info.Manual = Adjust(t, info.Base, add, drop, 0, Off, out _, out _);
        info.Final = Adjust(t, info.Base, add, drop, info.Irq, WideMask(t, info), mode, out info.IrqDrop, out info.IrqIgnored, info.Notes);
        return info;
    }

    static string Hex(ulong m) => "0x" + m.ToString("x", CultureInfo.InvariantCulture);

    /// <summary>What is stored for the rule: the hex when it differs from the base, otherwise the spec.</summary>
    public static string Rule(GameMaskInfo info) => info.Final != info.Base ? Hex(info.Final) : info.BaseRule;

    public static string ManualRule(GameMaskInfo info) => info.Manual != info.Base ? Hex(info.Manual) : info.BaseRule;

    /// <summary>Where background programs go next to this game mask.</summary>
    public static ulong Background(Topo t, GameMaskInfo info) => Masks.BgMask(t, Masks.Resolve(t, ManualRule(info)).Mask);
}
