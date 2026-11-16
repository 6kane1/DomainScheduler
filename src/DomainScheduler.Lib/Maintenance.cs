using Microsoft.Win32;

namespace DomainScheduler;

/// <summary>Factory reset / uninstall: the logon task, the settings and the data folder go away.</summary>
public static class Maintenance
{
    /// <summary>Restores a crashed session first (nothing may stay changed), then removes everything the program created.</summary>
    public static List<string> FactoryReset(Engine engine)
    {
        var log = new List<string>();
        var s = SessionFile.Read();
        if (s != null && !s.CleanShutdown && engine != null) log.Add(L.F("Restored after a crash: {0} values", engine.RestoreSession(s)));
        Autostart.TaskDelete();
        log.Add(L.F("Autostart task {0} removed", Autostart.TaskName));
        try
        {
            Registry.CurrentUser.DeleteSubKeyTree(RegistrySettingsBackend.Root, false);
            log.Add(L.T("Settings removed"));
        }
        catch (Exception e) { log.Add(L.F("Settings: {0}", e.Message)); }
        try
        {
            if (Directory.Exists(Store.Dir)) Directory.Delete(Store.Dir, true);
            log.Add(L.T("Data folder removed"));
        }
        catch (Exception e) { log.Add(L.F("Data folder: {0}", e.Message)); }
        return log;
    }
}
