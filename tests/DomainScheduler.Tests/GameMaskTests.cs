using Xunit;

namespace DomainScheduler.Tests;

[Collection("static-hooks")]
public class GameMaskTests
{
    [Fact]
    public void AdjustAndGuard()
    {
        var x = Synth.X7950X3D();
        ulong m = GameMask.Adjust(x, 0xFFFF, 0, 0, 0x10, GameMask.CoreMode, out ulong drop, out bool ign);
        Assert.Equal(0xFFCFUL, m);
        Assert.Equal(0x30UL, drop);
        Assert.False(ign);
        Assert.Equal(0xFFEFUL, GameMask.Adjust(x, 0xFFFF, 0, 0, 0x10, GameMask.ThreadMode, out _, out _));
        // A cut of more than half the mask is ignored and the mask stays as it is.
        m = GameMask.Adjust(x, 0xFF, 0, 0, 0x3F, GameMask.CoreMode, out drop, out ign);
        Assert.Equal(0xFFUL, m);
        Assert.Equal(0UL, drop);
        Assert.True(ign);
        Assert.Equal(0x1FEUL, GameMask.Adjust(x, 0xFF, 0x100, 0x1, 0, GameMask.Off, out _, out _));
    }

    [Fact]
    public void DedicatedPinsOnly()
    {
        var x = Synth.X7950X3D();
        Assert.True(GameMask.Dedicated(x, 0x30));
        Assert.False(GameMask.Dedicated(x, 0x50));
        GameMask.IrqHook = () => new() { new("GPU", 0x30), new("NIC", 0xF0) };
        try
        {
            var info = GameMask.ForRule(x, 0xFFFF, GameMask.CoreMode);
            Assert.Equal(0x30UL, info.Irq);
            Assert.Single(info.IrqWide);
            Assert.Equal(0xFF0FUL, info.Final);      // 22.8: the wide pin takes out only its own processors
            Assert.Contains(GameMask.WideNote, info.Notes);
            Assert.True(info.Adjusted);
        }
        finally { GameMask.IrqHook = null; }
    }

    [Fact]
    public void EditsFromAnotherProcessorAreStale()
    {
        var x = Synth.X7950X3D();
        GameMask.IrqHook = () => new();
        try
        {
            var stale = GameMask.Build(x, "vcache", new GameMask.Edits { Add = 0x10000, Sig = "8|8|1|0" }, GameMask.Off);
            Assert.True(stale.EditsStale);
            Assert.Equal(0xFFFFUL, stale.Final);
            Assert.Equal("vcache", GameMask.Rule(stale));
            var ok = GameMask.Build(x, "vcache", new GameMask.Edits { Add = 0x10000, Sig = GameMask.CpuSig(x) }, GameMask.Off);
            Assert.Equal(0x1FFFFUL, ok.Final);
            Assert.Equal("0x1ffff", GameMask.Rule(ok));
            Assert.Equal(0xFFFC0000UL, GameMask.Background(x, ok));   // core 8 (LP 16,17) is partly the game's: whole free cores only
        }
        finally { GameMask.IrqHook = null; }
    }
}
