using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal sealed class DriversPage : Page
{
	private StackPanel _list;

	public override string Title => L.T("Drivers");

	public override string Section => "Hardware";

	public DriversPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		_list = new StackPanel();
		TextBlock textBlock = new TextBlock
		{
			Text = L.T("Drivers"),
			FontSize = Ui.FsBody * 1.3,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong")
		};
		TextBlock textBlock2 = Ui.Text(L.T("Domain OS does not let Windows Update install drivers, so these are yours to install, top to bottom. Links lead to the makers' official pages only."), Ui.FsNote, "Dim", bold: false, wrap: true);
		textBlock2.Margin = new Thickness(0.0, 2.0, 0.0, 12.0);
		string guide = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Domain OS", "Guide", "New-DriverGuide.ps1");
		StackPanel stackPanel = Ui.Row(Ui.Btn(L.T("Refresh"), Load));
		if (File.Exists(guide))
		{
			stackPanel.Children.Add(Ui.Btn(L.T("Open the step-by-step guide"), delegate
			{
				try
				{
					// Windows PowerShell by its full path, no profile, only Windows' own modules (SysExe).
					ProcessStartInfo psi = SysExe.Start(SysExe.PowerShellPath, redirect: false);
					foreach (string a in new[] { "-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File", guide })
					{
						psi.ArgumentList.Add(a);
					}
					Process.Start(psi);
				}
				catch (Exception ex)
				{
					S.Did(L.F("Guide: {0}", ex.Message), AppLog.Sev.Warn);
				}
			}, 220.0));
		}
		return Ui.Scroll(Ui.Col(textBlock, textBlock2, stackPanel, _list));
	}

	public override void Shown()
	{
		Load();
	}

	private void Load()
	{
		S.BusyTask(L.T("Reading the hardware…"), Hardware.Detect, delegate(Hardware.Facts f)
		{
			_list.Children.Clear();
			if (f != null)
			{
				string item2 = L.T("Open the official page");
				string item3 = Hardware.BoardLink(f.BoardVendor);
				Step(L.T("Network driver"), string.Join(", ", f.Net), f.Net.Count == 0, L.T("Without it nothing else can be downloaded. The driver from the maker of the board supports every function of the adapter."), null, new(string, string)[1] { (item2, item3) });
				Step(L.T("Chipset driver"), f.Cpu, bad: false, L.T("Install it first and restart. It gives Windows the right power and core information for this processor; on Ryzen X3D with two chip blocks it contains the component that keeps games on the big-cache cores."), null, new(string, string)[1] { (item2, Hardware.ChipsetLink(f.CpuVendor)) });
				foreach (Hardware.Gpu gpu in f.Gpus)
				{
					string state2 = (gpu.Basic ? (gpu.Name + " — " + L.T("the real driver is missing")) : (gpu.Name + " · " + gpu.Version + " · " + gpu.Date));
					if (gpu.Vendor == "nvidia")
					{
						Step(L.T("Graphics driver"), state2, gpu.Basic, L.T("NVCleanstall installs the NVIDIA driver without the extras: you lose game recording and automatic driver updates from NVIDIA, nothing else."), new string[6]
						{
							L.T("Download NVCleanstall from its official page (TechPowerUp) and start it."),
							L.T("Choose \"Install best driver for my hardware\"."),
							L.T("Keep only Display Driver. Add HD Audio via HDMI only if sound goes through the monitor, PhysX only if a game asks for it."),
							L.T("Leave NVIDIA app / GeForce Experience and telemetry unticked."),
							L.T("On the tweaks page tick only \"Perform a clean installation\" and \"Disable installer telemetry and advertising\"."),
							L.T("Install and restart.")
						}, new(string, string)[2]
						{
							(L.T("NVCleanstall (official page)"), "https://www.techpowerup.com/download/techpowerup-nvcleanstall/"),
							(L.T("NVIDIA drivers"), Hardware.GpuLink("nvidia"))
						});
					}
					else
					{
						Step(L.T("Graphics driver"), state2, gpu.Basic, L.T("Without the maker's driver games run on a basic adapter with no acceleration."), (!(gpu.Vendor == "amd")) ? null : new string[2]
						{
							L.T("In the AMD installer choose \"Driver Only\" (or \"Minimal Install\" if you want the settings panel)."),
							L.T("Tick \"Factory Reset\" for a clean installation if it is offered, then restart.")
						}, new(string, string)[1] { (item2, Hardware.GpuLink(gpu.Vendor)) });
					}
				}
				Step(L.T("Audio driver"), string.Join(", ", f.Audio), f.Audio.Count == 0, L.T("The generic driver works; the maker's one adds the outputs, microphone processing and the control panel of your board. Choose your exact model on the support page."), null, new(string, string)[1] { (item2, item3) });
				Step(L.T("Devices without a driver"), (f.Problems.Count == 0) ? L.T("none") : string.Join(", ", f.Problems.Take(6)), f.Problems.Count > 0, L.T("A device with a yellow mark in Device Manager does not work and may keep the system from sleeping or saving power correctly."), null, Array.Empty<(string, string)>());
				if (f.Refresh > 0)
				{
					Step(L.T("Refresh rate of the monitor"), f.Refresh + " Hz", f.Refresh <= 60, L.T("Windows often starts at 60 Hz on a faster monitor. Settings → System → Display → Advanced display: choose the highest rate your monitor supports."), null, Array.Empty<(string, string)>());
				}
				if (f.RamRated > 0 && f.RamNow > 0 && f.RamNow < f.RamRated - 50)
				{
					Step(L.T("Memory profile (XMP / EXPO)"), L.F("now {0} MHz, rated {1} MHz", f.RamNow, f.RamRated), bad: true, L.T("The memory runs slower than it was sold as, which lowers the minimum FPS. Turn on the XMP or EXPO profile in the BIOS."), null, Array.Empty<(string, string)>());
				}
			}
		});
		void Step(string name, string state, bool bad, string why, string[] how, (string Text, string Url)[] links)
		{
			TextBlock textBlock = Ui.Text(name, 0.0, "Strong");
			textBlock.FontWeight = FontWeights.Medium;
			Border border = Ui.Pill((state.Length > 0) ? state : L.T("not found"), accent: false);
			border.Margin = new Thickness(0.0, 6.0, 0.0, 0.0);
			if (bad)
			{
				((TextBlock)border.Child).Foreground = Ui.B("Warn");
			}
			StackPanel stackPanel = Ui.Col(textBlock, border);
			int num = 1;
			string[] array = how ?? Array.Empty<string>();
			foreach (string text in array)
			{
				TextBlock textBlock2 = Ui.Text(num++ + ".  " + text, Ui.FsNote, "Text", bold: false, wrap: true);
				textBlock2.Margin = new Thickness(0.0, 5.0, 0.0, 0.0);
				stackPanel.Children.Add(textBlock2);
			}
			TextBlock textBlock3 = Ui.Text(why, Ui.FsNote, "Dim", bold: false, wrap: true);
			textBlock3.Margin = new Thickness(0.0, 7.0, 0.0, 0.0);
			stackPanel.Children.Add(textBlock3);
			StackPanel stackPanel2 = Ui.Row();
			stackPanel2.Margin = new Thickness(0.0, 10.0, 0.0, 0.0);
			bool flag = true;
			foreach (var item4 in links.Where(((string Text, string Url) l) => l.Url.Length > 0))
			{
				string item = item4.Text;
				string url = item4.Url;
				Button button = Ui.Btn(item, delegate
				{
					Hardware.OpenLink(url);
				}, 190.0);
				if (flag)
				{
					Ui.Primary(button);
				}
				flag = false;
				stackPanel2.Children.Add(button);
			}
			if (stackPanel2.Children.Count > 0)
			{
				stackPanel.Children.Add(stackPanel2);
			}
			_list.Children.Add(Ui.Card(stackPanel, 11.0));
		}
	}
}
