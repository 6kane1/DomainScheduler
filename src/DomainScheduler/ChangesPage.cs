using System;
using System.Linq;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal sealed class ChangesPage : Page
{
	private StackPanel _list;

	public override string Title => L.T("System changes");

	public override string Section => "System";

	public ChangesPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		_list = new StackPanel();
		TextBlock textBlock = new TextBlock
		{
			Text = L.T("System changes"),
			FontSize = Ui.FsBody * 1.3,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong")
		};
		TextBlock textBlock2 = Ui.Text(L.T("This is the list the Domain OS playbook applies. It does not touch Defender, Secure Boot, memory integrity or anti-cheats. Each change can be switched back here."), Ui.FsNote, "Dim", bold: false, wrap: true);
		textBlock2.Margin = new Thickness(0.0, 2.0, 0.0, 12.0);
		Button button = Ui.Btn(L.T("Switch on all recommended"), delegate
		{
			int num = 0;
			foreach (Tweak item in Tweaks.Catalog.Where((Tweak t) => t.Recommended && Tweaks.State(t) != TweakState.On))
			{
				string text = Tweaks.Apply(item);
				if (text == null)
				{
					num++;
				}
				else
				{
					AppLog.Add(L.T(item.Title) + ": " + text, AppLog.Sev.Error);
				}
			}
			S.Did(L.F("System changes switched on: {0}", num));
			Draw();
		}, 200.0);
		Draw();
		return Ui.Scroll(Ui.Col(textBlock, textBlock2, Ui.Row(button), _list));
	}

	private void Draw()
	{
		_list.Children.Clear();
		foreach (IGrouping<string, Tweak> item in from t in Tweaks.Catalog
			group t by t.Group)
		{
			TextBlock textBlock = Ui.Text(L.T(item.Key), Ui.FsSmall, "Dim");
			textBlock.Margin = new Thickness(2.0, 8.0, 0.0, 6.0);
			_list.Children.Add(textBlock);
			foreach (Tweak item2 in item)
			{
				_list.Children.Add(Card(item2));
			}
		}
	}

	private UIElement Line(string label, string text, string brush)
	{
		TextBlock textBlock = Ui.Text(label, Ui.FsSmall, "Faint");
		textBlock.Width = 96.0;
		textBlock.VerticalAlignment = VerticalAlignment.Top;
		textBlock.Margin = new Thickness(0.0, 2.0, 0.0, 0.0);
		DockPanel obj = new DockPanel
		{
			Margin = new Thickness(0.0, 3.0, 0.0, 0.0)
		};
		DockPanel.SetDock(textBlock, Dock.Left);
		obj.Children.Add(textBlock);
		obj.Children.Add(Ui.Text(text, Ui.FsNote, brush, bold: false, wrap: true));
		return obj;
	}

	private static string Describe(RegChange c)
	{
		if (!c.IfKeyExists)
		{
			if (!(c.Hive == "BCD"))
			{
				return c.Hive + "\\…\\" + c.Name + " = " + ((c.On.Length > 12) ? (c.On.Substring(0, 12) + "…") : c.On);
			}
			return "bcdedit " + c.Name + " " + c.On;
		}
		return c.Path.Split('\\').Last() + " → " + L.T("disabled");
	}

	private UIElement Card(Tweak t)
	{
		TweakState state = Tweaks.State(t);
		Border border = Ui.Pill((state == TweakState.On) ? L.T("On") : ((state == TweakState.Mixed) ? L.T("Partly") : L.T("Off")), state == TweakState.On);
		border.Margin = new Thickness(10.0, 0.0, 0.0, 0.0);
		border.VerticalAlignment = VerticalAlignment.Center;
		TextBlock textBlock = Ui.Text(L.T(t.Title), 0.0, "Strong");
		textBlock.FontWeight = FontWeights.Medium;
		Button button = Ui.Btn((state == TweakState.On) ? L.T("Switch off") : L.T("Switch on"), delegate
		{
			string text = ((state == TweakState.On) ? Tweaks.Revert(t) : Tweaks.Apply(t));
			if (text == null)
			{
				S.Did((state == TweakState.On) ? L.F("{0}: off, the previous values are back", L.T(t.Title)) : L.F("{0}: on", L.T(t.Title)));
			}
			else
			{
				S.Did(L.T(t.Title) + ": " + text, AppLog.Sev.Error);
			}
			if (text == null && t.NeedsRestart)
			{
				S.Did(L.T("Restart Windows for this to take effect."));
			}
			Draw();
		}, 110.0);
		button.Margin = new Thickness(0.0);
		if (state != TweakState.On)
		{
			Ui.Primary(button);
		}
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
		return Ui.Card(Ui.Col(dockPanel, Line(L.T("What"), L.T(t.What), "Text"), Line(L.T("Why"), L.T(t.Why), "Dim"), Line(L.T("You give up"), L.T(t.Cost), "Dim"), Line(L.T("Changes"), string.Join("   ", t.Changes.Select(Describe)), "Faint")), 11.0);
	}
}
