using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

public class LayoutInPlaceTests
{
    [Fact]
    public void OnlyAFullMatchCountsAsInPlace()
    {
        Assert.True(IrqAuto.LayoutInPlace(new (ulong?, ulong)[] { (0b100, 0b100), (null, 0), (0, 0) }));
        Assert.False(IrqAuto.LayoutInPlace(new (ulong?, ulong)[] { (0b100, 0b1000) }));        // another core
        Assert.False(IrqAuto.LayoutInPlace(new (ulong?, ulong)[] { (null, 0b100) }));          // not written yet
        Assert.False(IrqAuto.LayoutInPlace(new (ulong?, ulong)[] { (0b100, 0) }));             // pinned, plan says Windows
        Assert.False(IrqAuto.LayoutInPlace(System.Array.Empty<(ulong?, ulong)>()));              // nothing to compare
    }
}
