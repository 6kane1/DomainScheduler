using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security;

namespace DomainScheduler;

/// <summary>Where the logon task lives (Task Scheduler COM, schtasks.exe, or memory in tests).</summary>
public interface ITaskBackend
{
    /// <summary>The task's XML, null when there is no task.</summary>
    string Read(string name);
    void Register(string name, string xml);
    void Delete(string name);
}

/// <summary>Task Scheduler 2.0 through COM (Schedule.Service).</summary>
public sealed class ComTaskBackend : ITaskBackend
{
    const int TASK_CREATE_OR_UPDATE = 6, TASK_LOGON_INTERACTIVE_TOKEN = 3;

    static T WithFolder<T>(Func<dynamic, T> f)
    {
        Type t = Type.GetTypeFromProgID("Schedule.Service") ?? throw new InvalidOperationException("Task Scheduler is not available");
        dynamic svc = Activator.CreateInstance(t);
        dynamic folder = null;
        try
        {
            svc.Connect();
            folder = svc.GetFolder("\\");
            return f(folder);
        }
        finally
        {
            if (folder != null) Marshal.FinalReleaseComObject(folder);
            Marshal.FinalReleaseComObject(svc);
        }
    }

    public string Read(string name) => WithFolder<string>(folder =>
    {
        dynamic task = null;
        try
        {
            task = folder.GetTask(name);
            return (string)task.Xml;
        }
        catch (COMException) { return null; }
        catch (FileNotFoundException) { return null; }
        finally
        {
            if (task != null) Marshal.FinalReleaseComObject(task);
        }
    });

    public void Register(string name, string xml) => WithFolder<object>(folder =>
    {
        dynamic task = folder.RegisterTask(name, xml, TASK_CREATE_OR_UPDATE, null, null, TASK_LOGON_INTERACTIVE_TOKEN, null);
        Marshal.FinalReleaseComObject(task);
        return null;
    });

    public void Delete(string name) => WithFolder<object>(folder =>
    {
        try { folder.DeleteTask(name, 0); }
        catch (COMException) { }
        catch (FileNotFoundException) { }
        return null;
    });
}

/// <summary>schtasks.exe: the fallback when COM fails.</summary>
public sealed class SchtasksBackend : ITaskBackend
{
    static (int code, string output) Run(string args)
    {
        var psi = new ProcessStartInfo("schtasks.exe", args) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        using var p = Process.Start(psi)!;
        string o = p.StandardOutput.ReadToEnd();
        p.WaitForExit(15000);
        return (p.HasExited ? p.ExitCode : -1, o);
    }

    public string Read(string name)
    {
        var (code, output) = Run($"/Query /TN \"{name}\" /XML");
        return code == 0 && output.Contains("<Task", StringComparison.Ordinal) ? output : null;
    }

    public void Register(string name, string xml)
    {
        string tmp = Path.Combine(Path.GetTempPath(), "ds-task-" + Guid.NewGuid().ToString("N") + ".xml");
        try
        {
            File.WriteAllText(tmp, xml, System.Text.Encoding.Unicode);
            var (code, output) = Run($"/Create /TN \"{name}\" /XML \"{tmp}\" /F");
            if (code != 0) throw new InvalidOperationException("schtasks: " + output.Trim());
        }
        finally { File.Delete(tmp); }
    }

    public void Delete(string name) => Run($"/Delete /TN \"{name}\" /F");
}

/// <summary>
/// Autostart (section 23): a Task Scheduler logon task with the highest privileges, created
/// on the first run without asking, read back after creation. The Run key is never used.
/// </summary>
public static class Autostart
{
    public const string TaskName = "DomainScheduler";
    public const string Arg = "--autostart";
    public static ITaskBackend Primary = new ComTaskBackend();
    public static ITaskBackend Fallback = new SchtasksBackend();
    public static Func<string> UserHook;

    static string User => UserHook?.Invoke() ?? Environment.UserDomainName + "\\" + Environment.UserName;

    public static string Xml(string exePath) =>
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n" +
        "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n" +
        "  <RegistrationInfo><Description>DomainScheduler at logon</Description></RegistrationInfo>\n" +
        "  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>" + SecurityElement.Escape(User) + "</UserId></LogonTrigger></Triggers>\n" +
        "  <Principals><Principal id=\"Author\"><UserId>" + SecurityElement.Escape(User) + "</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>\n" +
        "  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>" +
        "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Enabled>true</Enabled><Priority>5</Priority></Settings>\n" +
        "  <Actions Context=\"Author\"><Exec><Command>" + SecurityElement.Escape(exePath) + "</Command><Arguments>" + Arg + "</Arguments></Exec></Actions>\n" +
        "</Task>";

    static string ReadAny()
    {
        try
        {
            string x = Primary.Read(TaskName);
            if (x != null) return x;
        }
        catch (Exception) { }
        try { return Fallback.Read(TaskName); }
        catch (Exception) { return null; }
    }

    public static bool TaskExists() => ReadAny() != null;

    /// <summary>The task exists and is enabled.</summary>
    public static bool TaskEnabled()
    {
        string x = ReadAny();
        if (x == null) return false;
        int i = x.LastIndexOf("<Settings>", StringComparison.Ordinal);
        return i < 0 || !x[i..].Contains("<Enabled>false</Enabled>", StringComparison.OrdinalIgnoreCase);
    }

    static bool Matches(string xml, string exePath) =>
        xml != null && xml.Contains(SecurityElement.Escape(exePath), StringComparison.OrdinalIgnoreCase) && xml.Contains("HighestAvailable", StringComparison.Ordinal);

    /// <summary>Creates (or updates) the task and checks it by reading it back.</summary>
    public static void TaskCreate(string exePath)
    {
        string xml = Xml(exePath);
        Exception first = null;
        try
        {
            Primary.Register(TaskName, xml);
            if (Matches(Primary.Read(TaskName), exePath)) return;
            first = new InvalidOperationException("the task read back differs");
        }
        catch (Exception e) { first = e; }
        AppLog.Add(L.F("Autostart through COM failed ({0}): schtasks.exe is used", first.Message), AppLog.Sev.Warn);
        Fallback.Register(TaskName, xml);
        if (!Matches(Fallback.Read(TaskName), exePath)) throw new InvalidOperationException(L.T("The autostart task read back differs"));
    }

    public static void TaskDelete()
    {
        try { Primary.Delete(TaskName); }
        catch (Exception) { }
        try
        {
            if (Fallback.Read(TaskName) != null) Fallback.Delete(TaskName);
        }
        catch (Exception) { }
    }

    /// <summary>First run: the task is created without asking (once; a task the user removed is not recreated).</summary>
    public static void EnsureOnFirstRun(string exePath)
    {
        if (Settings.TryStored("Autostart")) return;
        try
        {
            if (!TaskExists()) TaskCreate(exePath);
            Settings.Set("Autostart", "true");
            AppLog.Add(L.F("Autostart task {0} created", TaskName));
        }
        catch (Exception e) { AppLog.Add(L.F("Autostart task not created: {0}", e.Message), AppLog.Sev.Error); }
    }

    public static string ReportLine() => L.F("Autostart: task {0} ({1})", TaskName, TaskEnabled() ? L.T("on") : L.T("off"));
}
