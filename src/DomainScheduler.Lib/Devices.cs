using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using Microsoft.Win32;

namespace DomainScheduler;

public enum IrqKind { Dpc, Isr }

public sealed class IrqResource
{
    public int Vector;
    public ulong Affinity;
    public ushort Group;
    public bool Message;          // true = MSI
    public IrqKind Kind;
}

public sealed class DeviceFacts
{
    public string DriverFile = "";
    public DateTime Restarted = DateTime.MinValue;   // from the Devices state file, not the registry
    public DateTime Boot = DateTime.MinValue;        // Reservation.BootUtc()
    public List<IrqResource> Resources = new();
    public List<KeyValuePair<string, ulong>> Others = new();   // pins of the other devices
}

/// <summary>
/// PCI devices and their interrupt affinity (section 7). Reading is used by the
/// game mask (pinned interrupts are kept off the game's cores); writing is a
/// separate, manual operation: backup → registry → device restart → read-back →
/// rollback on mismatch. The Auto path never writes.
/// </summary>
public static class Devices
{
    // Test hooks.
    public static Func<List<PciDevice>> ListHook;
    public static Func<uint, Tuple<int, int, List<int>>> AllocHook;   // devInst → (message interrupts, line interrupts, numbers)
    public static Func<string, byte[]> ResourceHook;                  // instance id → raw IRQ resource descriptor data
    public static Func<string, IrqConfig> ReadHook;                   // instance id → registry values

    internal const string BackupName = "irq-backup.json";
    public static string BackupPath => Path.Combine(Store.Dir, BackupName);

    /// <summary>A backup file exists (false when the state folder cannot be used).</summary>
    public static bool HasBackup
    {
        get
        {
            try { return File.Exists(BackupPath); }
            catch (Exception) { return false; }
        }
    }

    const string EnumRoot = @"SYSTEM\CurrentControlSet\Enum\";
    const string PolicySub = @"\Device Parameters\Interrupt Management\Affinity Policy";
    const string MsiSub = @"\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties";

    // ---- SetupAPI / CfgMgr32 -------------------------------------------------------------------
    const uint DIGCF_PRESENT = 0x2, DIGCF_ALLCLASSES = 0x4;
    const uint SPDRP_DEVICEDESC = 0x0, SPDRP_SERVICE = 0x4, SPDRP_CLASS = 0x7, SPDRP_DRIVER = 0x9, SPDRP_FRIENDLYNAME = 0xC,
               SPDRP_LOCATION_INFORMATION = 0xD, SPDRP_PHYSICAL_DEVICE_OBJECT_NAME = 0xE;
    const uint DIF_PROPERTYCHANGE = 0x12, DICS_PROPCHANGE = 3, DICS_FLAG_CONFIGSPECIFIC = 2;
    const uint ALLOC_LOG_CONF = 2, ResType_IRQ = 4, CR_SUCCESS = 0;

    [StructLayout(LayoutKind.Sequential)]
    struct SP_DEVINFO_DATA
    {
        public uint cbSize;
        public Guid ClassGuid;
        public uint DevInst;
        public IntPtr Reserved;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct SP_CLASSINSTALL_HEADER
    {
        public uint cbSize;
        public uint InstallFunction;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct SP_PROPCHANGE_PARAMS
    {
        public SP_CLASSINSTALL_HEADER Header;
        public uint StateChange;
        public uint Scope;
        public uint HwProfile;
    }

    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern IntPtr SetupDiGetClassDevs(IntPtr classGuid, string enumerator, IntPtr hwnd, uint flags);
    [DllImport("setupapi.dll", SetLastError = true)] static extern bool SetupDiEnumDeviceInfo(IntPtr set, uint index, ref SP_DEVINFO_DATA data);
    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool SetupDiGetDeviceInstanceId(IntPtr set, ref SP_DEVINFO_DATA data, StringBuilder id, int size, out int required);
    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool SetupDiGetDeviceRegistryProperty(IntPtr set, ref SP_DEVINFO_DATA data, uint prop, out uint regType, byte[] buf, uint size, out uint required);
    [DllImport("setupapi.dll", SetLastError = true)] static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    [DllImport("setupapi.dll", SetLastError = true)] static extern bool SetupDiSetClassInstallParams(IntPtr set, ref SP_DEVINFO_DATA data, ref SP_PROPCHANGE_PARAMS p, uint size);
    [DllImport("setupapi.dll", SetLastError = true)] static extern bool SetupDiCallClassInstaller(uint fn, IntPtr set, ref SP_DEVINFO_DATA data);
    [DllImport("cfgmgr32.dll", SetLastError = true)] static extern uint CM_Get_First_Log_Conf(out IntPtr logConf, uint devInst, uint flags);
    [DllImport("cfgmgr32.dll", SetLastError = true)] static extern uint CM_Get_Next_Res_Des(out IntPtr resDes, IntPtr current, uint forResource, out uint resourceId, uint flags);
    [DllImport("cfgmgr32.dll", SetLastError = true)] static extern uint CM_Get_Res_Des_Data_Size(out uint size, IntPtr resDes, uint flags);
    [DllImport("cfgmgr32.dll", SetLastError = true)] static extern uint CM_Get_Res_Des_Data(IntPtr resDes, byte[] buf, uint size, uint flags);
    [DllImport("cfgmgr32.dll", SetLastError = true)] static extern uint CM_Free_Log_Conf_Handle(IntPtr logConf);
    [DllImport("cfgmgr32.dll", SetLastError = true)] static extern uint CM_Free_Res_Des_Handle(IntPtr resDes);

    static readonly IntPtr Invalid = new(-1);

    static string Prop(IntPtr set, ref SP_DEVINFO_DATA d, uint prop)
    {
        var buf = new byte[1024];
        if (!SetupDiGetDeviceRegistryProperty(set, ref d, prop, out _, buf, (uint)buf.Length, out uint need)) return "";
        int n = (int)Math.Min(need, (uint)buf.Length);
        return Encoding.Unicode.GetString(buf, 0, n).TrimEnd('\0').Split('\0')[0];
    }

    /// <summary>The present PCI devices (no name or no instance: skipped).</summary>
    public static List<PciDevice> List()
    {
        if (ListHook != null) return ListHook() ?? new();
        var list = new List<PciDevice>();
        IntPtr set = SetupDiGetClassDevs(IntPtr.Zero, "PCI", IntPtr.Zero, DIGCF_PRESENT | DIGCF_ALLCLASSES);
        if (set == Invalid || set == IntPtr.Zero) return list;
        try
        {
            var d = new SP_DEVINFO_DATA { cbSize = (uint)Marshal.SizeOf<SP_DEVINFO_DATA>() };
            for (uint i = 0; SetupDiEnumDeviceInfo(set, i, ref d); i++)
            {
                var id = new StringBuilder(512);
                if (!SetupDiGetDeviceInstanceId(set, ref d, id, id.Capacity, out _)) continue;
                string name = Prop(set, ref d, SPDRP_FRIENDLYNAME);
                if (name.Length == 0) name = Prop(set, ref d, SPDRP_DEVICEDESC);
                if (name.Length == 0 || id.Length == 0) continue;
                list.Add(new PciDevice
                {
                    Name = name, Pdo = id.ToString(), Service = Prop(set, ref d, SPDRP_SERVICE),
                    DriverKey = Prop(set, ref d, SPDRP_DRIVER), DevInst = d.DevInst,
                    Location = Prop(set, ref d, SPDRP_LOCATION_INFORMATION),
                    Class = Prop(set, ref d, SPDRP_CLASS),
                });
                d.cbSize = (uint)Marshal.SizeOf<SP_DEVINFO_DATA>();
            }
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
        return list;
    }

    public static IEnumerable<(string Name, string Instance)> ListPciNames() => List().Select(d => (d.Name, d.Pdo));

    static object Value(RegistryKey k, string name) => k?.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames);

    /// <summary>The device's interrupt settings in the registry.</summary>
    public static IrqConfig ReadIrq(string instance)
    {
        if (ReadHook != null) return ReadHook(instance) ?? new IrqConfig();
        var c = new IrqConfig();
        try
        {
            using (var k = Registry.LocalMachine.OpenSubKey(EnumRoot + instance + PolicySub))
            {
                if (Value(k, "AssignmentSetOverride") is byte[] b && b.Length > 0)
                {
                    var full = new byte[8];
                    Array.Copy(b, full, Math.Min(8, b.Length));
                    c.Assign = BitConverter.ToUInt64(full, 0);
                }
                if (Value(k, "DevicePriority") is int p) c.Priority = p;
            }
            using (var k = Registry.LocalMachine.OpenSubKey(EnumRoot + instance + MsiSub))
            {
                if (Value(k, "MSISupported") is int m) c.Msi = m != 0;
                if (Value(k, "MessageNumberLimit") is int n) c.MsgLimit = n;
            }
        }
        catch (Exception) { }
        return c;
    }

    /// <summary>Devices whose interrupts are pinned to given processors (AssignmentSetOverride ≠ 0).</summary>
    public static List<KeyValuePair<string, ulong>> PinnedIrqs()
    {
        var res = new List<KeyValuePair<string, ulong>>();
        foreach (var (name, inst) in ListPciNames())
        {
            var c = ReadIrq(inst);
            if (c.Assign.HasValue && c.Assign.Value != 0) res.Add(new(name, c.Assign.Value));
        }
        return res;
    }

    /// <summary>The interrupts Windows allocated to a device: (message, line, numbers).</summary>
    public static Tuple<int, int, List<int>> Allocated(uint devInst)
    {
        if (AllocHook != null) return AllocHook(devInst);
        var res = Resources(devInst);
        return Tuple.Create(res.Count(r => r.Message), res.Count(r => !r.Message), res.Select(r => r.Vector).ToList());
    }

    /// <summary>Allocated interrupt resources of a device (CfgMgr32, ALLOC_LOG_CONF, ResType_IRQ).</summary>
    public static List<IrqResource> Resources(uint devInst)
    {
        var list = new List<IrqResource>();
        if (CM_Get_First_Log_Conf(out IntPtr conf, devInst, ALLOC_LOG_CONF) != CR_SUCCESS) return list;
        try
        {
            IntPtr cur = conf;
            while (CM_Get_Next_Res_Des(out IntPtr rd, cur, ResType_IRQ, out _, 0) == CR_SUCCESS)
            {
                try
                {
                    if (CM_Get_Res_Des_Data_Size(out uint size, rd, 0) == CR_SUCCESS && size >= 16)
                    {
                        var data = new byte[size];
                        if (CM_Get_Res_Des_Data(rd, data, size, 0) == CR_SUCCESS) list.AddRange(ParseIrq(data));
                    }
                }
                finally
                {
                    if (cur != conf) CM_Free_Res_Des_Handle(cur);
                }
                cur = rd;
            }
            if (cur != conf) CM_Free_Res_Des_Handle(cur);
        }
        finally { CM_Free_Log_Conf_Handle(conf); }
        return list;
    }

    /// <summary>
    /// IRQ_DES_64 records (24 bytes each): Count, Type, Flags, Alloc_Num, Affinity. Message
    /// signalled interrupts carry the top bit of Alloc_Num. CfgMgr32 lists the interrupt (ISR)
    /// resources; DPC routines are not resources and are not visible here.
    /// </summary>
    public static List<IrqResource> ParseIrq(byte[] data)
    {
        var list = new List<IrqResource>();
        if (data == null) return list;
        for (int off = 0; off + 16 <= data.Length; off += 24)
        {
            uint alloc = BitConverter.ToUInt32(data, off + 12);
            ulong aff = off + 24 <= data.Length ? BitConverter.ToUInt64(data, off + 16) : 0;
            list.Add(new IrqResource { Vector = unchecked((int)alloc), Affinity = aff, Group = 0, Message = (alloc & 0x80000000) != 0, Kind = IrqKind.Isr });
        }
        return list;
    }

    /// <summary>Raw IRQ descriptor data of a device (tests).</summary>
    public static byte[] Resource(string instance) => ResourceHook?.Invoke(instance);

    // ---- facts (section 20): read-only diagnostics, used by the Report only ----------------------

    public static Func<string> StatePathHook;
    public static string StatePath => StatePathHook?.Invoke() ?? Path.Combine(Store.Dir, "devices-state.json");

    static Dictionary<string, DateTime> ReadState()
    {
        try { return File.Exists(StatePath) ? JsonSerializer.Deserialize<Dictionary<string, DateTime>>(File.ReadAllText(StatePath)) ?? new() : new(); }
        catch (Exception) { return new(); }
    }

    static void MarkRestarted(string pdo)
    {
        try
        {
            var st = ReadState();
            st[pdo] = DateTime.UtcNow;
            Directory.CreateDirectory(Path.GetDirectoryName(StatePath)!);
            File.WriteAllText(StatePath, JsonSerializer.Serialize(st));
        }
        catch (Exception) { }
    }

    static string DriverFileOf(string service)
    {
        if (string.IsNullOrEmpty(service)) return "";
        try
        {
            using var k = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services\" + service);
            return Value(k, "ImagePath") is string p ? Path.GetFileName(p.Trim('"')) : "";
        }
        catch (Exception) { return ""; }
    }

    public static DeviceFacts Facts(PciDevice dev, List<KeyValuePair<string, ulong>> allPins)
    {
        var f = new DeviceFacts { DriverFile = DriverFileOf(dev.Service) };
        try { f.Boot = Reservation.BootUtc(); }
        catch (Exception) { f.Boot = DateTime.MinValue; }
        if (ReadState().TryGetValue(dev.Pdo, out var t)) f.Restarted = t;
        byte[] raw = Resource(dev.Pdo);
        f.Resources = raw != null ? ParseIrq(raw) : Resources(dev.DevInst);
        f.Others = (allPins ?? new()).Where(p => p.Key != dev.Name).ToList();
        return f;
    }

    // ---- writing (manual only) -----------------------------------------------------------------

    sealed class Backup
    {
        public Dictionary<string, Dictionary<string, string>> Devices { get; set; } = new();
    }

    static readonly string[] BackedUpNames = { "AssignmentSetOverride", "DevicePolicy", "DevicePriority", "MSISupported", "MessageNumberLimit" };

    /// <summary>
    /// One device of the backup may be restored (pure): a PCI instance ID and, for every value, a name and range this
    /// program writes (<see cref="IrqAuto.ValidIrqValue"/>).
    /// </summary>
    public static bool ValidBackupDevice(string pdo, Dictionary<string, string> vals) =>
        pdo != null && pdo.StartsWith(@"PCI\", StringComparison.OrdinalIgnoreCase) && IrqAuto.InstanceRx.IsMatch(pdo) && vals != null
        && vals.All(kv => BackedUpNames.Contains(kv.Key) && IrqAuto.ValidIrqValue(kv.Key, kv.Value ?? ""));

    static Backup Clean(Backup b, List<string> dropped)
    {
        var c = new Backup();
        foreach (var (pdo, vals) in b?.Devices ?? new())
            if (ValidBackupDevice(pdo, vals)) c.Devices[pdo] = vals;
            else dropped?.Add(pdo);
        return c;
    }

    static Backup LoadBackup()
    {
        if (!File.Exists(BackupPath)) return new Backup();
        var dropped = new List<string>();
        var b = Clean(JsonSerializer.Deserialize<Backup>(File.ReadAllText(BackupPath)), dropped);
        if (dropped.Count > 0) AppLog.Add(L.F("Device backup: entries that are not values the program writes were ignored: {0}", string.Join(", ", dropped.Take(5))), AppLog.Sev.Warn);
        return b;
    }

    /// <summary>Takes over the manual device backup of an older version: only valid devices; null when none is left.</summary>
    internal static string ImportBackup(string json, string existing, List<string> dropped)
    {
        var b = Clean(JsonSerializer.Deserialize<Backup>(json), dropped);
        var cur = existing == null ? new Backup() : Clean(JsonSerializer.Deserialize<Backup>(existing), null);
        foreach (var (k, v) in b.Devices) cur.Devices.TryAdd(k, v);
        return cur.Devices.Count == 0 ? null : JsonSerializer.Serialize(cur);
    }

    static string Enc(object v) => v switch
    {
        null => "",
        byte[] b => "b:" + Convert.ToHexString(b),
        int i => "d:" + i.ToString(CultureInfo.InvariantCulture),
        _ => "",
    };

    static void SetOrDelete(RegistryKey k, string name, string enc)
    {
        if (enc.StartsWith("b:")) k.SetValue(name, Convert.FromHexString(enc[2..]), RegistryValueKind.Binary);
        else if (enc.StartsWith("d:")) k.SetValue(name, int.Parse(enc[2..], CultureInfo.InvariantCulture), RegistryValueKind.DWord);
        else k.DeleteValue(name, false);
    }

    /// <summary>
    /// Writes a device's interrupt policy: backup → values → device restart → read-back; on a
    /// mismatch the backup is put back. Returns an error text, null on success.
    /// </summary>
    public static int WriteCalls;

    public static string Write(PciDevice dev, IrqConfig want) => Write(dev, want, true);

    /// <summary>restart = false: only the registry is written (and read back); the policy starts at the next start of Windows.</summary>
    public static string Write(PciDevice dev, IrqConfig want, bool restart)
    {
        Interlocked.Increment(ref WriteCalls);
        if (Flags.Refuse(Flags.DevicesWrite, L.T("Device policy write"))) return L.T("The feature flag devices-write is closed");
        if (dev == null || string.IsNullOrEmpty(dev.Pdo)) return L.T("No device");
        if (IsStorage(dev)) return L.T("Storage controllers are never changed: a wrong value there can stop Windows from starting");
        string polPath = EnumRoot + dev.Pdo + PolicySub, msiPath = EnumRoot + dev.Pdo + MsiSub;
        try
        {
            // The boot guard's marker first: Windows that does not start fine afterwards gets the backup back.
            try { BootGuard.MarkPending(); }
            catch (Exception e) { return L.F("Nothing was written: the safety marker could not be saved ({0})", e.Message); }
            // Backup of the original values (kept from the first change).
            var bk = LoadBackup();
            if (!bk.Devices.ContainsKey(dev.Pdo))
            {
                using var pk = Registry.LocalMachine.OpenSubKey(polPath);
                using var mk = Registry.LocalMachine.OpenSubKey(msiPath);
                bk.Devices[dev.Pdo] = new Dictionary<string, string>
                {
                    ["AssignmentSetOverride"] = Enc(Value(pk, "AssignmentSetOverride")),
                    ["DevicePolicy"] = Enc(Value(pk, "DevicePolicy")),
                    ["DevicePriority"] = Enc(Value(pk, "DevicePriority")),
                    ["MSISupported"] = Enc(Value(mk, "MSISupported")),
                    ["MessageNumberLimit"] = Enc(Value(mk, "MessageNumberLimit")),
                };
                Directory.CreateDirectory(Store.Dir);
                File.WriteAllText(BackupPath + ".tmp", JsonSerializer.Serialize(bk));
                File.Move(BackupPath + ".tmp", BackupPath, true);
            }
            using (var pk = Registry.LocalMachine.CreateSubKey(polPath, true))
            {
                if (want.Assign.HasValue && want.Assign.Value != 0)
                {
                    pk.SetValue("AssignmentSetOverride", BitConverter.GetBytes(want.Assign.Value), RegistryValueKind.Binary);
                    pk.SetValue("DevicePolicy", 4, RegistryValueKind.DWord);   // IrqPolicySpecifiedProcessors
                }
                else
                {
                    pk.DeleteValue("AssignmentSetOverride", false);
                    pk.DeleteValue("DevicePolicy", false);
                }
                if (want.Priority.HasValue) pk.SetValue("DevicePriority", want.Priority.Value, RegistryValueKind.DWord);
                else pk.DeleteValue("DevicePriority", false);
            }
            using (var mk = Registry.LocalMachine.CreateSubKey(msiPath, true))
            {
                mk.SetValue("MSISupported", want.Msi ? 1 : 0, RegistryValueKind.DWord);
                if (want.MsgLimit > 0) mk.SetValue("MessageNumberLimit", want.MsgLimit, RegistryValueKind.DWord);
                else mk.DeleteValue("MessageNumberLimit", false);
            }
            string restarted = restart ? Restart(dev) : null;
            var got = ReadIrq(dev.Pdo);
            bool same = (got.Assign ?? 0) == (want.Assign ?? 0) && got.Priority == want.Priority && got.Msi == want.Msi && got.MsgLimit == want.MsgLimit;
            if (!same)
            {
                RestoreValues(dev.Pdo, bk.Devices[dev.Pdo]);
                if (restart) Restart(dev);
                return L.T("The values did not stick: the backup was put back");
            }
            return restarted;
        }
        catch (UnauthorizedAccessException) { return L.T("Access denied: run as administrator"); }
        catch (Exception e) { return e.Message; }
    }

    static void RestoreValues(string pdo, Dictionary<string, string> vals)
    {
        if (!ValidBackupDevice(pdo, vals))
        {
            AppLog.Add(L.F("Device backup: {0} holds values the program does not write; it was not restored", pdo), AppLog.Sev.Warn);
            return;
        }
        using (var pk = Registry.LocalMachine.CreateSubKey(EnumRoot + pdo + PolicySub, true))
            foreach (var n in new[] { "AssignmentSetOverride", "DevicePolicy", "DevicePriority" })
                SetOrDelete(pk, n, vals.GetValueOrDefault(n, ""));
        using (var mk = Registry.LocalMachine.CreateSubKey(EnumRoot + pdo + MsiSub, true))
            foreach (var n in new[] { "MSISupported", "MessageNumberLimit" })
                SetOrDelete(mk, n, vals.GetValueOrDefault(n, ""));
    }

    /// <summary>A storage controller or disk (never written; see <see cref="IrqAuto.IsStorage"/>).</summary>
    public static bool IsStorage(PciDevice dev) => dev != null && IrqAuto.IsStorage(dev.Class, dev.Name, dev.Service);

    /// <summary>Puts every backed-up device back and removes the backup (and the boot guard's marker).</summary>
    public static string RevertAll()
    {
        try { IrqAuto.RevertExtras(); } catch (Exception) { }
        if (!HasBackup) { BootGuard.Clear(); return null; }
        try
        {
            var bk = LoadBackup();
            var devs = List().ToDictionary(d => d.Pdo, StringComparer.OrdinalIgnoreCase);
            foreach (var (pdo, vals) in bk.Devices)
            {
                RestoreValues(pdo, vals);
                if (devs.TryGetValue(pdo, out var d)) Restart(d);
            }
            File.Delete(BackupPath);
            BootGuard.Clear();
            return null;
        }
        catch (Exception e) { return e.Message; }
    }

    /// <summary>Restarts a device so the interrupt policy takes effect (DIF_PROPERTYCHANGE / DICS_PROPCHANGE).</summary>
    static string Restart(PciDevice dev)
    {
        IntPtr set = SetupDiGetClassDevs(IntPtr.Zero, "PCI", IntPtr.Zero, DIGCF_PRESENT | DIGCF_ALLCLASSES);
        if (set == Invalid || set == IntPtr.Zero) return L.T("Device list unavailable");
        try
        {
            var d = new SP_DEVINFO_DATA { cbSize = (uint)Marshal.SizeOf<SP_DEVINFO_DATA>() };
            for (uint i = 0; SetupDiEnumDeviceInfo(set, i, ref d); i++)
            {
                if (d.DevInst != dev.DevInst) continue;
                var p = new SP_PROPCHANGE_PARAMS
                {
                    Header = new SP_CLASSINSTALL_HEADER { cbSize = (uint)Marshal.SizeOf<SP_CLASSINSTALL_HEADER>(), InstallFunction = DIF_PROPERTYCHANGE },
                    StateChange = DICS_PROPCHANGE, Scope = DICS_FLAG_CONFIGSPECIFIC, HwProfile = 0,
                };
                if (!SetupDiSetClassInstallParams(set, ref d, ref p, (uint)Marshal.SizeOf<SP_PROPCHANGE_PARAMS>()) ||
                    !SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, ref d))
                    return L.F("The device did not restart (error {0}): the change applies after a reboot", Marshal.GetLastWin32Error());
                MarkRestarted(dev.Pdo);
                return null;
            }
            return L.T("The device was not found for a restart");
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
    }
}
