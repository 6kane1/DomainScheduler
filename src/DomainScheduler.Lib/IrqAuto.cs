using System.Globalization;
using System.Management;
using System.Runtime.InteropServices;
using System.Runtime.Intrinsics.X86;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using Microsoft.Win32;

namespace DomainScheduler;

public enum AutoKind { Usb, Gpu, Audio, Net, Stor }

/// <summary>A device of the automatic layout and everything AUTO needs to know about it.</summary>
public sealed class AutoDevice
{
    public string Id = "", Name = "", Class = "", Service = "", DriverKey = "";
    public uint DevInst;
    public AutoKind Kind;
    public bool Igpu, Wifi, DisplayAudio, NetCx;
    public string StorTag = "";
    public IrqRole? Role;                       // USB: the most important input role found on the controller
    public List<string> Endpoints = new();      // AUDIO: names of its sound endpoints
    public IrqSlot Slot;                        // filled by PlanNow for USB with a role, discrete GPU, wired NIC
    public string NicMode = "";                 // NDIS: IRQ, RSS or BOTH
}

/// <summary>
/// The automatic interrupt layout on a real PC, following DEVICE-TWEAKER's AUTO mode (its behaviour re-implemented
/// from its open-source description; no code was copied; its kernel-driver features are not part of it):
/// the device list and filters, the roles of USB controllers from their HID devices, <see cref="IrqLayout"/> for the
/// cores, then the writes per kind (MSI where the device supports it, priority, affinity policy, the network card's
/// RSS, USB and network power saving off on desktops, the USB selective suspend of the power plan on mains power,
/// ReservedCpuSets removed).
/// Safety rules (1.12.0): storage controllers and the boot device are never touched; MSI is switched on only when the
/// device already offers it; on a PC with a battery or Modern Standby no power-saving value is changed; no battery (DC)
/// power value is ever written; <see cref="BootGuard"/> reverts everything when Windows does not start fine afterwards.
/// Every value is backed up before it is first changed; <see cref="RevertExtras"/> puts all of it back.
/// It takes effect after a restart of Windows; no device is restarted.
/// </summary>
public static class IrqAuto
{
    // ---- classification (pure, tested) -------------------------------------------------------------------

    static readonly Regex ExcludedId = new(@"^ACPI\\PNP0100|^ACPI\\AMDI00(10|30)\\|^ACPI\\PNP0103|^ACPI\\PNP0501|^PCI\\VEN_1022&DEV_14DB|^PCI\\VEN_1022&DEV_14DD|^ACPI\\INTC1055\\|^PCI\\VEN_8086&DEV_51E8|^PCI\\VEN_8086&DEV_A73E", RegexOptions.IgnoreCase);
    static readonly Regex VirtualNet = new("virtual|loopback|hyper-v|vmware|wan miniport|tap|tun|isatap|teredo|npcap", RegexOptions.IgnoreCase);
    static readonly Regex SoftAudio = new("vb-?audio|voicemeeter|virtual cable|virtual audio|broadcast|software device", RegexOptions.IgnoreCase);
    static readonly Regex SystemBridge = new(@"PCI Express Root Port|PCI-to-PCI Bridge|PCI standard .*bridge|SMBus|LPC Controller|ISA Bridge|I2C Controller|SPI Controller|GPIO Controller|PS/2 Controller", RegexOptions.IgnoreCase);
    static readonly Regex Serial = new(@"\b(COM\d+|Serial Port|Communications Port)", RegexOptions.IgnoreCase);

    /// <summary>The exclusions applied before a device is classified.</summary>
    public static bool Excluded(string id, string name, string cls)
    {
        id ??= ""; name ??= ""; cls ??= "";
        bool C(string c) => cls.Equals(c, StringComparison.OrdinalIgnoreCase);
        if (ExcludedId.IsMatch(id)) return true;
        if (C("System") && name.Contains("ACPI", StringComparison.OrdinalIgnoreCase)) return true;
        if (C("Ports") || Serial.IsMatch(name)) return true;
        if (C("Bluetooth") || name.Contains("Bluetooth", StringComparison.OrdinalIgnoreCase)) return true;
        if (C("Net") && VirtualNet.IsMatch(name)) return true;
        if (C("SoftwareDevice") || SoftAudio.IsMatch(name)) return true;
        if (C("System") && SystemBridge.IsMatch(name)) return true;
        if (Regex.IsMatch(name, @"Intel\s*\(R\)\s*RST VMD Controller", RegexOptions.IgnoreCase)) return true;
        if (C("Display") && (name.Contains("VirtualBox", StringComparison.OrdinalIgnoreCase) || id.Contains("VEN_80EE", StringComparison.OrdinalIgnoreCase))) return true;
        return false;
    }

    static readonly Regex StorName = new(@"NVM\s*Express|NVMe|\bSATA\b|AHCI|\bRAID\b|SCSI|Storage|\bVMD\b|\bIDE\b|Mass Storage|Disk", RegexOptions.IgnoreCase);
    static readonly Regex StorService = new(@"^(stornvme|storahci|storport|stornvm\w*|iaStor\w*|iaVROC|rcraid|rcbottom|amd_sata|amdsata|amd_xata|amdxata|nvme\w*|secnvme|samsungnvme\w*|msahci|pciide|intelide|atapi|vmd|megasas\w*|lsi_\w*|arcsas|percsas\w*|stexstor|mvs\w*|disk|uaspstor|usbstor)$", RegexOptions.IgnoreCase);

    /// <summary>
    /// A storage controller (NVMe, SATA/AHCI, RAID, VMD, SCSI) or a disk: never touched by the interrupt layout or a
    /// manual pin. The boot disk always hangs on one of these, so a wrong value there can stop Windows from starting.
    /// </summary>
    public static bool IsStorage(string cls, string name, string service)
    {
        cls ??= "";
        foreach (var c in new[] { "SCSIAdapter", "HDC", "IDE", "Storage", "DiskDrive", "Volume", "StorageVolume" })
            if (cls.Equals(c, StringComparison.OrdinalIgnoreCase)) return true;
        return StorName.IsMatch(name ?? "") || StorService.IsMatch(service ?? "");
    }

    /// <summary>
    /// Whether MSISupported = 1 may be written (pure). Only when the device already offers MSI: it uses message
    /// interrupts now, or its PCI interrupt support (DEVPKEY_PciDevice_InterruptSupport: 2 = MSI, 4 = MSI-X) says so,
    /// or — when that property cannot be read — its driver put a MessageNumberLimit into the MSI key. Unsure: no.
    /// </summary>
    public static bool MsiAllowed(int? interruptSupport, bool usesMsiNow, bool driverLimit)
    {
        if (usesMsiNow) return true;
        if (interruptSupport.HasValue) return (interruptSupport.Value & 6) != 0;
        return driverLimit;
    }

    /// <summary>
    /// Power saving of devices and the USB selective suspend may be changed only on a desktop on mains power with no
    /// battery and no Modern Standby (pure). BatteryFlag 128 = "no system battery"; 255 (unknown) counts as a battery.
    /// </summary>
    public static bool PowerSavingAllowed(byte batteryFlag, bool batteriesPresent, bool modernStandby) =>
        batteryFlag == 128 && !batteriesPresent && !modernStandby;

    /// <summary>The kind, or null when the device is not part of the list.</summary>
    public static AutoKind? Classify(string cls, string name)
    {
        cls ??= ""; name ??= "";
        bool C(string c) => cls.Equals(c, StringComparison.OrdinalIgnoreCase);
        if (C("USB") || Regex.IsMatch(name, "xHCI|Host Controller", RegexOptions.IgnoreCase)) return AutoKind.Usb;
        if (C("Display")) return AutoKind.Gpu;
        if (C("Net")) return AutoKind.Net;
        if (C("SCSIAdapter") || C("HDC") || C("IDE") || C("Storage")) return AutoKind.Stor;
        if (C("MEDIA") || C("AudioEndpoint") || Regex.IsMatch(name, "High Definition Audio|HD Audio|Audio Controller", RegexOptions.IgnoreCase)) return AutoKind.Audio;
        return null;
    }

    static readonly Regex Discrete = new(@"\bGeForce\b|\bRTX\b|\bGTX\b|\bQuadro\b|\bTesla\b|\bRadeon\s+RX\b|\bRadeon\s+Pro\b|Laptop\s+GPU\b|Intel\s*\(R\)\s*Arc\s*\(TM\)\s*A\d{3,}", RegexOptions.IgnoreCase);
    static readonly Regex Integrated = new(@"Intel\s*\(R\)\s*(UHD|HD|Iris( Xe)?)( Graphics)?\b|Intel\s*Arc\s*\(TM\)\s*Graphics\b|(AMD\s+)?Radeon\s*\(TM\)\s*Graphics\b|(AMD\s+)?Radeon\s*\(TM\)\s*\d{3,4}M\b", RegexOptions.IgnoreCase);

    public static bool IsIntegratedGpu(string id, string name)
    {
        string s = (id ?? "") + " " + (name ?? "");
        return !Discrete.IsMatch(s) && Integrated.IsMatch(s);
    }

    /// <summary>With two or more GPUs and one of them clearly discrete, any other plain Radeon is the integrated one.</summary>
    public static void MarkIntegrated(List<AutoDevice> gpus)
    {
        foreach (var g in gpus) g.Igpu = IsIntegratedGpu(g.Id, g.Name);
        if (gpus.Count < 2) return;
        var strong = new Regex(@"GeForce|RTX|GTX|Quadro|Laptop GPU|Intel\s*\(R\)\s*Arc\s*\(TM\)\s*A\d{3,}", RegexOptions.IgnoreCase);
        if (!gpus.Any(g => strong.IsMatch(g.Name))) return;
        foreach (var g in gpus)
            if (!strong.IsMatch(g.Name) && Regex.IsMatch(g.Id + " " + g.Name, "VEN_1002|Radeon", RegexOptions.IgnoreCase) && !Regex.IsMatch(g.Name, @"Radeon\s+(RX|Pro)\b", RegexOptions.IgnoreCase))
                g.Igpu = true;
    }

    static readonly Regex WifiService = new(@"^(netwtw\w*|rtwl\w*|rtwlane\w*|rtwlanu\w*|athw\w*|ath\w*|bcmwl\w*|qcwlan\w*|qwlan\w*|iwl\w*|mtk\w*)$", RegexOptions.IgnoreCase);
    public static bool IsWifi(string id, string name, string service) =>
        Regex.IsMatch(name ?? "", @"Wi-?Fi|Wireless|802\.11|WLAN", RegexOptions.IgnoreCase) || (id ?? "").Contains("VEN_14C3", StringComparison.OrdinalIgnoreCase)
        || WifiService.IsMatch(service ?? "");

    public static bool IsDisplayAudioDevice(string id, string name) =>
        Regex.IsMatch(id ?? "", @"\\VEN_10DE&DEV_(1A|22|26|28)[0-9A-F]{2}|\\VEN_1002&DEV_AA[0-9A-F]{2}|\\VEN_8086&DEV_28[0-9A-F]{2}", RegexOptions.IgnoreCase)
        || Regex.IsMatch(name ?? "", "Display Audio|HDMI Audio|NVIDIA High Definition Audio|AMD High Definition Audio", RegexOptions.IgnoreCase);

    public static bool IsDisplayEndpoint(string endpoint) =>
        Regex.IsMatch(endpoint ?? "", @"hdmi|display\s*audio|monitor|монитор|displayport|\bdp\b", RegexOptions.IgnoreCase);

    public static bool IsSpdif(string endpoint) =>
        Regex.IsMatch(endpoint ?? "", @"s/pdif|spdif|iec958|toslink|digital audio", RegexOptions.IgnoreCase) && !IsDisplayEndpoint(endpoint);

    static readonly string[] PadWords = { "gamepad", "game pad", "game controller", "wireless controller", "xbox", "xinput", "dualshock", "dualsense", "playstation", "ps3", "ps4", "ps5", "8bitdo", "joycon", "joy-con", "switch pro", "pro controller", "steam controller", "nintendo" };

    /// <summary>
    /// The role of one HID collection from its usage (page 1: 2 mouse, 6 keyboard, 4/5/8 game controller), its setup
    /// class and its product string (game controller words). Null = no input role.
    /// </summary>
    public static IrqRole? HidRole(int usagePage, int usage, string cls, string name, string product)
    {
        cls ??= "";
        IrqRole? u = usagePage == 1 ? usage switch { 2 => IrqRole.Mouse, 6 => IrqRole.Keyboard, 4 or 5 or 8 => IrqRole.Controller, _ => null } : null;
        IrqRole? pnp = cls.Equals("Keyboard", StringComparison.OrdinalIgnoreCase) ? IrqRole.Keyboard
            : cls.Equals("Mouse", StringComparison.OrdinalIgnoreCase) ? IrqRole.Mouse
            : Regex.IsMatch(name ?? "", "game controller|gamepad", RegexOptions.IgnoreCase) ? IrqRole.Controller : null;
        string pr = (product ?? "").ToLowerInvariant();
        bool audioWord = Regex.IsMatch(pr, "headset|headphone|audio|mic|speaker");
        IrqRole? prod = !audioWord && PadWords.Any(pr.Contains) ? IrqRole.Controller : null;
        if (prod != null) return u == null || u == prod ? prod : null;
        if (pnp == IrqRole.Controller) return pnp;
        return u ?? pnp;
    }

    // ---- reading the PC -----------------------------------------------------------------------------------

    const uint DIGCF_PRESENT = 0x2, DIGCF_ALLCLASSES = 0x4, DIGCF_DEVICEINTERFACE = 0x10;
    const uint SPDRP_DEVICEDESC = 0, SPDRP_SERVICE = 4, SPDRP_CLASS = 7, SPDRP_DRIVER = 9, SPDRP_FRIENDLYNAME = 12;
    const string EnumRoot = @"SYSTEM\CurrentControlSet\Enum\", ClassRoot = @"SYSTEM\CurrentControlSet\Control\Class\";
    const string ImSub = @"\Device Parameters\Interrupt Management";
    static readonly Guid HidGuid = new("4d1e55b2-f16f-11cf-88cb-001111000030");

    [StructLayout(LayoutKind.Sequential)] struct SP_DEVINFO_DATA { public uint cbSize; public Guid ClassGuid; public uint DevInst; public IntPtr Reserved; }
    [StructLayout(LayoutKind.Sequential)] struct SP_DEVICE_INTERFACE_DATA { public uint cbSize; public Guid Guid; public uint Flags; public IntPtr Reserved; }

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, EntryPoint = "SetupDiGetClassDevsW")] static extern IntPtr SetupDiGetClassDevs(IntPtr cls, string enumerator, IntPtr parent, uint flags);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, EntryPoint = "SetupDiGetClassDevsW")] static extern IntPtr SetupDiGetClassDevsGuid(ref Guid cls, string enumerator, IntPtr parent, uint flags);
    [DllImport("setupapi.dll")] static extern bool SetupDiEnumDeviceInfo(IntPtr set, uint i, ref SP_DEVINFO_DATA d);
    [DllImport("setupapi.dll")] static extern bool SetupDiEnumDeviceInterfaces(IntPtr set, IntPtr info, ref Guid g, uint i, ref SP_DEVICE_INTERFACE_DATA d);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, EntryPoint = "SetupDiGetDeviceInterfaceDetailW")] static extern bool SetupDiGetDeviceInterfaceDetail(IntPtr set, ref SP_DEVICE_INTERFACE_DATA d, IntPtr detail, uint size, out uint need, ref SP_DEVINFO_DATA info);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, EntryPoint = "SetupDiGetDeviceRegistryPropertyW")] static extern bool SetupDiGetDeviceRegistryProperty(IntPtr set, ref SP_DEVINFO_DATA d, uint prop, out uint type, byte[] buf, uint size, out uint need);
    [DllImport("setupapi.dll")] static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    [DllImport("cfgmgr32.dll")] static extern int CM_Get_Parent(out uint parent, uint devInst, uint flags);
    [DllImport("cfgmgr32.dll")] static extern int CM_Get_Child(out uint child, uint devInst, uint flags);
    [DllImport("cfgmgr32.dll")] static extern int CM_Get_Sibling(out uint sibling, uint devInst, uint flags);
    [DllImport("cfgmgr32.dll")] static extern int CM_Get_DevNode_Status(out uint status, out uint problem, uint devInst, uint flags);
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode, EntryPoint = "CM_Get_Device_IDW")] static extern int CM_Get_Device_ID(uint devInst, StringBuilder buf, int len, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, EntryPoint = "CreateFileW", SetLastError = true)] static extern IntPtr CreateFile(string path, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr template);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("hid.dll")] static extern bool HidD_GetPreparsedData(IntPtr h, out IntPtr data);
    [DllImport("hid.dll")] static extern bool HidD_FreePreparsedData(IntPtr data);
    [DllImport("hid.dll")] static extern int HidP_GetCaps(IntPtr data, IntPtr caps);
    [DllImport("hid.dll")] static extern bool HidD_GetProductString(IntPtr h, byte[] buf, int len);
    [StructLayout(LayoutKind.Sequential)] struct DEVPROPKEY { public Guid Fmtid; public uint Pid; }
    [DllImport("cfgmgr32.dll", EntryPoint = "CM_Get_DevNode_PropertyW")] static extern int CM_Get_DevNode_Property(uint devInst, ref DEVPROPKEY key, out uint type, byte[] buf, ref uint size, uint flags);
    [StructLayout(LayoutKind.Sequential)] struct SYSTEM_POWER_STATUS { public byte ACLineStatus, BatteryFlag, BatteryLifePercent, SystemStatusFlag; public int BatteryLifeTime, BatteryFullLifeTime; }
    [DllImport("kernel32.dll")] static extern bool GetSystemPowerStatus(out SYSTEM_POWER_STATUS s);
    [DllImport("powrprof.dll")] static extern bool GetPwrCapabilities(IntPtr caps);

    /// <summary>DEVPKEY_PciDevice_InterruptSupport (bit 1 line, 2 MSI, 4 MSI-X), or null when it cannot be read.</summary>
    static int? InterruptSupport(uint devInst)
    {
        try
        {
            var key = new DEVPROPKEY { Fmtid = new Guid("3ab22e31-8264-4b4e-9af5-a8d2d8e33e62"), Pid = 14 };
            var buf = new byte[8];
            uint size = (uint)buf.Length;
            if (CM_Get_DevNode_Property(devInst, ref key, out uint type, buf, ref size, 0) != 0 || size < 4 || type != 7 /* DEVPROP_TYPE_UINT32 */) return null;
            return BitConverter.ToInt32(buf, 0);
        }
        catch (Exception) { return null; }
    }

    /// <summary>The device offers MSI (see <see cref="MsiAllowed"/>).</summary>
    static bool MsiCapable(AutoDevice d)
    {
        bool usesMsi = false, limit = false;
        try { usesMsi = d.DevInst != 0 && Devices.Resources(d.DevInst).Any(r => r.Message); } catch (Exception) { }
        using (var k = Lm(EnumRoot + d.Id + ImSub + @"\MessageSignaledInterruptProperties")) limit = k?.GetValue("MessageNumberLimit") != null;
        return MsiAllowed(d.DevInst != 0 ? InterruptSupport(d.DevInst) : null, usesMsi, limit);
    }

    /// <summary>A battery (laptop, tablet, UPS reported as a battery) or Modern Standby: power saving is left alone.</summary>
    public static bool BatteryOrModernStandby()
    {
        try
        {
            byte flag = GetSystemPowerStatus(out var st) ? st.BatteryFlag : (byte)255;
            bool batteries = false, aoac = false;
            IntPtr caps = Marshal.AllocHGlobal(256);
            try
            {
                for (int i = 0; i < 256; i++) Marshal.WriteByte(caps, i, 0);
                if (GetPwrCapabilities(caps))
                {
                    aoac = Marshal.ReadByte(caps, 20) != 0;        // SYSTEM_POWER_CAPABILITIES.AoAc
                    batteries = Marshal.ReadByte(caps, 30) != 0;   // SystemBatteriesPresent
                }
            }
            finally { Marshal.FreeHGlobal(caps); }
            return !PowerSavingAllowed(flag, batteries, aoac);
        }
        catch (Exception) { return true; }
    }

    static string Prop(IntPtr set, ref SP_DEVINFO_DATA d, uint p)
    {
        var buf = new byte[2048];
        if (!SetupDiGetDeviceRegistryProperty(set, ref d, p, out _, buf, (uint)buf.Length, out uint need)) return "";
        return Encoding.Unicode.GetString(buf, 0, (int)Math.Min(need, (uint)buf.Length)).TrimEnd('\0').Split('\0')[0];
    }

    static string Id(uint inst)
    {
        var sb = new StringBuilder(512);
        return CM_Get_Device_ID(inst, sb, sb.Capacity, 0) == 0 ? sb.ToString().ToUpperInvariant() : "";
    }

    /// <summary>The PCI device a device hangs on (walking up the device tree), or null.</summary>
    static (string Id, uint Inst)? PciParent(uint node)
    {
        for (int depth = 0; depth < 16; depth++)
        {
            if (CM_Get_Parent(out uint parent, node, 0) != 0) return null;
            string id = Id(parent);
            if (id.StartsWith(@"PCI\", StringComparison.Ordinal)) return (id, parent);
            node = parent;
        }
        return null;
    }

    sealed record Present(string Id, string Name, string Class, string Service, string Driver, uint Inst);

    static List<Present> AllPresent()
    {
        var list = new List<Present>();
        IntPtr set = SetupDiGetClassDevs(IntPtr.Zero, null, IntPtr.Zero, DIGCF_PRESENT | DIGCF_ALLCLASSES);
        if (set == IntPtr.Zero || set == new IntPtr(-1)) return list;
        try
        {
            var d = new SP_DEVINFO_DATA { cbSize = (uint)Marshal.SizeOf<SP_DEVINFO_DATA>() };
            for (uint i = 0; SetupDiEnumDeviceInfo(set, i, ref d); i++)
            {
                if (CM_Get_DevNode_Status(out _, out uint problem, d.DevInst, 0) == 0 && problem == 0)
                {
                    string name = Prop(set, ref d, SPDRP_FRIENDLYNAME);
                    if (name.Length == 0) name = Prop(set, ref d, SPDRP_DEVICEDESC);
                    list.Add(new Present(Id(d.DevInst), name, Prop(set, ref d, SPDRP_CLASS), Prop(set, ref d, SPDRP_SERVICE), Prop(set, ref d, SPDRP_DRIVER), d.DevInst));
                }
                d.cbSize = (uint)Marshal.SizeOf<SP_DEVINFO_DATA>();
            }
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
        return list;
    }

    /// <summary>Every HID collection with an input role: (its PCI controller, role).</summary>
    static List<(string Controller, IrqRole Role)> HidCollections()
    {
        var res = new List<(string, IrqRole)>();
        var g = HidGuid;
        IntPtr set = SetupDiGetClassDevsGuid(ref g, null, IntPtr.Zero, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if (set == IntPtr.Zero || set == new IntPtr(-1)) return res;
        try
        {
            var ifd = new SP_DEVICE_INTERFACE_DATA { cbSize = (uint)Marshal.SizeOf<SP_DEVICE_INTERFACE_DATA>() };
            for (uint i = 0; SetupDiEnumDeviceInterfaces(set, IntPtr.Zero, ref g, i, ref ifd); i++)
            {
                var info = new SP_DEVINFO_DATA { cbSize = (uint)Marshal.SizeOf<SP_DEVINFO_DATA>() };
                SetupDiGetDeviceInterfaceDetail(set, ref ifd, IntPtr.Zero, 0, out uint need, ref info);
                IntPtr detail = Marshal.AllocHGlobal((int)Math.Max(need, 16));
                try
                {
                    Marshal.WriteInt32(detail, 8);                     // cbSize of SP_DEVICE_INTERFACE_DETAIL_DATA_W on x64
                    if (!SetupDiGetDeviceInterfaceDetail(set, ref ifd, detail, need, out _, ref info)) continue;
                    string path = Marshal.PtrToStringUni(detail + 4) ?? "";
                    string cls = Prop(set, ref info, SPDRP_CLASS), name = Prop(set, ref info, SPDRP_FRIENDLYNAME);
                    if (name.Length == 0) name = Prop(set, ref info, SPDRP_DEVICEDESC);
                    int page = 0, usage = 0;
                    string product = "";
                    IntPtr h = CreateFile(path, 0, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
                    if (h != new IntPtr(-1))
                    {
                        try
                        {
                            if (HidD_GetPreparsedData(h, out IntPtr pp))
                            {
                                IntPtr caps = Marshal.AllocHGlobal(64);
                                if (HidP_GetCaps(pp, caps) == 0x00110000) { usage = (ushort)Marshal.ReadInt16(caps, 0); page = (ushort)Marshal.ReadInt16(caps, 2); }
                                Marshal.FreeHGlobal(caps);
                                HidD_FreePreparsedData(pp);
                            }
                            var pb = new byte[512];
                            if (HidD_GetProductString(h, pb, pb.Length)) product = Encoding.Unicode.GetString(pb).TrimEnd('\0');
                        }
                        finally { CloseHandle(h); }
                    }
                    var role = HidRole(page, usage, cls, name, product);
                    if (role == null) continue;
                    var ctrl = PciParent(info.DevInst);
                    if (ctrl != null) res.Add((ctrl.Value.Id, role.Value));
                }
                finally
                {
                    Marshal.FreeHGlobal(detail);
                    ifd.cbSize = (uint)Marshal.SizeOf<SP_DEVICE_INTERFACE_DATA>();
                }
            }
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
        return res;
    }

    static RegistryKey Lm(string path) => Registry.LocalMachine.OpenSubKey(path);

    static bool NonDefaultAffinity(string id)
    {
        using var k = Lm(EnumRoot + id + ImSub + @"\Affinity Policy");
        if (k == null) return false;
        if (k.GetValue("DevicePolicy") is int p && p != 0) return true;
        if (k.GetValue("DevicePriority") is int pr && pr != 2) return true;
        return k.GetValue("AssignmentSetOverride") is byte[] b && b.Any(x => x != 0);
    }

    static bool IsNetCx(Present p)
    {
        if (!string.IsNullOrEmpty(p.Service))
        {
            using var s = Lm(@"SYSTEM\CurrentControlSet\Services\" + p.Service);
            if (s != null)
            {
                if (Regex.IsMatch(s.GetValue("DisplayName") as string ?? "", "netadapter", RegexOptions.IgnoreCase)) return true;
                if (Regex.IsMatch(s.GetValue("ImagePath") as string ?? "", @"netadaptercx|netadapter\.sys", RegexOptions.IgnoreCase)) return true;
                if ((s.GetValue("DependOnService") as string[] ?? Array.Empty<string>()).Any(x => x.Contains("netadapter", StringComparison.OrdinalIgnoreCase))) return true;
            }
        }
        using var e = Lm(EnumRoot + p.Id);
        foreach (var f in new[] { "UpperFilters", "LowerFilters" })
            if ((e?.GetValue(f) as string[] ?? Array.Empty<string>()).Any(x => x.Contains("netadapter", StringComparison.OrdinalIgnoreCase))) return true;
        return false;
    }

    /// <summary>The CPU is AMD family 17h (Zen, Zen+, Zen 2): two L3 groups form one CCD.</summary>
    static bool AmdFamily17()
    {
        if (!X86Base.IsSupported) return false;
        var (_, b, c, d) = X86Base.CpuId(0, 0);
        bool amd = b == 0x68747541 && d == 0x69746E65 && c == 0x444D4163;
        int eax = X86Base.CpuId(1, 0).Eax, bas = (eax >> 8) & 0xF, ext = (eax >> 20) & 0xFF;
        return amd && (bas == 0xF ? bas + ext : bas) == 0x17;
    }

    /// <summary>The device list in DEVICE-TWEAKER's order (USB, GPU, audio, network, storage), with roles and kinds.</summary>
    public static List<AutoDevice> Discover()
    {
        var all = AllPresent();
        var list = new List<AutoDevice>();
        foreach (var p in all)
        {
            if (Excluded(p.Id, p.Name, p.Class)) continue;
            if (IsStorage(p.Class, p.Name, p.Service)) continue;          // never: the boot disk hangs on one of these
            var kind = Classify(p.Class, p.Name);
            if (kind == null || p.Class.Equals("AudioEndpoint", StringComparison.OrdinalIgnoreCase)) continue;
            if (kind == AutoKind.Usb && Regex.IsMatch(p.Id, @"VEN_10DE[&\\]", RegexOptions.IgnoreCase)) continue;     // NVIDIA USB-C
            using (var im = Lm(EnumRoot + p.Id + ImSub)) if (im == null) continue;
            list.Add(new AutoDevice { Id = p.Id, Name = p.Name, Class = p.Class, Service = p.Service, DriverKey = p.Driver, DevInst = p.Inst, Kind = kind.Value });
        }
        // USB roles from the HID collections: the most important one wins (mouse, game controller, keyboard).
        foreach (var (ctrl, role) in HidCollections())
        {
            var dev = list.FirstOrDefault(d => d.Kind == AutoKind.Usb && d.Id == ctrl);
            if (dev != null && (dev.Role == null || role < dev.Role)) dev.Role = role;
        }
        // Every USB host controller is listed (a PC usually has two or more, e.g. USB 3.10 and USB 3.0): the ones with a
        // mouse, keyboard or game controller get a slot, the others stay with Windows' own placement.
        MarkIntegrated(list.Where(d => d.Kind == AutoKind.Gpu).ToList());
        foreach (var n in list.Where(d => d.Kind == AutoKind.Net))
        {
            n.Wifi = IsWifi(n.Id, n.Name, n.Service);
            n.NetCx = IsNetCx(all.First(x => x.Id == n.Id));
        }
        // Audio: endpoints mapped to their controller; no endpoints or only S/PDIF → not listed.
        foreach (var ep in all.Where(x => x.Class.Equals("AudioEndpoint", StringComparison.OrdinalIgnoreCase)))
        {
            var ctrl = PciParent(ep.Inst);
            var dev = ctrl == null ? null : list.FirstOrDefault(d => d.Kind == AutoKind.Audio && d.Id == ctrl.Value.Id);
            dev?.Endpoints.Add(ep.Name);
        }
        foreach (var a in list.Where(d => d.Kind == AutoKind.Audio))
            a.DisplayAudio = IsDisplayAudioDevice(a.Id, a.Name) || (a.Endpoints.Count > 0 && a.Endpoints.All(IsDisplayEndpoint));
        list.RemoveAll(d => d.Kind == AutoKind.Audio && (d.Endpoints.Count == 0 || d.Endpoints.All(IsSpdif)));
        list.RemoveAll(d => d.Kind == AutoKind.Stor);
        return list.Select((d, i) => (d, i)).OrderBy(x => x.d.Kind).ThenBy(x => x.i).Select(x => x.d).ToList();
    }

    // ---- planning -----------------------------------------------------------------------------------------

    public static Func<Dictionary<int, int>> CppcHook;   // tests

    /// <summary>MaximumPerformancePercent per processor from the newest Kernel-Processor-Power event 55 of each.</summary>
    public static Dictionary<int, int> Cppc()
    {
        if (CppcHook != null) return CppcHook();
        try
        {
            int n = Math.Max(4 * Environment.ProcessorCount, 16);
            var psi = SysExe.Start("wevtutil.exe", redirect: false);
            psi.RedirectStandardOutput = true;
            psi.Arguments = $"qe System /q:\"*[System[Provider[@Name='Microsoft-Windows-Kernel-Processor-Power'] and (EventID=55)]]\" /c:{n} /rd:true /f:xml";
            using var p = System.Diagnostics.Process.Start(psi)!;
            string xml = p.StandardOutput.ReadToEnd();
            p.WaitForExit(10000);
            return ParseCppc(xml);
        }
        catch (Exception) { return new(); }
    }

    /// <summary>Newest first: the first value seen for a group-0 processor wins.</summary>
    public static Dictionary<int, int> ParseCppc(string xml)
    {
        var res = new Dictionary<int, int>();
        foreach (Match ev in Regex.Matches(xml ?? "", "<Event[ >].*?</Event>", RegexOptions.Singleline))
        {
            var grp = Regex.Match(ev.Value, "<Data Name=['\"]Group['\"]>(\\d+)</Data>");
            var num = Regex.Match(ev.Value, "<Data Name=['\"]Number['\"]>(\\d+)</Data>");
            var perf = Regex.Match(ev.Value, "<Data Name=['\"]MaximumPerformancePercent['\"]>(\\d+)</Data>");
            if (!num.Success || !perf.Success || (grp.Success && grp.Groups[1].Value != "0")) continue;
            int lp = int.Parse(num.Groups[1].Value, CultureInfo.InvariantCulture);
            if (!res.ContainsKey(lp)) res[lp] = int.Parse(perf.Groups[1].Value, CultureInfo.InvariantCulture);
        }
        return res;
    }

    /// <summary>Gives each device its slot (USB with a role, discrete GPU, wired NIC) and plans the cores.</summary>
    public static void Assign(List<AutoDevice> devs, Topo topo, IReadOnlyDictionary<int, int> perf, bool ccxPairs)
    {
        foreach (var d in devs)
        {
            IrqRole? r = d.Kind switch
            {
                AutoKind.Usb => d.Role,
                AutoKind.Gpu when !d.Igpu => IrqRole.Gpu,
                AutoKind.Net when !d.Wifi => IrqRole.Net,
                _ => null,
            };
            d.Slot = r == null ? null : new IrqSlot { Key = d.Id, Name = d.Name, Role = r.Value };
        }
        IrqLayout.Plan(topo, devs.Where(d => d.Slot != null).Select(d => d.Slot), perf, ccxPairs);
    }

    /// <summary>Discovers the devices and plans their cores (nothing is written).</summary>
    public static List<AutoDevice> PlanNow()
    {
        var devs = Discover();
        Assign(devs, TopoBuilder.Live(), Cppc(), AmdFamily17());
        foreach (var n in devs.Where(d => d.Kind == AutoKind.Net && !d.Wifi && !d.NetCx)) n.NicMode = NicMode(n);
        return devs;
    }

    /// <summary>NDIS network card: RSS, IRQ or BOTH, as DEVICE-TWEAKER decides it (one queue planned).</summary>
    static string NicMode(AutoDevice n)
    {
        bool cap = false, configured = false, enabled = false;
        int queues = 0, maxProc = 0;
        using (var k = Lm(ClassRoot + n.DriverKey))
        {
            if (k != null)
            {
                configured = k.GetValue("*RssBaseProcNumber") != null || k.GetValue("*NumRssQueues") != null;
                using var ndi = k.OpenSubKey(@"Ndi\Params\*RssBaseProcNumber");
                cap = configured || ndi != null;
            }
        }
        using (var e = Lm(EnumRoot + n.Id)) if (e?.GetValue("*RssBaseProcNumber") != null) cap = true;
        try
        {
            string script = "Get-NetAdapter -IncludeHidden -EA 0 | ForEach-Object { $r = Get-NetAdapterRss -Name $_.Name -EA 0; if ($r) { $_.PnPDeviceID + '|' + [int]$r.Enabled + '|' + $r.NumberOfReceiveQueues + '|' + $r.MaxProcessors } }";
            // Windows' own NetAdapter module only (SysExe.PowerShell: no profile, system module path).
            var psi = SysExe.PowerShell(script);
            using var p = System.Diagnostics.Process.Start(psi)!;
            var t = p.StandardOutput.ReadToEndAsync();
            _ = p.StandardError.ReadToEndAsync();
            if (p.WaitForExit(10000))
                foreach (var line in t.Result.Split('\n'))
                {
                    var f = line.Trim().Split('|');
                    if (f.Length == 4 && f[0].Equals(n.Id, StringComparison.OrdinalIgnoreCase))
                    {
                        cap = true;
                        enabled = f[1] == "1";
                        int.TryParse(f[2], out queues);
                        int.TryParse(f[3], out maxProc);
                    }
                }
        }
        catch (Exception) { }
        return ChooseNicMode(cap, enabled, configured, queues, maxProc);
    }

    public static string ChooseNicMode(bool capable, bool enabled, bool configured, int queues, int maxProc)
    {
        if (capable && (enabled || configured)) return "BOTH";
        if (capable && (queues > 1 || maxProc > 1)) return "BOTH";
        return capable ? "RSS" : "IRQ";
    }

    // ---- writing, with the backup ------------------------------------------------------------------------

    sealed class Backup
    {
        public Dictionary<string, string> Reg { get; set; } = new();     // "HIVE|path|name" → "" (absent) / d:int / b:hex / s:text; "|*" = a deleted key
        public Dictionary<string, bool> Wmi { get; set; } = new();       // MSPower_DeviceEnable InstanceName → Enable
        public uint[] UsbSuspend { get; set; }                           // AC, DC of the active plan, before
    }

    internal const string BackupName = "irq-auto-backup.json";
    static string BackupPath => Path.Combine(Store.Dir, BackupName);
    static Backup _bk = new();

    static Backup Load()
    {
        try
        {
            if (!File.Exists(BackupPath)) return new();
            var dropped = new List<string>();
            var b = Clean(JsonSerializer.Deserialize<Backup>(File.ReadAllText(BackupPath)), dropped);
            if (dropped.Count > 0) AppLog.Add(L.F("Device backup: entries that are not values the program writes were ignored: {0}", string.Join(", ", dropped.Take(5))), AppLog.Sev.Warn);
            return b;
        }
        catch (Exception) { return new(); }
    }

    // ---- what a backup may hold: exactly what this program (or an older version of it) writes ---------------

    /// <summary>A device instance ID: ENUMERATOR\DEVICE\INSTANCE, nothing that could reach outside the device's key.</summary>
    internal static readonly Regex InstanceRx = new(@"^[A-Za-z0-9_]{1,32}\\[A-Za-z0-9&_.#{}+\- ]{1,200}\\[A-Za-z0-9&_.#{}+\- ]{1,200}$", RegexOptions.CultureInvariant);
    static readonly Regex DriverKeyRx = new(@"^\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}\\\d{4}$", RegexOptions.CultureInvariant);
    static readonly Regex ValueNameRx = new(@"^[A-Za-z0-9 _\-]{1,64}$", RegexOptions.CultureInvariant);
    static readonly Regex WmiInstanceRx = new(@"^[A-Za-z0-9&_.#{}+\\\- ]{1,400}$", RegexOptions.CultureInvariant);
    const string KernelKey = @"SYSTEM\CurrentControlSet\Control\Session Manager\Kernel";

    static bool DWord(string enc, long lo, long hi) =>
        enc.StartsWith("d:", StringComparison.Ordinal) && long.TryParse(enc[2..], NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out long v) && v >= lo && v <= hi;

    static bool Bin(string enc, int maxBytes) =>
        enc.StartsWith("b:", StringComparison.Ordinal) && enc.Length - 2 <= maxBytes * 2 && (enc.Length - 2) % 2 == 0 && enc.Skip(2).All(Uri.IsHexDigit);

    static bool Digits(string enc, int maxLen) =>
        enc.StartsWith("s:", StringComparison.Ordinal) && enc.Length > 2 && enc.Length - 2 <= maxLen && enc.Skip(2).All(char.IsAsciiDigit);

    const long I32Min = int.MinValue, I32Max = int.MaxValue;

    /// <summary>
    /// The interrupt values of a device's Affinity Policy / MSI key: the names this program writes and puts back, each
    /// in its registry type — a DWORD, or for the affinity mask a binary value of at most 64 bytes (pure; also used for
    /// the manual device backup). The number itself is not limited: a backup holds whatever value the device had
    /// before, and it has to come back as it was. "" = the value did not exist.
    /// </summary>
    internal static bool ValidIrqValue(string name, string enc)
    {
        enc ??= "";
        if (enc.Length == 0) return name is "DevicePriority" or "DevicePolicy" or "AssignmentSetOverride" or "MSISupported" or "MessageNumberLimit";
        return name switch
        {
            "DevicePriority" or "DevicePolicy" or "MSISupported" or "MessageNumberLimit" => DWord(enc, I32Min, I32Max),
            "AssignmentSetOverride" => Bin(enc, 64),
            _ => false,
        };
    }

    /// <summary>
    /// Whether one backed-up entry ("HIVE|path|name" → encoded value) is something the automatic layout writes (pure):
    /// the key (by its path: a device's interrupt keys, its Device Parameters, its driver's class key, the Kernel key,
    /// the mouse key), the value name and the registry type. Everything else — another key, another value name,
    /// another hive, another type, a text or binary value longer than the program ever wrote — is refused: the backup
    /// is never trusted to say what to write with administrator rights.
    /// </summary>
    public static bool AllowedEntry(string id, string enc)
    {
        enc ??= "";
        var f = (id ?? "").Split('|');
        if (f.Length != 3) return false;
        string hive = f[0], path = f[1], name = f[2];
        if (hive == "HKCU") return path == @"Control Panel\Mouse" && name == "RawMouseThrottleDuration" && (enc == "" || DWord(enc, I32Min, I32Max) || Digits(enc, 10));
        if (hive != "HKLM") return false;
        if (path.Equals(KernelKey, StringComparison.OrdinalIgnoreCase))
            return name == "ReservedCpuSets" && (enc == "" || Bin(enc, 64));
        if (path.StartsWith(ClassRoot, StringComparison.OrdinalIgnoreCase))
        {
            if (!DriverKeyRx.IsMatch(path[ClassRoot.Length..])) return false;
            if (name == "PnPCapabilities") return enc == "" || DWord(enc, I32Min, I32Max);
            if (RssNames.Contains(name) || name == "*NumaNodeId") return enc == "" || DWord(enc, I32Min, I32Max) || Digits(enc, 10);
            return false;
        }
        if (!path.StartsWith(EnumRoot, StringComparison.OrdinalIgnoreCase)) return false;
        string rest = path[EnumRoot.Length..];
        foreach (var (tail, check) in DeviceKeys)
        {
            if (!rest.EndsWith(tail, StringComparison.OrdinalIgnoreCase)) continue;
            string inst = rest[..^tail.Length];
            return InstanceRx.IsMatch(inst) && check(name, enc);
        }
        return false;
    }

    static readonly (string Tail, Func<string, string, bool> Check)[] DeviceKeys =
    {
        (ImSub + @"\Affinity Policy", (n, e) => n is "DevicePriority" or "DevicePolicy" or "AssignmentSetOverride" && ValidIrqValue(n, e)),
        (ImSub + @"\MessageSignaledInterruptProperties", (n, e) => n is "MSISupported" or "MessageNumberLimit" && ValidIrqValue(n, e)),
        // The legacy Priority subkey is deleted whole: "*" marks the key, its values are kept as they were (bounded).
        (ImSub + @"\Priority", (n, e) => n == "*" ? e == "s:key" : ValueNameRx.IsMatch(n) && (e == "" || DWord(e, int.MinValue, int.MaxValue) || Bin(e, 64) || (e.StartsWith("s:", StringComparison.Ordinal) && e.Length <= 66 && !e.Any(char.IsControl)))),
        (@"\Device Parameters", (n, e) => n is "SelectiveSuspendEnabled" or "EnhancedPowerManagementEnabled" && (e == "" || DWord(e, I32Min, I32Max) || Bin(e, 4))),
    };

    /// <summary>
    /// The backup without anything <see cref="AllowedEntry"/> refuses (WMI switches: only device instance names; the
    /// USB suspend pair: only 0/1). The refused entries are named in dropped (null: not collected).
    /// </summary>
    static Backup Clean(Backup b, List<string> dropped)
    {
        var c = new Backup();
        if (b == null) return c;
        foreach (var (id, enc) in b.Reg ?? new())
            if (AllowedEntry(id, enc)) c.Reg[id] = enc;
            else dropped?.Add(id);
        foreach (var (inst, on) in b.Wmi ?? new())
            if (WmiInstanceRx.IsMatch(inst ?? "")) c.Wmi[inst] = on;
            else dropped?.Add("WMI " + inst);
        if (b.UsbSuspend is { Length: 2 } u && u[0] <= 1 && u[1] <= 1) c.UsbSuspend = u;
        else if (b.UsbSuspend != null) dropped?.Add("UsbSuspend");
        return c;
    }

    /// <summary>
    /// Takes over a backup file of an older version (from the user's folder): only the allowed entries, as JSON for
    /// the state folder; null when nothing is left. The refused ones are named in dropped.
    /// </summary>
    internal static string ImportBackup(string json, string existing, List<string> dropped)
    {
        var b = Clean(JsonSerializer.Deserialize<Backup>(json), dropped);
        // A backup already in the state folder wins: it holds the values from before the first change.
        var cur = existing == null ? new Backup() : Clean(JsonSerializer.Deserialize<Backup>(existing), null);
        foreach (var (k, v) in b.Reg) cur.Reg.TryAdd(k, v);
        foreach (var (k, v) in b.Wmi) cur.Wmi.TryAdd(k, v);
        cur.UsbSuspend ??= b.UsbSuspend;
        return cur.Reg.Count == 0 && cur.Wmi.Count == 0 && cur.UsbSuspend == null ? null : JsonSerializer.Serialize(cur);
    }

    static void Save()
    {
        string path = BackupPath;
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(_bk));
        File.Move(path + ".tmp", path, true);
    }

    public static bool HasBackup
    {
        get
        {
            try { return File.Exists(BackupPath); }
            catch (Exception) { return false; }
        }
    }

    static RegistryKey Root(string hive) => hive switch { "HKLM" => Registry.LocalMachine, "HKCU" => Registry.CurrentUser, _ => throw new InvalidDataException("hive " + hive) };

    static string Enc(object v) => v switch
    {
        null => "",
        int i => "d:" + i.ToString(CultureInfo.InvariantCulture),
        byte[] b => "b:" + Convert.ToHexString(b),
        string s => "s:" + s,
        _ => "",
    };

    static void Remember(string hive, string path, string name)
    {
        string id = hive + "|" + path + "|" + name;
        if (_bk.Reg.ContainsKey(id)) return;
        using var k = Root(hive).OpenSubKey(path);
        _bk.Reg[id] = Enc(k?.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames));
    }

    static void SetD(string hive, string path, string name, int v)
    {
        Remember(hive, path, name);
        using var k = Root(hive).CreateSubKey(path, true);
        k.SetValue(name, v, RegistryValueKind.DWord);
    }

    static void SetB(string hive, string path, string name, byte[] v)
    {
        Remember(hive, path, name);
        using var k = Root(hive).CreateSubKey(path, true);
        k.SetValue(name, v, RegistryValueKind.Binary);
    }

    static void Del(string hive, string path, string name)
    {
        using var k = Root(hive).OpenSubKey(path, true);
        if (k == null || !k.GetValueNames().Contains(name, StringComparer.OrdinalIgnoreCase)) return;
        Remember(hive, path, name);
        k.DeleteValue(name, false);
    }

    /// <summary>Deletes a subkey; its values are remembered so the restore can recreate it.</summary>
    static void DelKey(string path)
    {
        using (var k = Registry.LocalMachine.OpenSubKey(path))
        {
            if (k == null) return;
            _bk.Reg.TryAdd("HKLM|" + path + "|*", "s:key");
            foreach (var n in k.GetValueNames()) Remember("HKLM", path, n);
        }
        Registry.LocalMachine.DeleteSubKeyTree(path, false);
    }

    /// <summary>
    /// The interrupt values of one device. priority null = deleted; policy null = untouched; 0 = written 0 and the mask
    /// deleted. MSI is switched on only when the device offers it; the driver's MessageNumberLimit is never touched.
    /// </summary>
    static void Irq(AutoDevice d, int? priority, int? policy, ulong mask, List<(string, bool)> log, bool deletePolicy = false)
    {
        if (IsStorage(d.Class, d.Name, d.Service)) return;
        string im = EnumRoot + d.Id + ImSub, ap = im + @"\Affinity Policy";
        Msi(d, log);
        if (priority.HasValue) SetD("HKLM", ap, "DevicePriority", priority.Value); else Del("HKLM", ap, "DevicePriority");
        DelKey(im + @"\Priority");
        if (deletePolicy) { Del("HKLM", ap, "DevicePolicy"); Del("HKLM", ap, "AssignmentSetOverride"); }
        else if (policy == 4 && mask != 0) { SetD("HKLM", ap, "DevicePolicy", 4); SetB("HKLM", ap, "AssignmentSetOverride", BitConverter.GetBytes(mask)); }
        else if (policy.HasValue) { SetD("HKLM", ap, "DevicePolicy", 0); Del("HKLM", ap, "AssignmentSetOverride"); }
    }

    static void Msi(AutoDevice d, List<(string, bool)> log)
    {
        if (MsiCapable(d)) SetD("HKLM", EnumRoot + d.Id + ImSub + @"\MessageSignaledInterruptProperties", "MSISupported", 1);
        else log.Add((L.F("{0}: MSI left as it is (the device does not offer it)", d.Name), true));
    }

    static readonly string[] RssNames = { "*NumRssQueues", "*RssBaseProcNumber", "*RssBaseProcGroup", "*MaxRssProcessors", "*RSSMaxProcGroup", "*RssMaxProcNumber" };

    static void Rss(AutoDevice d, int lp)
    {
        string c = ClassRoot + d.DriverKey;
        int[] values = { 1, lp, 0, 1, 0, lp };
        for (int i = 0; i < RssNames.Length; i++) SetD("HKLM", c, RssNames[i], values[i]);
        Del("HKLM", c, "*NumaNodeId");
    }

    static void RssClear(AutoDevice d)
    {
        string c = ClassRoot + d.DriverKey;
        foreach (var n in RssNames.Append("*NumaNodeId")) Del("HKLM", c, n);
    }

    /// <summary>Power saving off for one device: class-key PnPCapabilities bit 0x08 and WMI MSPower_DeviceEnable.</summary>
    static void PowerOff(string id, string driverKey, List<(string, bool)> log)
    {
        if (!string.IsNullOrEmpty(driverKey))
        {
            string c = ClassRoot + driverKey;
            int cur;
            using (var k = Registry.LocalMachine.OpenSubKey(c)) cur = k?.GetValue("PnPCapabilities") is int v ? v : 0;
            SetD("HKLM", c, "PnPCapabilities", cur | 0x08);
        }
        try
        {
            using var q = new ManagementObjectSearcher(@"root\WMI", "SELECT * FROM MSPower_DeviceEnable");
            foreach (ManagementObject o in q.Get())
                using (o)
                {
                    string key = o["InstanceName"] as string ?? "";
                    if (!key.Replace('#', '\\').StartsWith(id, StringComparison.OrdinalIgnoreCase)) continue;
                    _bk.Wmi.TryAdd(key, o["Enable"] is bool b && b);
                    o["Enable"] = false;
                    o.Put();
                }
        }
        catch (Exception e) { log.Add((L.F("{0}: the power-saving switch was not changed ({1})", id, e.Message), false)); }
    }

    static List<(string Id, string Driver)> RootHubs(uint controller)
    {
        var res = new List<(string, string)>();
        if (CM_Get_Child(out uint child, controller, 0) != 0) return res;
        for (int guard = 0; guard < 64; guard++)
        {
            string id = Id(child);
            if (id.Contains("ROOT_HUB", StringComparison.OrdinalIgnoreCase))
            {
                using var k = Lm(EnumRoot + id);
                res.Add((id, k?.GetValue("Driver") as string ?? ""));
            }
            if (CM_Get_Sibling(out uint next, child, 0) != 0) break;
            child = next;
        }
        return res;
    }

    // Power plan: USB selective suspend of the active plan.
    static readonly Guid UsbSub = new("2a737441-1930-4402-8d77-b2bebba308a3"), UsbSuspend = new("48e6b7a6-50f5-4782-a5d4-53bb8f07e226");
    [DllImport("powrprof.dll")] static extern uint PowerGetActiveScheme(IntPtr root, out IntPtr scheme);
    [DllImport("powrprof.dll")] static extern uint PowerReadACValueIndex(IntPtr root, ref Guid scheme, ref Guid sub, ref Guid setting, out uint value);
    [DllImport("powrprof.dll")] static extern uint PowerReadDCValueIndex(IntPtr root, ref Guid scheme, ref Guid sub, ref Guid setting, out uint value);
    [DllImport("powrprof.dll")] static extern uint PowerWriteACValueIndex(IntPtr root, ref Guid scheme, ref Guid sub, ref Guid setting, uint value);
    [DllImport("powrprof.dll")] static extern uint PowerWriteDCValueIndex(IntPtr root, ref Guid scheme, ref Guid sub, ref Guid setting, uint value);
    [DllImport("powrprof.dll")] static extern uint PowerSetActiveScheme(IntPtr root, ref Guid scheme);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);

    /// <summary>
    /// The USB selective suspend of the active plan, mains power (AC) only. The battery (DC) value is never changed;
    /// restoreDc only puts back a DC value that an older version (before 1.12.0) changed.
    /// </summary>
    static void UsbSuspendPlan(uint ac, uint? restoreDc, bool remember)
    {
        if (PowerGetActiveScheme(IntPtr.Zero, out IntPtr p) != 0) return;
        var scheme = Marshal.PtrToStructure<Guid>(p);
        LocalFree(p);
        Guid sub = UsbSub, set = UsbSuspend;
        if (remember && _bk.UsbSuspend == null)
        {
            PowerReadACValueIndex(IntPtr.Zero, ref scheme, ref sub, ref set, out uint a);
            PowerReadDCValueIndex(IntPtr.Zero, ref scheme, ref sub, ref set, out uint d);
            _bk.UsbSuspend = new[] { a, d };
        }
        PowerWriteACValueIndex(IntPtr.Zero, ref scheme, ref sub, ref set, ac);
        if (restoreDc.HasValue && PowerReadDCValueIndex(IntPtr.Zero, ref scheme, ref sub, ref set, out uint now) == 0 && now != restoreDc.Value)
            PowerWriteDCValueIndex(IntPtr.Zero, ref scheme, ref sub, ref set, restoreDc.Value);
        PowerSetActiveScheme(IntPtr.Zero, ref scheme);
    }

    /// <summary>
    /// Writes the plan in DEVICE-TWEAKER's order: ReservedCpuSets removed, every device (Wi-Fi and storage skipped),
    /// the power plan's USB selective suspend (mains power, desktops only). The boot guard's marker is saved first;
    /// without it nothing is written. Returns lines for the log.
    /// </summary>
    public static List<(string Text, bool Ok)> Apply(List<AutoDevice> devs)
    {
        var lines = new List<(string, bool)>();
        if (Flags.Refuse(Flags.DevicesWrite, L.T("Device policy write"))) { lines.Add((L.T("The feature flag devices-write is closed"), false)); return lines; }
        try { BootGuard.MarkPending(); }
        catch (Exception e) { lines.Add((L.F("Nothing was written: the safety marker could not be saved ({0})", e.Message), false)); return lines; }
        _bk = Load();
        var log = new List<(string, bool)>();
        bool power = !BatteryOrModernStandby();
        if (!power) lines.Add((L.T("Battery or Modern Standby found: USB and network power saving stay as Windows set them"), true));
        try
        {
            Del("HKLM", @"SYSTEM\CurrentControlSet\Control\Session Manager\Kernel", "ReservedCpuSets");
            foreach (var d in devs)
            {
                try
                {
                    string what;
                    var s = d.Slot;
                    bool placed = s != null && s.Mask != 0;
                    switch (d.Kind)
                    {
                        case AutoKind.Usb when placed: Irq(d, 3, 4, s.Mask, log); what = L.F("processor {0}", Masks.Ranges(s.Mask)); break;
                        case AutoKind.Usb when s != null: Irq(d, 3, 0, 0, log); what = L.T("high priority, no core of its own"); break;
                        case AutoKind.Usb: Irq(d, null, 0, 0, log); what = L.T("Windows decides"); break;
                        case AutoKind.Gpu when d.Igpu: Irq(d, null, 0, 0, log); what = L.T("integrated graphics: Windows decides"); break;
                        case AutoKind.Gpu when placed: Irq(d, 3, 4, s.Mask, log); what = L.F("processor {0}", Masks.Ranges(s.Mask)); break;
                        case AutoKind.Gpu: Irq(d, 3, 0, 0, log); what = L.T("high priority, no core of its own"); break;
                        case AutoKind.Audio when d.DisplayAudio: Msi(d, log); what = L.T("display audio: MSI only"); break;
                        case AutoKind.Audio: Irq(d, null, 0, 0, log); what = L.T("Windows decides"); break;
                        case AutoKind.Net when d.Wifi: lines.Add((L.F("{0}: Wi-Fi, not touched", d.Name), true)); continue;
                        case AutoKind.Net when d.NetCx && placed: Irq(d, 3, 4, s.Mask, log); what = L.F("processor {0}", Masks.Ranges(s.Mask)); break;
                        case AutoKind.Net when !placed: Irq(d, 3, 0, 0, log); what = L.T("high priority, no core of its own"); break;
                        case AutoKind.Net:
                        {
                            int lp = System.Numerics.BitOperations.TrailingZeroCount(s.Mask);
                            if (d.NicMode == "RSS") Irq(d, 3, null, 0, log, deletePolicy: true); else Irq(d, 3, 4, s.Mask, log);
                            if (d.NicMode == "IRQ") RssClear(d); else Rss(d, lp);
                            what = L.F("processor {0}, mode {1}", lp, d.NicMode);
                            break;
                        }
                        case AutoKind.Stor: lines.Add((L.F("{0}: storage, never touched", d.Name), true)); continue;
                        default: continue;
                    }
                    if (d.Kind == AutoKind.Usb && power)
                        foreach (var (id, drv) in new[] { (d.Id, d.DriverKey) }.Concat(RootHubs(d.DevInst)))
                        {
                            SetD("HKLM", EnumRoot + id + @"\Device Parameters", "SelectiveSuspendEnabled", 0);
                            SetD("HKLM", EnumRoot + id + @"\Device Parameters", "EnhancedPowerManagementEnabled", 0);
                            PowerOff(id, drv, log);
                        }
                    if (d.Kind == AutoKind.Net && power) PowerOff(d.Id, d.DriverKey, log);
                    lines.Add((L.F("{0}: {1}", d.Name, what), true));
                }
                catch (Exception e) { lines.Add((L.F("{0}: {1}", d.Name, e.Message), false)); }
            }
            if (power && devs.Any(d => d.Kind == AutoKind.Usb)) UsbSuspendPlan(0, null, true);
        }
        catch (UnauthorizedAccessException) { lines.Add((L.T("Access denied: run as administrator"), false)); }
        finally { Save(); }
        lines.AddRange(log);
        return lines;
    }

    static void RestoreEntry(string id, string enc)
    {
        if (!AllowedEntry(id, enc))
        {
            AppLog.Add(L.F("Device backup: {0} is not a value the program writes; it was not restored", id), AppLog.Sev.Warn);
            return;
        }
        var f = id.Split('|');
        if (enc == "")
        {
            using var k = Root(f[0]).OpenSubKey(f[1], true);
            k?.DeleteValue(f[2], false);
            return;
        }
        using var w = Root(f[0]).CreateSubKey(f[1], true);
        if (enc.StartsWith("d:")) w.SetValue(f[2], int.Parse(enc[2..], CultureInfo.InvariantCulture), RegistryValueKind.DWord);
        else if (enc.StartsWith("b:")) w.SetValue(f[2], Convert.FromHexString(enc[2..]), RegistryValueKind.Binary);
        else if (enc.StartsWith("s:")) w.SetValue(f[2], enc[2..], RegistryValueKind.String);
    }

    /// <summary>Puts back everything the automatic layout changed (part of "Restore all devices").</summary>
    public static void RevertExtras()
    {
        if (!HasBackup) return;
        _bk = Load();      // only allowed entries (Clean)
        foreach (var id in _bk.Reg.Keys.Where(k => k.EndsWith("|*") && k.StartsWith("HKLM|", StringComparison.Ordinal)))
            Registry.LocalMachine.CreateSubKey(id.Split('|')[1], true)?.Dispose();
        foreach (var (id, enc) in _bk.Reg.Where(kv => !kv.Key.EndsWith("|*")))
        {
            try { RestoreEntry(id, enc); }
            catch (Exception) { }
        }
        try
        {
            if (_bk.Wmi.Count > 0)
            {
                using var q = new ManagementObjectSearcher(@"root\WMI", "SELECT * FROM MSPower_DeviceEnable");
                foreach (ManagementObject o in q.Get())
                    using (o)
                        if (_bk.Wmi.TryGetValue(o["InstanceName"] as string ?? "", out bool en)) { o["Enable"] = en; o.Put(); }
            }
        }
        catch (Exception) { }
        if (_bk.UsbSuspend is { Length: 2 }) try { UsbSuspendPlan(_bk.UsbSuspend[0], _bk.UsbSuspend[1], false); } catch (Exception) { }
        File.Delete(BackupPath);
        _bk = new();
    }

    /// <summary>
    /// First start of 1.12.0: an automatic layout written by an older version (it could touch storage controllers,
    /// force MSI, change laptop power saving and RawMouseThrottleDuration) is reverted from its backup, then the safe
    /// layout of this version is planned and written again, with the boot guard. Takes effect after a restart.
    /// Returns lines for the log (text, ok).
    /// </summary>
    public static List<(string Text, bool Ok)> RedoOldLayout()
    {
        var log = new List<(string, bool)>();
        if (!HasBackup) return log;
        RevertExtras();
        log.Add((L.T("The automatic device layout of an older version was reverted: it could touch storage, force MSI and change power saving on laptops"), true));
        if (!Flags.Open(Flags.DevicesWrite))
        {
            log.Add((L.T("Restart Windows: the device values are back as they were before the layout"), true));
            return log;
        }
        log.Add((L.T("The new safe device layout is written again:"), true));
        log.AddRange(Apply(PlanNow()));
        log.Add((L.T("Restart Windows: the new safe device layout starts working after the restart"), true));
        return log;
    }

    // ---- values of older versions that 1.12.0 no longer writes -------------------------------------------

    const string RawMouseId = @"HKCU|Control Panel\Mouse|RawMouseThrottleDuration";

    /// <summary>The device instance a backed-up value belongs to ("…\Enum\&lt;id&gt;\Device Parameters…"), or null.</summary>
    public static string DeviceOfEntry(string id)
    {
        var f = (id ?? "").Split('|');
        if (f.Length < 2 || !f[1].StartsWith(EnumRoot, StringComparison.OrdinalIgnoreCase)) return null;
        string rest = f[1][EnumRoot.Length..];
        int i = rest.IndexOf(@"\Device Parameters", StringComparison.OrdinalIgnoreCase);
        return i > 0 ? rest[..i] : null;
    }

    static bool StorageInstance(string id)
    {
        using var k = Lm(EnumRoot + id);
        if (k == null) return false;
        string desc = (k.GetValue("FriendlyName") as string) ?? (k.GetValue("DeviceDesc") as string) ?? "";
        int semi = desc.LastIndexOf(';');
        if (semi >= 0) desc = desc[(semi + 1)..];
        return IsStorage(k.GetValue("Class") as string, desc, k.GetValue("Service") as string);
    }

    /// <summary>
    /// At every start: what older versions wrote and 1.12.0 no longer does is put back from the backup —
    /// RawMouseThrottleDuration (it only affects windows in the background) and every value of a storage controller.
    /// guessRawMouse: with no backup at all, a RawMouseThrottleDuration of exactly 20 (the value older versions wrote)
    /// is removed (the caller does that only once). Returns lines for the log.
    /// </summary>
    public static List<string> CleanupLegacy(bool guessRawMouse)
    {
        var log = new List<string>();
        if (HasBackup)
        {
            _bk = Load();
            bool changed = false;
            if (_bk.Reg.TryGetValue(RawMouseId, out var enc))
            {
                try
                {
                    RestoreEntry(RawMouseId, enc);
                    _bk.Reg.Remove(RawMouseId);
                    changed = true;
                    log.Add(L.T("RawMouseThrottleDuration is back to your previous value (it only affected windows in the background)"));
                }
                catch (Exception e) { log.Add(L.F("RawMouseThrottleDuration: {0}", e.Message)); }
            }
            var byDevice = _bk.Reg.Keys.GroupBy(DeviceOfEntry).Where(g => g.Key != null).ToList();
            foreach (var g in byDevice)
            {
                bool stor;
                try { stor = StorageInstance(g.Key); }
                catch (Exception) { continue; }
                if (!stor) continue;
                try
                {
                    foreach (var id in g.Where(k => k.EndsWith("|*") && k.StartsWith("HKLM|", StringComparison.Ordinal))) Registry.LocalMachine.CreateSubKey(id.Split('|')[1], true)?.Dispose();
                    foreach (var id in g.Where(k => !k.EndsWith("|*")).ToList()) RestoreEntry(id, _bk.Reg[id]);
                    foreach (var id in g.ToList()) _bk.Reg.Remove(id);
                    changed = true;
                    log.Add(L.F("Storage controller {0}: its interrupt values are back as they were (storage is no longer touched); restart Windows", g.Key));
                }
                catch (Exception e) { log.Add(L.F("Storage controller {0}: {1}", g.Key, e.Message)); }
            }
            if (changed)
            {
                if (_bk.Reg.Count == 0 && _bk.Wmi.Count == 0 && _bk.UsbSuspend == null) File.Delete(BackupPath);
                else Save();
            }
            _bk = new();
        }
        else if (guessRawMouse)
        {
            try
            {
                using var k = Registry.CurrentUser.OpenSubKey(@"Control Panel\Mouse", true);
                if (k?.GetValue("RawMouseThrottleDuration") is int v && v == 20)
                {
                    k.DeleteValue("RawMouseThrottleDuration", false);
                    log.Add(L.T("RawMouseThrottleDuration 20 written by an older version was removed (it only affected windows in the background)"));
                }
            }
            catch (Exception) { }
        }
        return log;
    }
}
