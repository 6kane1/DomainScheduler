using System.Text.RegularExpressions;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal sealed class PowerPage : Page
{
	private StackPanel _list;

	public override string Title => L.T("Power");

	public override string Section => "Hardware";

	public PowerPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		_list = new StackPanel();
		TextBlock textBlock = new TextBlock
		{
			Text = L.T("Power plan"),
			FontSize = Ui.FsBody * 1.3,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong")
		};
		TextBlock textBlock2 = Ui.Text(L.T("Only the choice of the plan. Core parking, boost mode and similar hidden parameters are not edited here: on current processors changing them by hand gives no repeatable gain and often costs boost frequency."), Ui.FsNote, "Dim", bold: false, wrap: true);
		textBlock2.Margin = new Thickness(0.0, 2.0, 0.0, 12.0);
		return Ui.Scroll(Ui.Col(textBlock, textBlock2, _list));
	}

	public override void Shown()
	{
		Load();
	}

	private static string Explain(string guid)
	{
		return guid.ToLowerInvariant() switch
		{
			"381b4222-f694-41f0-9685-ff5bb260df2e" => L.T("Balanced. The plan AMD and Intel tune their current processors for: idle cores sleep, which leaves more power and temperature headroom for the cores that work. Recommended unless you measured otherwise."), 
			"8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" => L.T("High performance. Cores stay awake and at high frequency all the time: more power and heat when idle. It can shave a little wake-up latency on older processors; on current ones the gain is usually within noise."), 
			"a1841308-3541-4fab-bc81-f71556f20b4a" => L.T("Power saver. Lowers the processor's frequency: not for games."), 
			"e9a42b02-d5df-448d-aa00-03f14749eb61" => L.T("Ultimate Performance. High performance with a few more idle savings removed; made for workstations. No advantage over High performance in games."), 
			_ => L.T("A plan added by the maker of the PC, by a driver or by a tool. Check its name before using it."), 
		};
	}

	private void Load()
	{
		S.BusyTask(L.T("Reading the power plans…"), () => Hardware.Run("powercfg.exe", "/list", 15000), delegate((int Code, string Output) res)
		{
			_list.Children.Clear();
			MatchCollection matchCollection = Regex.Matches(res.Output ?? "", "([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\\s+\\((.+?)\\)\\s*(\\*)?");
			if (matchCollection.Count == 0)
			{
				_list.Children.Add(Ui.Note(L.F("The power plans could not be read: {0}", res.Output)));
				return;
			}
			foreach (Match item in matchCollection)
			{
				string guid = item.Groups[1].Value;
				string name = item.Groups[2].Value;
				bool success = item.Groups[3].Success;
				TextBlock textBlock = Ui.Text(name, 0.0, "Strong");
				textBlock.FontWeight = FontWeights.Medium;
				Border border = Ui.Pill(success ? L.T("Active") : L.T("Not active"), success);
				border.Margin = new Thickness(10.0, 0.0, 0.0, 0.0);
				Button button = Ui.Btn(L.T("Use this plan"), delegate
				{
					S.BusyTask(L.T("Writing…"), () => Hardware.Run("powercfg.exe", "/setactive " + guid, 15000), delegate((int Code, string Output) r)
					{
						S.Did((r.Code == 0) ? L.F("Power plan: {0}", name) : L.F("Power plan: {0}", r.Output), (r.Code != 0) ? AppLog.Sev.Error : AppLog.Sev.Info);
						Load();
					});
				}, 130.0);
				button.Margin = new Thickness(0.0);
				button.IsEnabled = !success;
				DockPanel dockPanel = new DockPanel();
				DockPanel.SetDock(button, Dock.Right);
				dockPanel.Children.Add(button);
				dockPanel.Children.Add(new StackPanel
				{
					Orientation = Orientation.Horizontal,
					VerticalAlignment = VerticalAlignment.Center,
					Children = 
					{
						(UIElement)textBlock,
						(UIElement)border
					}
				});
				TextBlock textBlock2 = Ui.Text(Explain(guid), Ui.FsNote, "Dim", bold: false, wrap: true);
				textBlock2.Margin = new Thickness(0.0, 6.0, 0.0, 0.0);
				_list.Children.Add(Ui.Card(Ui.Col(dockPanel, textBlock2), 11.0));
			}
		});
	}
}
