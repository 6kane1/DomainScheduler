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
    static string File_ => Path.Combine(Store.Dir, "game-power.json");
    static readonly object Gate = new();

    sealed class Saved { public Guid Scheme { get; set; } public Dictionary<string, uint> Values { get; set; } = new(); }

    public static bool Applied => File.Exists(File_);

    /// <summary>Puts the saved values back (left over from a test build or a crash); "" when there is nothing to do.</summary>
    public static string Restore()
    {
        lock (Gate)
        {
            if (!Applied) return "";
            Saved saved;
            try { saved = JsonSerializer.Deserialize<Saved>(File.ReadAllText(File_)); }
            catch (Exception) { try { File.Delete(File_); } catch (Exception) { } return ""; }
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
