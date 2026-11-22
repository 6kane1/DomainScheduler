using System.Diagnostics;
using DomainScheduler;
using Xunit;

namespace DomainScheduler.Tests;

public class ReadyProbeTests
{
    [Fact]
    public void AWindowGivesP99WaitsOfGameThreadsAndTheirHardFaults()
    {
        var p = new ReadyProbe();
        double tpm = Stopwatch.Frequency / 1000.0;
        p.TestWindow(100, pr =>
        {
            pr.OnThread(100, 11); pr.OnThread(100, 12); pr.OnThread(200, 99);
            long t = 1_000_000;
            for (int i = 0; i < 100; i++) { pr.OnReady(11, t); pr.OnSwitch(11, t + (long)((i >= 98 ? 2.0 : 0.01) * tpm)); t += (long)(5 * tpm); }
            for (int i = 0; i < 2; i++) { pr.OnReady(12, t); pr.OnSwitch(12, t + (long)(3 * tpm)); t += (long)(5 * tpm); }   // too few waits
            pr.OnReady(99, t); pr.OnSwitch(99, t + (long)(9 * tpm));                                                       // not the game
            for (int i = 0; i < 25; i++) pr.OnHardFault(11);
            for (int i = 0; i < 40; i++) pr.OnHardFault(99);
        }, 0.25);
        Assert.InRange(p.WaitP99Ms(11), 1.9, 2.1);          // p99 of 100 waits with two at 2 ms = 2 ms
        Assert.Equal(-1, p.WaitP99Ms(12));
        Assert.Equal(-1, p.WaitP99Ms(99));
        Assert.Equal(100, p.HardFaultsPerSec!.Value, 1);    // 25 in a quarter of a second
    }
}
