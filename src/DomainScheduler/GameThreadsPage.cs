using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;

namespace DomainScheduler.App;

internal sealed class GameThreadsPage : Page
{
	private TextBlock _sub;

	private TextBlock _moves;

	private TextBlock _how;

	private StackPanel _rows;

	private StackPanel _log;

	private Button _role;

	private Button _top5;

	private Grid _headRow;

	private bool _busy;

	private string _key = "";

	private static readonly (double W, bool Star)[] Cols = new(double, bool)[5]
	{
		(52.0, false),
		(1.0, true),
		(230.0, false),
		(112.0, false),
		(130.0, false)
	};

	public override string Title => L.T("Game threads");

	public GameThreadsPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	private static Grid Line(params UIElement[] cells)
	{
		Grid grid = new Grid();
		for (int i = 0; i < Cols.Length; i++)
		{
			grid.ColumnDefinitions.Add(new ColumnDefinition
			{
				Width = (Cols[i].Star ? new GridLength(1.0, GridUnitType.Star) : new GridLength(Cols[i].W))
			});
			if (i < cells.Length && cells[i] != null)
			{
				Grid.SetColumn(cells[i], i);
				grid.Children.Add(cells[i]);
			}
		}
		return grid;
	}

	public override FrameworkElement Build()
	{
		TextBlock textBlock = new TextBlock
		{
			Text = L.T("Key threads of the game"),
			FontSize = Ui.FsBody * 1.3,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong")
		};
		_sub = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		_sub.Margin = new Thickness(0.0, 2.0, 0.0, 0.0);
		_role = new Button
		{
			Content = L.T("Role plan"),
			Height = 30.0,
			Padding = new Thickness(14.0, 0.0, 14.0, 1.0),
			FontSize = Ui.FsBody
		};
		_top5 = new Button
		{
			Content = L.T("Dynamic Top-5"),
			Height = 30.0,
			Padding = new Thickness(14.0, 0.0, 14.0, 1.0),
			FontSize = Ui.FsBody,
			Margin = new Thickness(3.0, 0.0, 0.0, 0.0)
		};
		_role.Click += delegate
		{
			SetMode(top5: false);
		};
		_top5.Click += delegate
		{
			SetMode(top5: true);
		};
		Border element = new Border
		{
			CornerRadius = new CornerRadius(3.0),
			Padding = new Thickness(3.0),
			Background = Ui.B("Panel2"),
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(1.0),
			VerticalAlignment = VerticalAlignment.Top,
			Child = new StackPanel
			{
				Orientation = Orientation.Horizontal,
				Children = 
				{
					(UIElement)_role,
					(UIElement)_top5
				}
			}
		};
		DockPanel dockPanel = new DockPanel
		{
			Margin = new Thickness(0.0, 0.0, 0.0, 16.0)
		};
		DockPanel.SetDock(element, Dock.Right);
		dockPanel.Children.Add(element);
		dockPanel.Children.Add(Ui.Col(textBlock, _sub));
		_headRow = new Grid();
		_key = "";
		_rows = new StackPanel();
		_moves = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		_how = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		_log = new StackPanel();
		TextBlock textBlock2 = Ui.Text(L.T("Core 0, anti-cheat threads and every other thread are left as Windows set them. A preferred core is a hint, nothing is hard-pinned."), Ui.FsNote, "Dim", bold: false, wrap: true);
		UniformGrid uniformGrid = new UniformGrid
		{
			Columns = 3
		};
		uniformGrid.Children.Add(Column(L.T("How it works"), _how));
		uniformGrid.Children.Add(Column(L.T("What is protected"), textBlock2));
		uniformGrid.Children.Add(Column(L.T("Right now"), _moves));
		TextBlock[] array = new TextBlock[3] { _how, textBlock2, _moves };
		foreach (TextBlock obj in array)
		{
			obj.LineHeight = Ui.FsNote * 1.5;
			obj.Margin = new Thickness(0.0, 0.0, 14.0, 0.0);
		}
		TextBlock textBlock3 = Ui.Note(L.T("To compare the two modes, play the same scene with each and look at the 1% low / p99 frame time; a few percent of difference is noise."));
		StackPanel child = Ui.Col(dockPanel, _headRow, _rows, new Border
		{
			Height = 16.0
		}, Ui.Card(uniformGrid), Ui.Card(Ui.Col(Ui.Head(L.T("Last moves")), _log)), textBlock3);
		Tick();
		return Ui.Scroll(child);
		static UIElement Column(string head, UIElement body)
		{
			TextBlock textBlock4 = Ui.Text(head, 0.0, "Strong");
			textBlock4.FontWeight = FontWeights.Medium;
			textBlock4.Margin = new Thickness(0.0, 0.0, 12.0, 3.0);
			return Ui.Col(textBlock4, body);
		}
	}

	private void SetMode(bool top5)
	{
		if (!_busy && H.Auto.Top5Mode != top5)
		{
			_busy = true;
			Settings.SetBool("Top5On", top5);
			S.Did(top5 ? L.T("Dynamic Top-5: on") : L.T("Dynamic Top-5: off"));
			H.Opt.Run(delegate
			{
				H.Auto.SetTop5(top5);
				return top5;
			}, delegate
			{
				_busy = false;
				Tick();
			});
		}
	}

	private static string RoleText(ThreadRole r)
	{
		return r switch
		{
			ThreadRole.Main => L.T("Main thread"), 
			ThreadRole.Render => L.T("Render thread"), 
			ThreadRole.Audio => L.T("Audio thread"), 
			ThreadRole.Worker => L.T("Worker"), 
			_ => L.T("Other thread"), 
		};
	}

	private static string PrioText(int? p)
	{
		return p switch
		{
			2 => L.T("Highest"), 
			1 => L.T("Above normal"), 
			0 => L.T("Normal"), 
			null => L.T("Unchanged"), 
			_ => p.Value.ToString("+0;-0", CultureInfo.InvariantCulture), 
		};
	}

	private UIElement Row(int number, bool lead, string name, string detail, double? share, string core, string prio, bool prioDim)
	{
		Border border = new Border
		{
			Width = 24.0,
			Height = 24.0,
			CornerRadius = new CornerRadius(3.0),
			BorderThickness = new Thickness(1.0),
			HorizontalAlignment = HorizontalAlignment.Left,
			VerticalAlignment = VerticalAlignment.Center,
			Background = Ui.B(lead ? "AccentSoft" : "None"),
			BorderBrush = Ui.B(lead ? "AccentLine" : "Line"),
			Child = new TextBlock
			{
				Text = number.ToString(CultureInfo.InvariantCulture),
				FontFamily = Ui.Mono,
				FontSize = Ui.FsSmall,
				Foreground = Ui.B(lead ? "AccentText" : "Dim"),
				HorizontalAlignment = HorizontalAlignment.Center,
				VerticalAlignment = VerticalAlignment.Center
			}
		};
		TextBlock textBlock = Ui.Text(name, 0.0, "Strong");
		textBlock.FontWeight = FontWeights.Medium;
		StackPanel stackPanel = Ui.Col(textBlock, Ui.Text(detail, Ui.FsSmall, "Faint"));
		stackPanel.VerticalAlignment = VerticalAlignment.Center;
		UIElement uIElement = null;
		if (share.HasValue)
		{
			FrameworkElement frameworkElement = Ui.Bar(Math.Min(1.0, share.Value * 2.2), accent: true, 4.0);
			frameworkElement.Width = 150.0;
			frameworkElement.VerticalAlignment = VerticalAlignment.Center;
			TextBlock element = new TextBlock
			{
				Text = (share.Value * 100.0).ToString("0", CultureInfo.CurrentCulture) + "%",
				FontFamily = Ui.Mono,
				FontSize = Ui.FsNote,
				Foreground = Ui.B("Text"),
				Margin = new Thickness(10.0, 0.0, 0.0, 0.0),
				VerticalAlignment = VerticalAlignment.Center
			};
			uIElement = new StackPanel
			{
				Orientation = Orientation.Horizontal,
				VerticalAlignment = VerticalAlignment.Center,
				Children = 
				{
					(UIElement)frameworkElement,
					(UIElement)element
				}
			};
		}
		Border border2 = new Border
		{
			CornerRadius = new CornerRadius(3.0),
			Padding = new Thickness(10.0, 3.0, 10.0, 4.0),
			Background = Ui.B("Panel2"),
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(1.0),
			HorizontalAlignment = HorizontalAlignment.Left,
			VerticalAlignment = VerticalAlignment.Center,
			Child = new TextBlock
			{
				Text = core,
				FontFamily = Ui.Mono,
				FontSize = Ui.FsNote,
				Foreground = Ui.B("Strong")
			}
		};
		TextBlock textBlock2 = Ui.Text(prio, 0.0, prioDim ? "Faint" : "Text");
		Border border3 = new Border();
		border3.BorderBrush = Ui.B("Line");
		border3.BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0);
		border3.Padding = new Thickness(10.0, 12.0, 10.0, 12.0);
		border3.Child = Line(border, stackPanel, uIElement, border2, textBlock2);
		return border3;
	}

	public override void Tick()
	{
		if (_rows == null)
		{
			return;
		}
		AutoController auto = H.Auto;
		bool top5Mode = auto.Top5Mode;
		Ui.Primary(_role, !top5Mode);
		Ui.Primary(_top5, top5Mode);
		_role.Background = Ui.B((!top5Mode) ? "Silver" : "None");
		_top5.Background = Ui.B(top5Mode ? "Silver" : "None");
		Button role = _role;
		Brush borderBrush = (_top5.BorderBrush = Ui.B("None"));
		role.BorderBrush = borderBrush;
		Button role2 = _role;
		bool isEnabled = (_top5.IsEnabled = !_busy);
		role2.IsEnabled = isEnabled;
		string text = $"{L.Lang}|{auto.State}|{top5Mode}|{auto.GameExe}|{auto.Blocked}|{auto.Confidence:0.00}|" + ((auto.State != AutoState.Active) ? "" : (top5Mode ? (auto.Dynamic.Moves + "|" + auto.Dynamic.Threads + "|" + string.Join(";", from s in auto.Dynamic.View()
			select $"{s.Index},{s.Tid},{s.LP},{s.Prio},{s.Share * 100.0:0}")) : string.Join(";", from p in auto.PlanView()
			select $"{p.Tid},{p.LP},{p.Pinned}")));
		if (text == _key)
		{
			return;
		}
		_key = text;
		_headRow.Children.Clear();
		_headRow.ColumnDefinitions.Clear();
		Grid child = Line(Ui.Text(top5Mode ? L.T("Slot") : L.T("No."), Ui.FsSmall, "Dim"), Ui.Text(L.T("Thread"), Ui.FsSmall, "Dim"), top5Mode ? Ui.Text(L.T("Share of the game's load"), Ui.FsSmall, "Dim") : null, Ui.Text(L.T("Preferred core"), Ui.FsSmall, "Dim"), Ui.Text(L.T("Priority"), Ui.FsSmall, "Dim"));
		_headRow.Children.Add(new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(10.0, 0.0, 10.0, 8.0),
			Child = child
		});
		_how.Text = (top5Mode ? L.T("The five busiest threads are re-checked every second and moved between cores and priorities when the load changes.") : L.T("The threads are recognized by their role (main, render, audio, workers) and placed once."));
		_rows.Children.Clear();
		_log.Children.Clear();
		string blocked = auto.Blocked;
		if (auto.State != AutoState.Active)
		{
			_sub.Text = L.T("Fully automatic. Nothing on this page needs to be set.");
			TextBlock textBlock = Ui.Text((auto.State != AutoState.Armed) ? L.T("Press Turn on (left) and start the game. The threads will appear here.") : ((blocked.Length > 0) ? blocked : L.T("Waiting for the game: start it and keep it in front for a few seconds.")), Ui.FsNote, (blocked.Length > 0) ? "Warn" : "Dim", bold: false, wrap: true);
			textBlock.Margin = new Thickness(10.0, 14.0, 0.0, 0.0);
			_rows.Children.Add(textBlock);
			_moves.Text = L.T("No game is handled now.");
			_log.Children.Add(Ui.Note(L.T("No moves yet.")));
			return;
		}
		_sub.Text = L.F("Fully automatic · {0}", auto.GameExe);
		if (top5Mode)
		{
			List<Top5Slot> list = auto.Dynamic.View();
			foreach (Top5Slot item2 in list)
			{
				_rows.Children.Add(Row(item2.Index + 1, item2.Index < 2, RoleText(item2.Role), item2.Name + " · " + item2.Tid.ToString(CultureInfo.InvariantCulture), item2.Share, "CPU " + item2.LP.ToString(CultureInfo.InvariantCulture), PrioText(item2.Prio), !item2.Prio.HasValue));
			}
			if (list.Count == 0)
			{
				_rows.Children.Add(new Border
				{
					Padding = new Thickness(10.0, 14.0, 0.0, 0.0),
					Child = Ui.Note(L.T("Measuring the threads…"))
				});
			}
			_moves.Text = L.F("{0} threads watched, {1} moves since the game started. No thread moves twice within 2 seconds.", auto.Dynamic.Threads, auto.Dynamic.Moves);
			foreach (string item3 in auto.Dynamic.Log())
			{
				_log.Children.Add(Ui.Text(item3, Ui.FsNote, "Dim"));
			}
			if (_log.Children.Count == 0)
			{
				_log.Children.Add(Ui.Note(L.T("No moves yet.")));
			}
			return;
		}
		List<(ThreadRole, uint, string, uint, bool)> list2 = auto.PlanView();
		int num = 1;
		foreach (var item4 in list2)
		{
			ThreadRole item = item4.Item1;
			isEnabled = (uint)item <= 2u;
			bool flag2 = isEnabled;
			_rows.Children.Add(Row(num++, flag2, RoleText(item4.Item1), item4.Item3 + " · " + item4.Item2.ToString(CultureInfo.InvariantCulture), null, item4.Item5 ? ("CPU " + item4.Item4.ToString(CultureInfo.InvariantCulture)) : "—", flag2 ? L.T("Above normal") : L.T("Unchanged"), !flag2));
		}
		if (list2.Count == 0)
		{
			_rows.Children.Add(new Border
			{
				Padding = new Thickness(10.0, 14.0, 0.0, 0.0),
				Child = Ui.Note(L.T("Measuring the threads…"))
			});
		}
		_moves.Text = ((list2.Count == 0) ? "" : (list2[0].Item5 ? L.F("{0} threads placed, confidence {1:0.00}.", list2.Count, auto.Confidence) : L.F("The threads are not recognized well enough (confidence {0:0.00}): only priorities and the mask are set. The dynamic mode does not need thread names.", auto.Confidence)));
		_log.Children.Add(Ui.Note(L.T("The role plan places the threads once and keeps them.")));
	}
}
