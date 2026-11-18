using System;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace DomainScheduler.App;

internal sealed class CheckRow : Border
{
	private bool _checked;

	private readonly TextBlock _hint;

	private readonly Border _box;

	private readonly TextBlock _mark;

	public bool Checked => _checked;

	public event Action<bool> Changed;

	public CheckRow(string label, string hint, bool value)
	{
		_checked = value;
		_box = new Border
		{
			Width = 16.0,
			Height = 16.0,
			CornerRadius = new CornerRadius(2.0),
			BorderThickness = new Thickness(1.0),
			BorderBrush = Ui.B("Line"),
			Margin = new Thickness(0.0, 1.0, 10.0, 0.0),
			VerticalAlignment = VerticalAlignment.Top
		};
		_mark = new TextBlock
		{
			Text = "✓",
			FontSize = Ui.FsBody * 0.9,
			FontFamily = Ui.Mono,
			Foreground = Ui.B("Bg"),
			HorizontalAlignment = HorizontalAlignment.Center,
			VerticalAlignment = VerticalAlignment.Center
		};
		_box.Child = _mark;
		_hint = Ui.Note(hint);
		_hint.Visibility = (string.IsNullOrEmpty(hint) ? Visibility.Collapsed : Visibility.Visible);
		StackPanel element = Ui.Col(Ui.Text(label, 0.0, "Text", bold: false, wrap: true), _hint);
		DockPanel dockPanel = new DockPanel();
		DockPanel.SetDock(_box, Dock.Left);
		dockPanel.Children.Add(_box);
		dockPanel.Children.Add(element);
		Child = dockPanel;
		base.Background = Ui.B("None");
		base.Margin = Ui.MRow;
		base.Cursor = Cursors.Hand;
		base.MouseEnter += delegate
		{
			if (base.IsEnabled && !_checked)
			{
				_box.BorderBrush = Ui.B("Accent");
			}
		};
		base.MouseLeave += delegate
		{
			Paint();
		};
		base.MouseLeftButtonUp += delegate
		{
			if (base.IsEnabled)
			{
				_checked = !_checked;
				Paint();
				this.Changed?.Invoke(_checked);
			}
		};
		base.IsEnabledChanged += delegate
		{
			base.Opacity = (base.IsEnabled ? 1.0 : 0.5);
		};
		Paint();
	}

	public void Set(bool value)
	{
		_checked = value;
		Paint();
	}

	public void Hint(string text)
	{
		_hint.Text = text ?? "";
		_hint.Visibility = (string.IsNullOrEmpty(text) ? Visibility.Collapsed : Visibility.Visible);
	}

	private void Paint()
	{
		_box.Background = (_checked ? Ui.B("Accent") : Ui.B("Panel"));
		_box.BorderBrush = (_checked ? Ui.B("Accent") : Ui.B("Dim"));
		_mark.Foreground = Ui.B("Ink");
		_mark.Visibility = ((!_checked) ? Visibility.Hidden : Visibility.Visible);
	}
}
