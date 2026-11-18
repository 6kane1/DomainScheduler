using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace DomainScheduler.App;

internal sealed class Leg
{
	public Brush C;

	public string Look = "fill";

	public string Text = "";

	public UIElement Build()
	{
		Border element = new Border
		{
			Width = 10.0,
			Height = 10.0,
			Margin = new Thickness(0.0, 0.0, 5.0, 0.0),
			VerticalAlignment = VerticalAlignment.Center,
			Background = ((Look == "fill") ? C : Ui.B("None")),
			BorderBrush = C,
			BorderThickness = new Thickness(2.0)
		};
		return new StackPanel
		{
			Orientation = Orientation.Horizontal,
			Margin = new Thickness(0.0, 0.0, 14.0, 0.0),
			Children = 
			{
				(UIElement)element,
				(UIElement)Ui.Text(Text, Ui.FsNote, "Dim")
			}
		};
	}

	public static UIElement Line(params Leg[] legs)
	{
		WrapPanel wrapPanel = new WrapPanel
		{
			Margin = Ui.MRow
		};
		foreach (Leg leg in legs)
		{
			wrapPanel.Children.Add(leg.Build());
		}
		return wrapPanel;
	}
}
