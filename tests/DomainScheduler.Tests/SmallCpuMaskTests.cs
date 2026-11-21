using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

/// <summary>1.16.0-test1.7: on 6 cores or fewer (one cache domain, no E-cores) Auto gives the game every processor; 8+ keep the mask.</summary>
public class SmallCpuMaskTests
{
    [Fact]
    public void SixCoresOrFewerKeepAllCores()
    {
        Assert.False(AutoLogic.KeepsAllCores(Synth.X7800X3D(), "auto"));   // 8 cores: the mask as before
        Assert.True(AutoLogic.KeepsAllCores(Synth.I12400(), "auto"));
        Assert.True(AutoLogic.KeepsAllCores(Synth.Six(), null));
    }

    [Fact]
    public void BigOrHybridProcessorsKeepTheMask()
    {
        Assert.False(AutoLogic.KeepsAllCores(Synth.X7950X3D(), "auto"));
        Assert.False(AutoLogic.KeepsAllCores(Synth.X7950X(), "auto"));
        Assert.False(AutoLogic.KeepsAllCores(Synth.I13900K(), "auto"));
    }

    [Fact]
    public void SettingOverridesTheRule()
    {
        Assert.False(AutoLogic.KeepsAllCores(Synth.I12400(), "on"));
        Assert.True(AutoLogic.KeepsAllCores(Synth.X7800X3D(), "off"));
        Assert.True(AutoLogic.KeepsAllCores(Synth.X7950X3D(), "off"));
        Assert.True(AutoLogic.KeepsAllCores(Synth.X7800X3D(), " OFF "));
        Assert.False(AutoLogic.KeepsAllCores(null, "auto"));
    }

    [Fact]
    public void AllCoresGivesTheFullMaskAndKeepsUnsatisfiedPlans()
    {
        var t = Synth.I12400();
        var mp = new AutoLogic.MaskPlan { Spec = "auto", Mask = 0x0FF, Satisfied = true };
        var all = AutoLogic.AllCores(t, mp);
        Assert.Equal(t.FullMask(), all.Mask);
        Assert.Equal(0xFFFUL, all.Mask);
        Assert.True(all.Satisfied);
        var no = new AutoLogic.MaskPlan { Spec = "auto", Satisfied = false };
        Assert.Same(no, AutoLogic.AllCores(t, no));     // an unsatisfiable plan stays as it is (no mask at all)
    }
}
