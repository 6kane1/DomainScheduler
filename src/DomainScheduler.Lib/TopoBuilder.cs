using System.Buffers.Binary;

namespace DomainScheduler;

/// <summary>Builds the processor model (section 3) from the CPU Set and cache buffers.</summary>
public static class TopoBuilder
{
    static readonly object _gate = new();
    static Topo _live;
    static DateTime _liveAt;

    /// <summary>The live topology: LiveHook in tests, otherwise read from Windows; cached for 30 s.</summary>
    public static Topo Live()
    {
        if (Topo.LiveHook != null) return Topo.LiveHook();
        lock (_gate)
        {
            if (_live != null && DateTime.UtcNow - _liveAt < TimeSpan.FromSeconds(30)) return _live;
            var sets = ParseCpuSets(Native.CpuSetBuffer());
            if (sets.Count == 0) return _live;
            List<L3Cache> caches;
            try { caches = ParseL3(Native.CacheBuffer()); }
            catch (Exception) { caches = new List<L3Cache>(); }   // a cache read error is not fatal
            _live = Build(sets, caches, Native.CpuName(), Native.ActiveLpCount());
            _liveAt = DateTime.UtcNow;
            return _live;
        }
    }

    /// <summary>Drops the cached topology (next Live() reads again).</summary>
    public static void Invalidate()
    {
        lock (_gate) _live = null;
    }

    /// <summary>Walks a GetSystemCpuSetInformation buffer with step Size, Type 0 only.</summary>
    public static List<CpuSet> ParseCpuSets(byte[] buf)
    {
        var list = new List<CpuSet>();
        if (buf == null) return list;
        int off = 0;
        while (off + 8 <= buf.Length)
        {
            int size = BinaryPrimitives.ReadInt32LittleEndian(buf.AsSpan(off));
            int type = BinaryPrimitives.ReadInt32LittleEndian(buf.AsSpan(off + 4));
            if (size <= 0 || off + size > buf.Length) break;
            if (type == 0 && size >= 20)
                list.Add(new CpuSet
                {
                    Id = BinaryPrimitives.ReadUInt32LittleEndian(buf.AsSpan(off + 8)),
                    Group = BinaryPrimitives.ReadUInt16LittleEndian(buf.AsSpan(off + 12)),
                    LP = buf[off + 14], Core = buf[off + 15], LLC = buf[off + 16],
                    Numa = buf[off + 17], Eff = buf[off + 18], Flags = buf[off + 19],
                });
            off += size;
        }
        return list;
    }

    /// <summary>
    /// Walks a GetLogicalProcessorInformationEx(RelationCache) buffer: level 3 unified (or data)
    /// caches with their group masks. A broken buffer gives an empty list.
    /// </summary>
    public static List<L3Cache> ParseL3(byte[] buf)
    {
        var list = new List<L3Cache>();
        if (buf == null) return list;
        int off = 0, len = buf.Length;
        while (off + 8 <= len)
        {
            int rel = BinaryPrimitives.ReadInt32LittleEndian(buf.AsSpan(off));
            int size = BinaryPrimitives.ReadInt32LittleEndian(buf.AsSpan(off + 4));
            if (size <= 0 || off + size > len) return new List<L3Cache>();
            int end = off + size;
            if (rel == 2 && size >= 56)
            {
                byte level = buf[off + 8];
                uint csize = BinaryPrimitives.ReadUInt32LittleEndian(buf.AsSpan(off + 12));
                int ctype = BinaryPrimitives.ReadInt32LittleEndian(buf.AsSpan(off + 16));
                if (level == 3 && (ctype == 0 || ctype == 2))   // CacheUnified, CacheData; Instruction/Trace skipped
                {
                    int count = BinaryPrimitives.ReadUInt16LittleEndian(buf.AsSpan(off + 38));
                    if (count == 0) count = 1;   // before Windows 11 the count is 0 and one mask follows
                    var c = new L3Cache { Size = csize };
                    for (int i = 0; i < count; i++)
                    {
                        int m = off + 40 + i * 16;
                        if (m + 16 > end) return new List<L3Cache>();
                        c.Masks.Add((BinaryPrimitives.ReadUInt16LittleEndian(buf.AsSpan(m + 8)), BinaryPrimitives.ReadUInt64LittleEndian(buf.AsSpan(m))));
                    }
                    list.Add(c);
                }
            }
            off = end;
        }
        return list;
    }

    /// <summary>Each L3 domain gets the size of the largest cache covering one of its processors.</summary>
    public static Dictionary<byte, ulong> L3Map(List<CpuSet> sets, List<L3Cache> caches)
    {
        var d = new Dictionary<byte, ulong>();
        foreach (var s in sets)
            foreach (var c in caches)
                if (c.Masks.Any(m => m.Group == s.Group && s.LP < 64 && ((m.Mask >> s.LP) & 1) != 0))
                    if (!d.TryGetValue(s.LLC, out var cur) || c.Size > cur) d[s.LLC] = c.Size;   // the MAXIMUM
        return d;
    }

    /// <summary>The domain with the extra cache: two or more domains, sizes known, largest ≥ 2× smallest, only one of that size.</summary>
    public static byte? DetectBigCache(Topo t)
    {
        if (t.LLCGroups.Count < 2) return null;
        if (t.LLCGroups.Any(l => !t.L3ByLlc.ContainsKey(l))) return null;
        var sz = t.LLCGroups.Select(l => t.L3ByLlc[l]).ToList();
        ulong max = sz.Max(), min = sz.Min();
        if (min == 0 || max < 2 * min) return null;
        var big = t.LLCGroups.Where(l => t.L3ByLlc[l] == max).ToList();
        return big.Count == 1 ? big[0] : (byte?)null;
    }

    /// <summary>The model from CPU Sets and caches.</summary>
    public static Topo Build(List<CpuSet> sets, List<L3Cache> caches, string name, int reported)
    {
        var s = sets.OrderBy(x => x.Group).ThenBy(x => x.LP).ToList();
        var t = new Topo
        {
            Sets = s,
            Name = name ?? "",
            Reported = reported,
            LogicalCount = s.Count,
            PhysicalCount = s.Select(x => (x.Group, x.Core)).Distinct().Count(),
            Groups = s.Select(x => x.Group).Distinct().Count(),
            EffClasses = s.Select(x => x.Eff).Distinct().OrderBy(x => x).ToList(),
            LLCGroups = s.Select(x => x.LLC).Distinct().OrderBy(x => x).ToList(),
        };
        t.SMT = t.LogicalCount > t.PhysicalCount;
        t.L3ByLlc = L3Map(s, caches ?? new List<L3Cache>());
        t.BigCacheLlc = DetectBigCache(t);
        return t;
    }

    /// <summary>A bit mask of the sets' logical processors (indexes ≥ 64 dropped).</summary>
    public static ulong Mask(IEnumerable<CpuSet> sets)
    {
        ulong m = 0;
        foreach (var s in sets)
            if (s.LP < 64) m |= 1UL << s.LP;
        return m;
    }

    public static int PopCount(ulong m) => System.Numerics.BitOperations.PopCount(m);
}
