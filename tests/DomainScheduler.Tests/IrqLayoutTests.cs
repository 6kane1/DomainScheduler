using Xunit;

namespace DomainScheduler.Tests;

/// <summary>The automatic interrupt layout (DEVICE-TWEAKER's AUTO rules) and the device recognition behind it.</summary>
public class IrqLayoutTests
{
    static IrqSlot S(IrqRole r, string name = null) => new() { Key = name ?? r.ToString(), Name = name ?? r.ToString(), Role = r };

    static Dictionary<string, ulong> Plan(Topo t, IReadOnlyDictionary<int, int> perf, params IrqSlot[] slots) =>
        IrqLayout.Plan(t, slots, perf).ToDictionary(s => s.Name, s => s.Mask);

    [Fact]
    public void Ryzen7800X3D_mouse_gpu_nic_get_the_layout_of_the_original()
    {
        var m = Plan(Synth.X7800X3D(), null, S(IrqRole.Gpu), S(IrqRole.Net), S(IrqRole.Mouse));
        Assert.Equal(0x4UL, m["Mouse"]);            // LP2
        Assert.Equal(0x5000UL, m["Gpu"]);           // LP12 + LP14
        Assert.Equal(0x40UL, m["Net"]);             // LP6 (LP4 stays empty after the GPU)
    }

    [Fact]
    public void A_separate_keyboard_controller_takes_the_next_core_after_the_nic_spacer()
    {
        var m = Plan(Synth.X7800X3D(), null, S(IrqRole.Mouse), S(IrqRole.Gpu), S(IrqRole.Net), S(IrqRole.Keyboard));
        Assert.Equal(0x400UL, m["Keyboard"]);       // LP10
        Assert.Equal(0x40UL, m["Net"]);
    }

    [Fact]
    public void Without_free_cores_the_keyboard_shares_the_mouse_core_through_its_sibling()
    {
        // 3 cores for 4 slots: too few to avoid core 0, so the mouse takes it; no core is left for the keyboard.
        var t = Synth.Symmetric(3, true, 3, new[] { 32 * Synth.MB }, "3 cores");
        var m = Plan(t, null, S(IrqRole.Mouse), S(IrqRole.Gpu), S(IrqRole.Keyboard));
        Assert.Equal(0x1UL, m["Mouse"]);
        Assert.Equal(0x14UL, m["Gpu"]);             // LP2 + LP4
        Assert.Equal(0x2UL, m["Keyboard"]);         // LP1 = the mouse core's second thread
    }

    [Fact]
    public void Ryzen7950X3D_devices_go_to_the_ccd_without_vcache()
    {
        var m = Plan(Synth.X7950X3D(), null, S(IrqRole.Mouse), S(IrqRole.Gpu), S(IrqRole.Net));
        Assert.Equal(1UL << 16, m["Mouse"]);
        Assert.Equal((1UL << 28) | (1UL << 30), m["Gpu"]);
        Assert.Equal(1UL << 20, m["Net"]);
    }

    [Fact]
    public void Intel_hybrid_uses_only_p_cores()
    {
        var m = Plan(Synth.I13900K(), null, S(IrqRole.Mouse), S(IrqRole.Gpu), S(IrqRole.Net));
        Assert.Equal(0x4UL, m["Mouse"]);
        Assert.Equal(0x5000UL, m["Gpu"]);
        Assert.Equal(0x40UL, m["Net"]);
        Assert.True(m.Values.All(v => v < (1UL << 16)));
    }

    [Fact]
    public void Cppc_ranking_puts_the_mouse_on_the_best_core()
    {
        var perf = Enumerable.Range(0, 16).ToDictionary(lp => lp, lp => lp is 6 or 7 ? 100 : 90);
        var m = Plan(Synth.X7800X3D(), perf, S(IrqRole.Mouse), S(IrqRole.Gpu), S(IrqRole.Net));
        Assert.Equal(0x40UL, m["Mouse"]);           // LP6, the best-ranked core
        Assert.Equal(0x5000UL, m["Gpu"]);
        Assert.Equal(0x10UL, m["Net"]);             // LP4 (LP2 is the spacer after the GPU)
    }

    [Fact]
    public void A_flat_ranking_is_ignored()
    {
        var perf = Enumerable.Range(0, 16).ToDictionary(lp => lp, _ => 100);
        Assert.Null(IrqLayout.Ranks(Synth.X7800X3D(), perf));
    }

    [Fact]
    public void Device_recognition_follows_the_rules()
    {
        Assert.True(IrqAuto.Excluded(@"PCI\VEN_1022&DEV_14DB&SUBSYS_0", "PCI bridge", "System"));
        Assert.True(IrqAuto.Excluded(@"BTH\X", "Intel(R) Wireless Bluetooth(R)", "Bluetooth"));
        Assert.True(IrqAuto.Excluded(@"ROOT\NET\0000", "Hyper-V Virtual Ethernet Adapter", "Net"));
        Assert.False(IrqAuto.Excluded(@"PCI\VEN_10DE&DEV_2D05", "NVIDIA GeForce RTX 5060", "Display"));
        Assert.Equal(AutoKind.Usb, IrqAuto.Classify("USB", "AMD USB 3.10 eXtensible Host Controller - 1.10 (Microsoft)"));
        Assert.Equal(AutoKind.Stor, IrqAuto.Classify("SCSIAdapter", "Standard NVM Express Controller"));
        Assert.Equal(AutoKind.Audio, IrqAuto.Classify("MEDIA", "Realtek(R) Audio"));
        Assert.Null(IrqAuto.Classify("System", "AMD GPIO Controller"));
        Assert.True(IrqAuto.IsIntegratedGpu("", "AMD Radeon(TM) Graphics"));
        Assert.False(IrqAuto.IsIntegratedGpu("", "NVIDIA GeForce RTX 5060"));
        Assert.True(IrqAuto.IsWifi("", "Intel(R) Wi-Fi 6E AX211 160MHz", ""));
        Assert.False(IrqAuto.IsWifi("", "Realtek PCIe 2.5GbE Family Controller", "rt25cx21x64"));
        Assert.True(IrqAuto.IsDisplayAudioDevice(@"HDAUDIO\FUNC_01&VEN_10DE&DEV_0099", "NVIDIA High Definition Audio"));
        Assert.True(IrqAuto.IsSpdif("Realtek Digital Output (S/PDIF)"));
        Assert.False(IrqAuto.IsSpdif("LG ULTRAGEAR (NVIDIA High Definition Audio) HDMI"));
    }

    [Fact]
    public void Two_gpus_with_a_geforce_make_the_plain_radeon_integrated()
    {
        var g = new List<AutoDevice>
        {
            new() { Id = @"PCI\VEN_10DE&DEV_2D05", Name = "NVIDIA GeForce RTX 5060", Kind = AutoKind.Gpu },
            new() { Id = @"PCI\VEN_1002&DEV_164E", Name = "AMD Radeon(TM) Graphics", Kind = AutoKind.Gpu },
        };
        IrqAuto.MarkIntegrated(g);
        Assert.False(g[0].Igpu);
        Assert.True(g[1].Igpu);
    }

    [Fact]
    public void Hid_roles_from_usage_class_and_product()
    {
        Assert.Equal(IrqRole.Mouse, IrqAuto.HidRole(1, 2, "HIDClass", "HID-compliant mouse", "G Pro Wireless"));
        Assert.Equal(IrqRole.Keyboard, IrqAuto.HidRole(1, 6, "Keyboard", "HID Keyboard Device", ""));
        Assert.Equal(IrqRole.Controller, IrqAuto.HidRole(1, 5, "HIDClass", "HID-compliant game controller", "Xbox Wireless Controller"));
        Assert.Null(IrqAuto.HidRole(0xC, 1, "HIDClass", "HID-compliant consumer control device", ""));
        Assert.Null(IrqAuto.HidRole(1, 2, "HIDClass", "x", "Xbox Wireless Controller"));   // product says pad, usage says mouse: no role
    }

    [Fact]
    public void Nic_mode_and_cppc_parsing()
    {
        Assert.Equal("BOTH", IrqAuto.ChooseNicMode(true, true, false, 4, 4));
        Assert.Equal("BOTH", IrqAuto.ChooseNicMode(true, false, false, 2, 1));
        Assert.Equal("RSS", IrqAuto.ChooseNicMode(true, false, false, 1, 1));
        Assert.Equal("IRQ", IrqAuto.ChooseNicMode(false, false, false, 0, 0));
        string xml = "<Events><Event><Data Name='Group'>0</Data><Data Name='Number'>3</Data><Data Name='MaximumPerformancePercent'>120</Data></Event>"
                   + "<Event><Data Name='Group'>0</Data><Data Name='Number'>3</Data><Data Name='MaximumPerformancePercent'>99</Data></Event></Events>";
        Assert.Equal(120, IrqAuto.ParseCppc(xml)[3]);   // newest first wins
    }
}
