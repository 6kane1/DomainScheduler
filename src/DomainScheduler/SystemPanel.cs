using System;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using Microsoft.Win32;

namespace DomainScheduler.App;

/// <summary>
/// "This system": what the PC is and how Windows is set up for games, in the free space of the right column
/// (home screen and advanced layout). Read once when the program starts (hardware facts come from the same
/// background read the footer uses), so it costs nothing while a game runs.
/// </summary>
internal sealed class SystemPanel
{
	private readonly TextBlock _cap = Ui.Head("");
	private readonly StackPanel _rows = new StackPanel();
	private Hardware.Facts _facts;

	public Border Root { get; }

	public SystemPanel()
	{
		DockPanel body = new DockPanel { LastChildFill = true };
		DockPanel.SetDock(_cap, Dock.Top);
		body.Children.Add(_cap);
		body.Children.Add(new ScrollViewer
		{
			Content = _rows,
			VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
			HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
			Focusable = false
		});
		Root = Ui.Panel(body, new Thickness(14.0, 12.0, 12.0, 8.0));
		Texts();
	}

	public void SetFacts(Hardware.Facts f)
	{
		_facts = f;
		Texts();
	}

	public void Texts()
	{
		Ui.SetHead(_cap, L.T("This system"));
		_rows.Children.Clear();
		Row(L.T("Windows"), WindowsText());
		Hardware.Facts f = _facts;
		if (f != null)
		{
			if (f.Cpu.Length > 0) Row(L.T("Processor"), f.Cpu.Trim());
			Hardware.Gpu g = f.Gpus.FirstOrDefault((Hardware.Gpu x) => !x.Basic) ?? f.Gpus.FirstOrDefault();
			if (g != null)
			{
				Row(L.T("Graphics card"), g.Name);
				if (g.Version.Length > 0) Row(L.T("Driver"), g.Date.Length > 0 ? g.Version + " · " + g.Date : g.Version);
			}
			if (f.RamNow > 0) Row(L.T("Memory"), f.RamRated > f.RamNow ? L.F("{0} MT/s (rated {1})", f.RamNow, f.RamRated) : L.F("{0} MT/s", f.RamNow));
			if (f.Refresh > 0) Row(L.T("Display"), L.F("{0} Hz", f.Refresh));
		}
		else
		{
			Row(L.T("Hardware"), L.T("reading…"));
		}
		Row(L.T("Game Mode"), GameMode() ? L.T("on") : L.T("off"), GameMode() ? "Accent" : "Warn");
	}

	private void Row(string key, string value, string brush = "Silver")
	{
		TextBlock k = Ui.Caps(HudMath.Spaced(key), Ui.FsSmall * 0.9);
		TextBlock v = Ui.Text(value, Ui.FsNote, brush, bold: false, wrap: true);
		v.Margin = new Thickness(0.0, 2.0, 0.0, 0.0);
		_rows.Children.Add(new Border
		{
			BorderBrush = Ui.B("Line2"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(0.0, 7.0, 0.0, 7.0),
			Child = new StackPanel { Children = { k, v } }
		});
	}

	/// <summary>"Windows 11 Pro 26H2 · 26300.9974" from the registry (ProductName still says Windows 10 on 11).</summary>
	private static string WindowsText()
	{
		try
		{
			using RegistryKey k = Registry.LocalMachine.OpenSubKey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
			if (k == null) return "";
			string edition = (k.GetValue("EditionID") as string ?? "") switch
			{
				"Professional" => "Pro",
				"ProfessionalN" => "Pro N",
				"ProfessionalWorkstation" => "Pro for Workstations",
				"Core" => "Home",
				"CoreSingleLanguage" => "Home",
				string e => e
			};
			string ver = k.GetValue("DisplayVersion") as string ?? "";
			string build = k.GetValue("CurrentBuild") as string ?? "";
			object ubr = k.GetValue("UBR");
			int.TryParse(build, out int b);
			string name = b >= 22000 ? "Windows 11" : "Windows 10";
			return $"{name} {edition} {ver}".Trim() + (build.Length > 0 ? " · " + build + (ubr != null ? "." + ubr : "") : "");
		}
		catch (Exception)
		{
			return "";
		}
	}

	private static bool GameMode()
	{
		try
		{
			using RegistryKey k = Registry.CurrentUser.OpenSubKey("Software\\Microsoft\\GameBar");
			object v = k?.GetValue("AutoGameModeEnabled");
			return v == null || Convert.ToInt32(v) != 0;   // not set = Windows' default, which is on
		}
		catch (Exception)
		{
			return true;
		}
	}
}
