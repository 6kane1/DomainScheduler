using Xunit;

namespace DomainScheduler.Tests;

public class MaskTests
{
    [Fact]
    public void SpecRoundTrip()
    {
        Assert.Equal("ccd1+nosmt", Masks.ToSpec("ccd1+nosmt"));
        Assert.Equal("ccd1+nosmt", Masks.TryParseSpec(Masks.ToSpec("ccd1+nosmt")).ToString());
        Assert.Equal("vcache+nocore0", Masks.ToSpec("v; nocore0"));
        Assert.Equal("game+nosmt", Masks.ToSpec("g,nosmt"));
        Assert.Null(Masks.ToSpec("0xff"));
        Assert.Null(Masks.ToSpec("vcache+game"));
    }

    [Fact]
    public void NumericForms()
    {
        Assert.True(Masks.Parse("0xFF", out ulong a)); Assert.Equal(0xFFUL, a);
        Assert.True(Masks.Parse("255", out ulong b)); Assert.Equal(255UL, b);
        Assert.True(Masks.Parse("0,2,4", out ulong c)); Assert.Equal(0x15UL, c);
        var t = Synth.I12400();
        var r = Masks.Resolve(t, "0x3F000");
        Assert.False(r.Satisfied);
        var r2 = Masks.Resolve(t, "0x3F");
        Assert.False(r2.Portable);
        Assert.Equal("0x3f", r2.Spec);
        var lim = Masks.Limit(t, 0x1FFFUL);
        Assert.Equal(0xFFFUL, lim.Kept);
        Assert.Equal(0x1000UL, lim.Dropped);
        Assert.NotEmpty(lim.Note);
    }

    [Fact]
    public void Specs()
    {
        var x = Synth.X7950X3D();
        var v = Masks.Resolve(x, "vcache");
        Assert.True(v.Satisfied);
        Assert.Equal(0xFFFFUL, v.Mask);
        Assert.Equal(1, v.Domains);
        Assert.Equal(256u, v.SetIds[0]);   // CPU Set IDs, not LP indexes
        Assert.Equal(0x5555UL, Masks.Resolve(x, "vcache+nosmt").Mask);
        Assert.Equal(0xFFFF0000UL, Masks.Resolve(x, "ccd1").Mask);
        Assert.Equal(0xFFFF0000UL, Masks.Resolve(x, "nongame").Mask);
        Assert.Equal(0xFFFFFFFCUL, Masks.Resolve(x, "all+nocore0").Mask);
        var h = Synth.I13900K();
        var hv = Masks.Resolve(h, "vcache");
        Assert.False(hv.Satisfied);
        Assert.Equal(h.FullMask(), hv.Mask);
        Assert.Equal(0xFFFFUL, Masks.Resolve(h, "pcores").Mask);
        Assert.Equal(0xFFFF0000UL, Masks.Resolve(h, "ecores").Mask);
        Assert.False(Masks.Resolve(Synth.I12400(), "game").Satisfied);
        Assert.False(Masks.Resolve(Synth.I12400(), "ccd1").Satisfied);
        Assert.Equal("0-7,16-23", Masks.Ranges(0xFF00FFUL));
    }

    [Fact]
    public void IdealCandidatesOnePerCore()
    {
        foreach (var t in new[] { Synth.X7950X3D(), Synth.I13900K(), Synth.X7800X3D(), Synth.I12400(), Synth.Six() })
        {
            var c = Masks.IdealCandidates(t, false, 0);
            if (c.Count >= 6) Assert.Equal(c.Count, c.Select(x => x.Core).Distinct().Count());
        }
        var x = Masks.IdealCandidates(Synth.X7950X3D(), false, 0xFFFF);
        Assert.Equal(8, x.Count);
        Assert.Equal(new byte[] { 0, 2, 4, 6, 8, 10, 12, 14 }, x.Select(s => s.LP).ToArray());
        var h = Masks.IdealCandidates(Synth.I13900K(), false, 0);
        Assert.Equal(8, h.Count);
        Assert.All(h, s => Assert.Equal(1, s.Eff));
        var four = Synth.Symmetric(4, true, 4, new[] { 8 * Synth.MB }, "4c");
        var f = Masks.IdealCandidates(four, false, 0);
        Assert.Equal(8, f.Count);
        Assert.Equal(1, f[4].LP);   // SMT siblings appended at the end
        Assert.NotEqual(0, Masks.IdealCandidates(Synth.Six(), true, 0)[0].LP);
    }

    [Fact]
    public void BgMaskOrder()
    {
        var x = Synth.X7950X3D();
        Assert.Equal(0xFFFF0000UL, Masks.BgMask(x, 0xFFFF));
        Assert.Equal(0x2UL, Masks.BgMask(x, 0xFFFFFFFD));
        Assert.Equal(0x3UL, Masks.BgMask(x, 0xFFFFFFFF));
    }
}
