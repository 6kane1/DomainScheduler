using System.Buffers.Binary;
using Xunit;

namespace DomainScheduler.Tests;

[Collection("static-hooks")]
public class TopoTests
{
    [Fact]
    public void ParsesCpuSetBuffer()
    {
        var buf = new byte[96];
        void Put(int off, int type, uint id, byte lp, byte core, byte llc, byte eff)
        {
            BinaryPrimitives.WriteInt32LittleEndian(buf.AsSpan(off), 32);
            BinaryPrimitives.WriteInt32LittleEndian(buf.AsSpan(off + 4), type);
            BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(off + 8), id);
            buf[off + 14] = lp; buf[off + 15] = core; buf[off + 16] = llc; buf[off + 18] = eff;
        }
        Put(0, 0, 256, 0, 0, 0, 1);
        Put(32, 7, 0, 0, 0, 0, 0);       // another type: skipped
        Put(64, 0, 257, 1, 0, 0, 1);
        var sets = TopoBuilder.ParseCpuSets(buf);
        Assert.Equal(2, sets.Count);
        Assert.Equal(257u, sets[1].Id);
        Assert.Equal(1, sets[1].LP);
        Assert.Empty(TopoBuilder.ParseCpuSets(new byte[] { 1, 2, 3 }));
    }

    static byte[] CacheRecord(byte level, int type, uint size, ushort groups, int recSize = 56)
    {
        var b = new byte[recSize];
        BinaryPrimitives.WriteInt32LittleEndian(b.AsSpan(0), 2);
        BinaryPrimitives.WriteInt32LittleEndian(b.AsSpan(4), recSize);
        b[8] = level;
        BinaryPrimitives.WriteUInt32LittleEndian(b.AsSpan(12), size);
        BinaryPrimitives.WriteInt32LittleEndian(b.AsSpan(16), type);
        BinaryPrimitives.WriteUInt16LittleEndian(b.AsSpan(38), groups);
        BinaryPrimitives.WriteUInt64LittleEndian(b.AsSpan(40), 0xFFFF);
        return b;
    }

    [Fact]
    public void ParsesL3AndRejectsBrokenBuffers()
    {
        var l3 = CacheRecord(3, 0, (uint)(32 * Synth.MB), 1);
        var l2 = CacheRecord(2, 0, 1 << 20, 1);
        var instr = CacheRecord(3, 1, 1 << 20, 1);
        var caches = TopoBuilder.ParseL3(l3.Concat(l2).Concat(instr).ToArray());
        Assert.Single(caches);
        Assert.Equal(32 * Synth.MB, caches[0].Size);
        Assert.Equal(0xFFFFUL, caches[0].Masks[0].Mask);
        // A record size past the end, or masks past the record: nothing, no exception.
        var broken = (byte[])l3.Clone();
        BinaryPrimitives.WriteInt32LittleEndian(broken.AsSpan(4), 500);
        Assert.Empty(TopoBuilder.ParseL3(broken));
        Assert.Empty(TopoBuilder.ParseL3(CacheRecord(3, 0, 1, 3)));
    }

    [Fact]
    public void L3MapTakesTheMaximum()
    {
        var sets = new List<CpuSet> { new CpuSet { LP = 0, LLC = 0 }, new CpuSet { LP = 1, LLC = 0 } };
        var a = new L3Cache { Size = 32 * Synth.MB }; a.Masks.Add((0, 1));
        var b = new L3Cache { Size = 96 * Synth.MB }; b.Masks.Add((0, 2));
        Assert.Equal(96 * Synth.MB, TopoBuilder.L3Map(sets, new List<L3Cache> { a, b })[0]);
    }

    [Fact]
    public void Counts()
    {
        var t = Synth.X7950X3D();
        Assert.Equal(32, t.LogicalCount);
        Assert.Equal(16, t.PhysicalCount);
        Assert.True(t.SMT);
        Assert.Equal(1, t.Groups);
        Assert.Equal(2, t.LLCGroups.Count);
        Assert.NotEmpty(t.L3ByLlc);
        var h = Synth.I13900K();
        Assert.True(h.Hybrid);
        Assert.Equal(24, h.PhysicalCount);
        Assert.Equal(0xFFFFFFFFUL, h.FullMask());
    }

    [Fact]
    public void DetectBigCache()
    {
        Assert.Equal((byte)0, TopoBuilder.DetectBigCache(Synth.X7950X3D()));
        Assert.Null(TopoBuilder.DetectBigCache(Synth.X7950X()));
        Assert.Null(TopoBuilder.DetectBigCache(Synth.X7800X3D()));
        var missing = Synth.X7950X3D();
        missing.L3ByLlc.Remove(missing.LLCGroups[1]);
        Assert.Null(TopoBuilder.DetectBigCache(missing));
    }

    [Fact]
    public void LiveHookIsUsed()
    {
        Topo.LiveHook = Synth.I13900K;
        try { Assert.True(TopoBuilder.Live().Hybrid); }
        finally { Topo.LiveHook = null; }
    }
}

public class SplitTests
{
    [Fact]
    public void X3DSplitsByTheLargerCache()
    {
        var sp = Synth.X7950X3D().Split;
        Assert.Equal("x3d", sp.Family);
        Assert.True(sp.CanSplit);
        Assert.Equal(0xFFFFUL, sp.Game);
        Assert.Equal(0xFFFF0000UL, sp.NonGame);
        Assert.False(sp.UpperHalf);
        Assert.Equal(32, sp.Expected);
        var up = Synth.Symmetric(16, true, 8, new[] { 32 * Synth.MB, 96 * Synth.MB }, "AMD Ryzen 9 9950X3D 16-Core Processor");
        Assert.True(up.Split.UpperHalf);
        var hidden = Synth.Symmetric(12, true, 6, new[] { 96 * Synth.MB, 32 * Synth.MB }, "AMD Ryzen 9 7950X3D");
        Assert.Equal("sku", hidden.Split.Code);
        Assert.False(hidden.Split.CanSplit);
    }

    [Fact]
    public void Others()
    {
        var x = Synth.X7950X().Split;
        Assert.Equal("equal", x.Code);
        Assert.False(x.CanSplit);
        var h = Synth.I13900K().Split;
        Assert.Equal("hybrid", h.Family);
        Assert.True(h.CanSplit);
        Assert.Equal(0xFFFFUL, h.Game);
        Assert.Equal(8, h.PCores);
        Assert.Equal(16, h.ECores);
        foreach (var t in new[] { Synth.I12400(), Synth.Six(), Synth.X7800X3D() })
        {
            Assert.Equal("single", t.Split.Code);
            Assert.False(t.Split.CanSplit);
            Assert.False(string.IsNullOrEmpty(t.Split.Why()));
        }
        var probe = Synth.Symmetric(6, true, 6, new[] { 18 * Synth.MB }, "x", reported: 16);
        Assert.Equal("probe", probe.Split.Code);
    }

    [Fact]
    public void SmtWhole()
    {
        var t = Synth.X7950X3D();
        Assert.True(CoreSplit.SmtWhole(t, 0xF));
        Assert.False(CoreSplit.SmtWhole(t, 0x1));
    }
}
