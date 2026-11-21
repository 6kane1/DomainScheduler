using System.Runtime.InteropServices;
using System.Text.Json;

namespace DomainScheduler;

/// <summary>
/// Cleanup only. Test builds 1.9.1 and 1.10.0 changed power settings of the active plan while a game ran
/// (processor idle disable, core parking) and kept the values from before in game-power.json. Those features
/// are gone; Restore puts back every value the file still holds (left over from a crash or an old build) and
/// deletes the file. Nothing here changes a power setting on its own.
/// </summary>
public static class GamePower
{
    static readonly Guid SubProcessor = new("54533251-82be-4824-96c1-47b60b740d00");
    internal const string FileName = "game-power.json";
    static string File_ => Path.Combine(Store.Dir, FileName);
    static readonly object Gate = new();

    sealed class Saved { public Guid Scheme { get; set; } public Dictionary<string, uint> Values { get; set; } = new(); }

    /// <summary>
    /// The only settings those builds changed, with the range each one takes: processor idle disable (0/1) and the
    /// two core parking minimums (percent). Anything else in the file is never written.
    /// </summary>
    static readonly Dictionary<Guid, uint> Managed = new()
    {
        [new Guid("5d76a2ca-e8c0-402f-a133-2158492d58ad")] = 1,
        [new Guid("0cc5b647-c1df-4637-891a-dec35c318583")] = 100,
        [new Guid("0cc5b647-c1df-4637-891a-dec35c318584")] = 100,
    };

    /// <summary>One saved value may be put back (pure): a managed setting and a value in its range.</summary>
    public static bool ValidValue(string setting, uint value) =>
        Guid.TryParse(setting, out var g) && Managed.TryGetValue(g, out uint max) && value <= max;

    static Saved Clean(Saved s, List<string> dropped)
    {
        var c = new Saved { Scheme = s?.Scheme ?? Guid.Empty };
        foreach (var (k, v) in s?.Values ?? new())
            if (ValidValue(k, v)) c.Values[k] = v;
            else dropped?.Add(k);
        return c;
    }

    /// <summary>Takes over the file of an older version: only managed settings; null when nothing is left.</summary>
    internal static string ImportFile(string json, List<string> dropped)
    {
        var c = Clean(JsonSerializer.Deserialize<Saved>(json), dropped);
        return c.Scheme == Guid.Empty || c.Values.Count == 0 ? null : JsonSerializer.Serialize(c);
    }

    public static bool Applied
    {
        get
        {
            try { return File.Exists(File_); }
            catch (Exception) { return false; }
        }
    }

    /// <summary>Puts the saved values back (left over from a test build or a crash); "" when there is nothing to do.</summary>
    public static string Restore()
    {
        lock (Gate)
        {
            if (!Applied) return "";
            Saved saved;
            var dropped = new List<string>();
            try { saved = Clean(JsonSerializer.Deserialize<Saved>(File.ReadAllText(File_)), dropped); }
            catch (Exception) { try { File.Delete(File_); } catch (Exception) { } return ""; }
            if (dropped.Count > 0) AppLog.Add(L.F("Power for the game: saved settings the program does not manage were ignored: {0}", string.Join(", ", dropped.Take(5))), AppLog.Sev.Warn);
            var scheme = saved.Scheme;
            foreach (var (k, v) in saved.Values)
            {
                var sub = SubProcessor; var st = Guid.Parse(k);
                PowerWriteACValueIndex(IntPtr.Zero, ref scheme, ref sub, ref st, v);
            }
            if (PowerGetActiveScheme(IntPtr.Zero, out IntPtr ps) == 0)
            {
                Guid active = Marshal.PtrToStructure<Guid>(ps);
                LocalFree(ps);
                if (active == scheme) PowerSetActiveScheme(IntPtr.Zero, ref scheme);
            }
            try { File.Delete(File_); } catch (Exception) { }
            return L.T("Power for the game: the previous values are back");
        }
    }

    [DllImport("powrprof.dll")] static extern uint PowerGetActiveScheme(IntPtr root, out IntPtr scheme);
    [DllImport("powrprof.dll")] static extern uint PowerSetActiveScheme(IntPtr root, ref Guid scheme);
    [DllImport("powrprof.dll")] static extern uint PowerWriteACValueIndex(IntPtr root, ref Guid scheme, ref Guid sub, ref Guid setting, uint value);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
}
