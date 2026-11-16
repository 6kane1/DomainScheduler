using System.Globalization;
using Microsoft.Win32;

namespace DomainScheduler;

/// <summary>The ReservedCpuSets value (8 bytes, little endian); null from Read = absent.</summary>
public interface IReservedRegistry
{
    byte[] Read();
    void Write(byte[] value);
    void Delete();
}

/// <summary>HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Kernel → ReservedCpuSets.</summary>
public sealed class WinReservedRegistry : IReservedRegistry
{
    const string KeyPath = @"SYSTEM\CurrentControlSet\Control\Session Manager\Kernel", Name = "ReservedCpuSets";

    public byte[] Read()
    {
        using var k = Registry.LocalMachine.OpenSubKey(KeyPath);
        return k?.GetValue(Name, null, RegistryValueOptions.DoNotExpandEnvironmentNames) as byte[];
    }

    public void Write(byte[] value)
    {
        using var k = Registry.LocalMachine.OpenSubKey(KeyPath, true) ?? throw new InvalidOperationException(L.T("The Session Manager\\Kernel key cannot be opened"));
        k.SetValue(Name, value, RegistryValueKind.Binary);
    }

    public void Delete()
    {
        using var k = Registry.LocalMachine.OpenSubKey(KeyPath, true);
        k?.DeleteValue(Name, false);
    }
}

/// <summary>
/// System-wide reserved cores (section 8): after a reboot Windows keeps ordinary
/// threads off them. Only the exact non-game half of a split processor may be
/// reserved; never in the Auto path.
/// </summary>
public static class Reservation
{
    public static IReservedRegistry Reg = new WinReservedRegistry();
    public static Func<DateTime> BootUtc = () => DateTime.UtcNow - TimeSpan.FromMilliseconds(Native.GetTickCount64());
    public static Func<string> BackupPathHook;
    public static string BackupPath => BackupPathHook?.Invoke() ?? Path.Combine(Store.Dir, "reserved-cpusets.bak");

    public sealed class State
    {
        public string Kind = "";          // absent | set | error
        public byte[] Bytes;
        public ulong Mask;
        public string Error = "";
        public bool HasBackup;
        public string Original = "";      // backed-up value: hex or "absent"
        public bool TimeKnown;
        public DateTime Applied;
        public bool Pending;
    }

    public static ulong Unmix(ulong mask, ulong res, bool preferReserved)
    {
        if (res == 0) return mask;
        ulong inn = mask & res, out_ = mask & ~res;
        return preferReserved ? (inn != 0 ? inn : mask) : (out_ != 0 ? out_ : mask);
    }

    public static bool Mixes(ulong m, ulong res) => res != 0 && (m & res) != 0 && (m & ~res) != 0;

    static ulong ToMask(byte[] b)
    {
        if (b == null || b.Length == 0) return 0;
        var full = new byte[8];
        Array.Copy(b, full, Math.Min(8, b.Length));
        return BitConverter.ToUInt64(full, 0);
    }

    /// <summary>The reserved processors now (0: none or unreadable).</summary>
    public static ulong Reserved()
    {
        try { return ToMask(Reg.Read()); }
        catch (Exception) { return 0; }
    }

    /// <summary>Why a mask must not be reserved; empty = it may.</summary>
    public static List<string> Refusals(Topo t, ulong mask)
    {
        var r = new List<string>();
        if (t == null) { r.Add(L.T("The processor topology is not known")); return r; }
        if (t.Groups > 1) r.Add(L.T("More than one processor group"));
        if (t.LogicalCount > 64) r.Add(L.T("More than 64 logical processors"));
        if (mask == 0) r.Add(L.T("The mask is empty"));
        if ((mask & 1) != 0) r.Add(L.T("Processor 0 is never reserved"));
        var sp = t.Split;
        if (!sp.CanSplit) r.Add(L.F("The processor does not split: {0}", sp.Why()));
        if (!sp.IsSplitFamily) r.Add(L.T("Only X3D with two dies and Intel hybrid are split"));
        if (mask != sp.NonGame) r.Add(L.F("Only the whole non-game half ({0}) can be reserved", Masks.Ranges(sp.NonGame)));
        if ((mask & sp.Game) != 0) r.Add(L.T("The mask holds game cores"));
        if (!CoreSplit.SmtWhole(t, mask)) r.Add(L.T("The mask takes only one hardware thread of a core"));
        return r;
    }

    static (bool ok, string original, DateTime backupAt, DateTime applied) ReadBackup()
    {
        try
        {
            if (!File.Exists(BackupPath)) return (false, "", default, default);
            var parts = File.ReadAllText(BackupPath).Trim().Split('|');
            DateTime.TryParse(parts.Length > 1 ? parts[1] : "", CultureInfo.InvariantCulture, DateTimeStyles.AdjustToUniversal | DateTimeStyles.AssumeUniversal, out var at);
            DateTime.TryParse(parts.Length > 2 ? parts[2] : "", CultureInfo.InvariantCulture, DateTimeStyles.AdjustToUniversal | DateTimeStyles.AssumeUniversal, out var applied);
            return (true, parts[0], at, applied);
        }
        catch (Exception) { return (false, "", default, default); }
    }

    static void WriteBackup(string original, DateTime backupAt, DateTime applied)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(BackupPath)!);
        File.WriteAllText(BackupPath, original + "|" + backupAt.ToString("o", CultureInfo.InvariantCulture) + "|" + applied.ToString("o", CultureInfo.InvariantCulture));
    }

    public static State Read()
    {
        var s = new State();
        try
        {
            s.Bytes = Reg.Read();
            s.Kind = s.Bytes == null ? "absent" : "set";
            s.Mask = ToMask(s.Bytes);
        }
        catch (Exception e)
        {
            s.Kind = "error";
            s.Error = e.Message;
        }
        var (ok, orig, _, applied) = ReadBackup();
        s.HasBackup = ok;
        s.Original = orig;
        s.TimeKnown = ok && applied != default;
        s.Applied = applied;
        s.Pending = s.TimeKnown && applied > BootUtc();
        return s;
    }

    /// <summary>Reserves the non-game half: refusals → backup → write → byte read-back. Error text or null.</summary>
    public static int ApplyCalls;

    public static string ApplyNonGame(Topo t)
    {
        Interlocked.Increment(ref ApplyCalls);
        ulong mask = t?.Split.NonGame ?? 0;
        var refusals = Refusals(t, mask);
        if (refusals.Count > 0) return string.Join("; ", refusals);
        try
        {
            var (had, orig, at, _) = ReadBackup();
            if (!had)
            {
                var cur = Reg.Read();
                orig = cur == null ? "absent" : Convert.ToHexString(cur);
                at = DateTime.UtcNow;
            }
            var want = BitConverter.GetBytes(mask);
            WriteBackup(orig, at, DateTime.UtcNow);   // the backup exists before the registry is touched
            Reg.Write(want);
            var got = Reg.Read();
            if (got == null || !got.AsSpan().SequenceEqual(want)) return L.T("The value read back differs: the reservation is not confirmed");
            return null;
        }
        catch (Exception e) { return e.Message; }
    }

    /// <summary>Puts the backed-up value back (or deletes the value) and removes the backup.</summary>
    public static string Revert()
    {
        var (ok, orig, _, _) = ReadBackup();
        if (!ok) return L.T("No backup: the program did not reserve cores");
        try
        {
            if (orig == "absent") Reg.Delete();
            else Reg.Write(Convert.FromHexString(orig));
            File.Delete(BackupPath);
            return null;
        }
        catch (Exception e) { return e.Message; }
    }
}
