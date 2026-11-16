using System.IO;
using System.Globalization;
using System.Management;
using System.Reflection;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>The machine, the build, the paths, the font of the language, the report.</summary>
sealed class AboutPage : Page
{
    public AboutPage(Hub h, Shell s) : base(h, s) { }

    public override string Title => L.T("About");

    /// <summary>One WMI property of every object of a class; the searcher, the collection and the objects are disposed.</summary>
    static List<string> Wmi(string cls, string prop)
    {
        var list = new List<string>();
        try
        {
            using var searcher = new ManagementObjectSearcher("SELECT " + prop + " FROM " + cls);
            using var found = searcher.Get();
            foreach (ManagementBaseObject o in found)
            {
                try
                {
                    string v = o[prop]?.ToString()?.Trim() ?? "";
                    if (v.Length > 0) list.Add(v);
                }
                finally { o.Dispose(); }
            }
        }
        catch (Exception) { }
        return list;
    }

    public override FrameworkElement Build()
    {
        var t = TopoBuilder.Live();
        var gpu = Ui.Text("…", wrap: true);
        var ram = Ui.Text("…", wrap: true);
        var board = Ui.Text("…", wrap: true);
        Task.Run(() =>
        {
            string g = string.Join(", ", Wmi("Win32_VideoController", "Name"));
            long bytes = Wmi("Win32_PhysicalMemory", "Capacity").Sum(x => long.TryParse(x, out long n) ? n : 0);
            string b = string.Join(" ", Wmi("Win32_BaseBoard", "Manufacturer").Concat(Wmi("Win32_BaseBoard", "Product")));
            S.Dispatcher.BeginInvoke(() =>
            {
                gpu.Text = g;
                ram.Text = bytes > 0 ? Ui.Size(bytes) : "";
                board.Text = b;
            });
        });
        UIElement Fact(string k, TextBlock v)
        {
            var d = new DockPanel { Margin = Ui.MCell };
            var key = Ui.Text(k, Ui.FsNote, "Dim");
            key.Margin = new Thickness(0, 0, 8, 0);
            d.Children.Add(key);
            d.Children.Add(v);
            return d;
        }
        var hw = new FactGrid(2).Add(L.T("CPU"), t != null ? Native.CpuName() + " · " + t.Split.Label : Native.CpuName());
        hw.Children.Add(Fact(L.T("GPU"), gpu));
        hw.Children.Add(Fact(L.T("Memory"), ram));
        hw.Children.Add(Fact(L.T("Board"), board));
        if (t != null)
            hw.Add(L.T("Topology"), L.F("{0} logical, {1} physical, SMT {2}, groups {3}, L3 domains {4}", t.LogicalCount, t.PhysicalCount, t.SMT ? L.T("on") : L.T("off"), t.Groups, t.LLCGroups.Count));

        var asm = Assembly.GetExecutingAssembly();
        string exe = Environment.ProcessPath ?? "";
        var build = new FactGrid(1)
            .Add(L.T("Version"), asm.GetName().Version?.ToString(3) ?? "")
            .Add(L.T("Build"), File.Exists(exe) ? File.GetLastWriteTime(exe).ToString("g", CultureInfo.CurrentCulture) : "")
            .Add(L.T("Program"), exe)
            .Add(L.T("Data"), Store.Dir)
            .Add(L.T("Log"), AppLog.FilePath)
            .Add(L.T("Settings"), @"HKCU\" + RegistrySettingsBackend.Root)
            .Add(L.T("SelfTest"), H.SelfTest.Count == 0 ? L.T("OK") : string.Join("; ", H.SelfTest));

        var sample = Ui.Text(L.Family + " — AaBbCc 0123456789 АаБбВв 简体中文", Ui.FsBody * 1.3, wrap: true);
        return Ui.Scroll(Ui.Col(
            Ui.Card(Ui.Col(Ui.Head(L.T("This computer")), hw)),
            Ui.Card(Ui.Col(Ui.Head("DomainScheduler"), build)),
            Ui.Card(Ui.Col(Ui.Head(L.T("Font")), sample)),
            Ui.Row(Ui.Btn(L.T("Copy the report"), () => H.Opt.Run(H.Auto.Report, text =>
            {
                try
                {
                    Clipboard.SetText(text ?? "");
                    S.Did(L.T("Report copied to the clipboard"));
                }
                catch (Exception e) { S.Did(L.F("Clipboard: {0}", e.Message), AppLog.Sev.Warn); }
            })))));
    }
}
