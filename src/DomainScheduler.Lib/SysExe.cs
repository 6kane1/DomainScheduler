using System.Diagnostics;

namespace DomainScheduler;

/// <summary>
/// Child processes of the program (1.15.0 Test 1.3). The program runs with administrator rights, and its environment
/// comes from the user's account, so nothing is looked up by name: every tool is started from System32 by its full
/// path, PowerShell never reads a profile and only sees the modules of Windows itself (a module of the same name in
/// the user's Documents is never loaded), and the environment variables that make a .NET or PowerShell process load
/// extra code (profilers, startup hooks) are taken out.
/// </summary>
public static class SysExe
{
    public static string System32(string exe) => Path.Combine(Environment.SystemDirectory, exe);

    public static string PowerShellPath => System32(@"WindowsPowerShell\v1.0\powershell.exe");

    /// <summary>The modules folder of Windows PowerShell itself (admin-only).</summary>
    public static string ModulesPath => System32(@"WindowsPowerShell\v1.0\Modules");

    /// <summary>Variables that would make a child load code chosen by whoever set them.</summary>
    public static bool Unsafe(string name) =>
        name.StartsWith("COR_", StringComparison.OrdinalIgnoreCase) || name.StartsWith("CORECLR_", StringComparison.OrdinalIgnoreCase)
        || name.StartsWith("COMPlus_", StringComparison.OrdinalIgnoreCase) || name.StartsWith("DOTNET_", StringComparison.OrdinalIgnoreCase)
        || name.Equals("PSModulePath", StringComparison.OrdinalIgnoreCase);

    static void Clean(ProcessStartInfo psi)
    {
        foreach (var k in psi.Environment.Keys.Where(Unsafe).ToList()) psi.Environment.Remove(k);
        psi.Environment["PSModulePath"] = ModulesPath;
    }

    /// <summary>A System32 tool by its full path, hidden, output read by the caller; add the arguments with ArgumentList.</summary>
    public static ProcessStartInfo Start(string exe, bool redirect = true)
    {
        var psi = new ProcessStartInfo(exe.Contains('\\') || exe.Contains('/') ? exe : System32(exe))
        {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = redirect, RedirectStandardError = redirect,
        };
        Clean(psi);
        return psi;
    }

    /// <summary>
    /// Windows PowerShell with a fixed command: -NoProfile -NonInteractive, the module path set to Windows' own folder
    /// (also inside the command, before anything is loaded). Values never go into the command text: pass them in env
    /// (read as $env:NAME in the command).
    /// </summary>
    public static ProcessStartInfo PowerShell(string command, IReadOnlyDictionary<string, string> env = null)
    {
        var psi = Start(PowerShellPath);
        psi.Environment["DOMAINOS_PSMODULES"] = ModulesPath;
        if (env != null) foreach (var (k, v) in env) psi.Environment[k] = v;
        foreach (var a in new[] { "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command",
                                  "$env:PSModulePath = $env:DOMAINOS_PSMODULES; " + command })
            psi.ArgumentList.Add(a);
        return psi;
    }
}
