namespace DomainScheduler.Tests;

/// <summary>Synthetic topologies of the acceptance list.</summary>
public static class Synth
{
    public const ulong MB = 1UL << 20;

    /// <summary>cores × (smt?2:1) logical processors; core c is LP 2c, 2c+1 with SMT; one LLC per ccdCores cores.</summary>
    public static Topo Symmetric(int cores, bool smt, int ccdCores, ulong[] sizes, string name, int reported = 0)
    {
        var sets = new List<CpuSet>();
        int t = smt ? 2 : 1, lp = 0;
        for (int c = 0; c < cores; c++)
            for (int h = 0; h < t; h++, lp++)
                sets.Add(new CpuSet { Id = (uint)(256 + lp), LP = (byte)lp, Core = (byte)c, LLC = (byte)(c / ccdCores * ccdCores) });
        var caches = new List<L3Cache>();
        for (int i = 0; i < sizes.Length; i++)
        {
            ulong m = TopoBuilder.Mask(sets.Where(s => s.Core / ccdCores == i));
            var c = new L3Cache { Size = sizes[i] };
            c.Masks.Add((0, m));
            caches.Add(c);
        }
        return TopoBuilder.Build(sets, caches, name, reported > 0 ? reported : sets.Count);
    }

    public static Topo X7950X3D() => Symmetric(16, true, 8, new[] { 96 * MB, 32 * MB }, "AMD Ryzen 9 7950X3D 16-Core Processor");
    public static Topo X7950X() => Symmetric(16, true, 8, new[] { 32 * MB, 32 * MB }, "AMD Ryzen 9 7950X 16-Core Processor");
    public static Topo X7800X3D() => Symmetric(8, true, 8, new[] { 96 * MB }, "AMD Ryzen 7 7800X3D 8-Core Processor");
    public static Topo I12400() => Symmetric(6, true, 6, new[] { 18 * MB }, "12th Gen Intel(R) Core(TM) i5-12400F");
    public static Topo Six() => Symmetric(6, false, 6, new[] { 12 * MB }, "6-core no SMT");

    /// <summary>13900K: 8 P-cores with SMT (LP 0-15, class 1), 16 E-cores (LP 16-31, class 0), one L3.</summary>
    public static Topo I13900K()
    {
        var sets = new List<CpuSet>();
        for (int c = 0; c < 8; c++)
        {
            sets.Add(new CpuSet { Id = (uint)(256 + 2 * c), LP = (byte)(2 * c), Core = (byte)(2 * c), Eff = 1 });
            sets.Add(new CpuSet { Id = (uint)(257 + 2 * c), LP = (byte)(2 * c + 1), Core = (byte)(2 * c), Eff = 1 });
        }
        for (int e = 0; e < 16; e++)
            sets.Add(new CpuSet { Id = (uint)(272 + e), LP = (byte)(16 + e), Core = (byte)(16 + e), Eff = 0 });
        var c3 = new L3Cache { Size = 36 * MB };
        c3.Masks.Add((0, 0xFFFFFFFFUL));
        return TopoBuilder.Build(sets, new List<L3Cache> { c3 }, "13th Gen Intel(R) Core(TM) i9-13900K", 32);
    }
}
