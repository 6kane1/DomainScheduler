using System;
using System.Collections.Generic;
using System.Linq;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal sealed class Table
{
	private readonly (string Head, double Width)[] _cols;

	private readonly StackPanel _rows = new StackPanel();

	private readonly List<(object Tag, Border Row)> _items = new List<(object, Border)>();

	public readonly DockPanel Root = new DockPanel();

	public object Selected { get; private set; }

	public int Count => _items.Count;

	public event Action SelectionChanged;

	public Table(params (string Head, double Width)[] cols)
	{
		_cols = cols;
		Grid child = MakeRow(((IEnumerable<(string, double)>)cols).Select((Func<(string, double), UIElement>)(((string Head, double Width) c) => Ui.Caps(c.Head, Ui.FsSmall * 0.95))).ToArray());
		Border element = new Border
		{
			Child = child,
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(8.0, 0.0, 16.0, 7.0)
		};
		DockPanel.SetDock(element, Dock.Top);
		Root.Children.Add(element);
		Root.Children.Add(Ui.Scroll(_rows));
	}

	private Grid MakeRow(UIElement[] cells)
	{
		Grid grid = new Grid();
		for (int i = 0; i < _cols.Length; i++)
		{
			grid.ColumnDefinitions.Add(new ColumnDefinition
			{
				Width = ((_cols[i].Width > 0.0) ? new GridLength(_cols[i].Width) : new GridLength(1.0, GridUnitType.Star))
			});
			if (i < cells.Length && cells[i] != null)
			{
				if (cells[i] is FrameworkElement frameworkElement && frameworkElement.Margin == default(Thickness))
				{
					frameworkElement.Margin = new Thickness(0.0, 0.0, 8.0, 0.0);
				}
				Grid.SetColumn(cells[i], i);
				grid.Children.Add(cells[i]);
			}
		}
		return grid;
	}

	public void SetRows(IEnumerable<(object Tag, UIElement[] Cells)> rows)
	{
		_rows.Children.Clear();
		_items.Clear();
		Selected = null;
		foreach (var row in rows)
		{
			object item = row.Tag;
			UIElement[] item2 = row.Cells;
			Border b = new Border
			{
				Child = MakeRow(item2),
				Padding = new Thickness(8.0, 6.0, 8.0, 6.0),
				Background = Ui.B("None"),
				CornerRadius = new CornerRadius(2.0),
				MinHeight = 30.0
			};
			object captured = item;
			b.MouseLeftButtonUp += delegate
			{
				Select(captured);
			};
			b.MouseEnter += delegate
			{
				if (!object.Equals(Selected, captured))
				{
					b.Background = Ui.B("Hover");
				}
			};
			b.MouseLeave += delegate
			{
				if (!object.Equals(Selected, captured))
				{
					b.Background = Ui.B("None");
				}
			};
			_items.Add((item, b));
			_rows.Children.Add(b);
		}
	}

	public void Select(object tag, bool raise = true)
	{
		Selected = tag;
		foreach (var item in _items)
		{
			var (objA, _) = item;
			item.Row.Background = (object.Equals(objA, tag) ? Ui.B("Sel") : Ui.B("None"));
		}
		if (raise)
		{
			this.SelectionChanged?.Invoke();
		}
	}
}
