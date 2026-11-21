using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Management;
using System.Reflection;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal sealed class AboutPage : Page
{
	public override string Title => L.T("About");

	public override string Section => "Settings";

	public AboutPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	private static List<string> Wmi(string cls, string prop)
	{
		List<string> list = new List<string>();
		try
		{
			using ManagementObjectSearcher managementObjectSearcher = new ManagementObjectSearcher("SELECT " + prop + " FROM " + cls);
			using ManagementObjectCollection managementObjectCollection = managementObjectSearcher.Get();
			foreach (ManagementBaseObject item in managementObjectCollection)
			{
				try
				{
					string text = item[prop]?.ToString()?.Trim() ?? "";
					if (text.Length > 0)
					{
						list.Add(text);
					}
				}
				finally
				{
					item.Dispose();
				}
			}
		}
		catch (Exception)
		{
		}
		return list;
	}

	public override FrameworkElement Build()
	{
		Topo topo = TopoBuilder.Live();
		TextBlock gpu = Ui.Text("…", 0.0, "Text", bold: false, wrap: true);
		TextBlock ram = Ui.Text("…", 0.0, "Text", bold: false, wrap: true);
		TextBlock board = Ui.Text("…", 0.0, "Text", bold: false, wrap: true);
		Task.Run(delegate
		{
			string g = string.Join(", ", Wmi("Win32_VideoController", "Name"));
			long result;
			long bytes = Wmi("Win32_PhysicalMemory", "Capacity").Sum((string x) => (!long.TryParse(x, out result)) ? 0 : result);
			string b = string.Join(" ", Wmi("Win32_BaseBoard", "Manufacturer").Concat(Wmi("Win32_BaseBoard", "Product")));
			S.Dispatcher.BeginInvoke((Action)delegate
			{
				gpu.Text = g;
				ram.Text = ((bytes > 0) ? Ui.Size(bytes) : "");
				board.Text = b;
			});
		});
		FactGrid factGrid = new FactGrid().Add(L.T("CPU"), (topo != null) ? (Native.CpuName() + " · " + topo.Split.Label) : Native.CpuName());
		factGrid.Children.Add(Fact(L.T("GPU"), gpu));
		factGrid.Children.Add(Fact(L.T("Memory"), ram));
		factGrid.Children.Add(Fact(L.T("Board"), board));
		if (topo != null)
		{
			factGrid.Add(L.T("Topology"), L.F("{0} logical, {1} physical, SMT {2}, groups {3}, L3 domains {4}", topo.LogicalCount, topo.PhysicalCount, topo.SMT ? L.T("on") : L.T("off"), topo.Groups, topo.LLCGroups.Count));
		}
		Assembly executingAssembly = Assembly.GetExecutingAssembly();
		string text2 = Environment.ProcessPath ?? "";
		FactGrid factGrid2 = new FactGrid(1).Add(L.T("Version"), AppVersion.Display + (AppVersion.IsTest ? " · " + L.T("test channel") : "")).Add(L.T("Build"), File.Exists(text2) ? File.GetLastWriteTime(text2).ToString("g", CultureInfo.CurrentCulture) : "").Add(L.T("Program"), text2)
			.Add(L.T("Data"), Store.DirShown)
			.Add(L.T("Log"), AppLog.ShownPath)
			.Add(L.T("Settings"), "HKCU\\Software\\DomainScheduler · HKLM\\SOFTWARE\\Domain OS\\Toolbox")
			.Add(L.T("Own load"), H.SelfLine())
			.Add(L.T("SelfTest"), (H.SelfTest.Count == 0) ? L.T("OK") : string.Join("; ", H.SelfTest));
		return Ui.Scroll(Ui.Col(Ui.Card(Ui.Col(Ui.Head(L.T("This computer")), factGrid)), Ui.Card(Ui.Col(Ui.Head("DomainScheduler"), factGrid2))));
		static UIElement Fact(string k, TextBlock v)
		{
			DockPanel obj = new DockPanel
			{
				Margin = Ui.MCell
			};
			TextBlock textBlock2 = Ui.Text(k, Ui.FsNote, "Dim");
			textBlock2.Margin = new Thickness(0.0, 0.0, 8.0, 0.0);
			obj.Children.Add(textBlock2);
			obj.Children.Add(v);
			return obj;
		}
	}
}
