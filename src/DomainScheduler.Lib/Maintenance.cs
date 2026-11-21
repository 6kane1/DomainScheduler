using Microsoft.Win32;

namespace DomainScheduler;

/// <summary>Factory reset / uninstall: the logon task, the settings (HKCU and HKLM) and the data folders go away.</summary>
public static class Maintenance
{
    /// <summary>Restores a crashed session first (nothing may stay changed), then removes everything the program created.</summary>
    public static List<string> FactoryReset(Engine engine)
    {
        var log = new List<string>();
        var s = SessionFile.Read();
        if (s != null && !s.CleanShutdown && engine != null) log.Add(L.F("Restored after a crash: {0} values", engine.RestoreSession(s)));
        // Their saved values live in the data folder, which goes away below: put them back first.
        try { string p = GamePower.Restore(); if (p.Length > 0) log.Add(p); } catch (Exception e) { log.Add(e.Message); }
        try { string d = DefenderGameExclusion.RemoveAll(); if (d.Length > 0) log.Add(d); } catch (Exception e) { log.Add(e.Message); }
        // Protection switches of older versions and the device interrupt changes: their backups are in the data folder too.
        try { log.AddRange(Tweaks.RevertRetired()); } catch (Exception e) { log.Add(e.Message); }
        try
        {
            if (IrqAuto.HasBackup || Devices.HasBackup)
            {
                string d = Devices.RevertAll();
                log.Add(d == null ? L.T("Device interrupt changes reverted; restart Windows") : L.F("Devices: {0}", d));
            }
        }
        catch (Exception e) { log.Add(e.Message); }
        Autostart.TaskDelete();
        log.Add(L.F("Autostart task {0} removed", Autostart.TaskName));
        try
        {
            Registry.CurrentUser.DeleteSubKeyTree(RegistrySettingsBackend.Root, false);
            Registry.LocalMachine.DeleteSubKeyTree(RegistrySettingsBackend.MachineRoot, false);
            log.Add(L.T("Settings removed"));
        }
        catch (Exception e) { log.Add(L.F("Settings: {0}", e.Message)); }
        try
        {
            // The state folder's contents; "native" holds the program's own unpacked libraries, in use while it runs.
            string dir = Store.Dir;
            foreach (var d in Directory.GetDirectories(dir).Where(d => !Path.GetFileName(d).Equals("native", StringComparison.OrdinalIgnoreCase)))
                Directory.Delete(d, true);
            foreach (var f in Directory.GetFiles(dir)) File.Delete(f);
            // The user's folder of versions before 1.15.0 Test 1.3 (already taken over and renamed *.migrated).
            if (Directory.Exists(Store.LegacyDir)) Directory.Delete(Store.LegacyDir, true);
            log.Add(L.T("Data folder removed"));
        }
        catch (Exception e) { log.Add(L.F("Data folder: {0}", e.Message)); }
        return log;
    }
}
