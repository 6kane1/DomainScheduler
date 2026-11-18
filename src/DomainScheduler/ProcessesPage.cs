using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace DomainScheduler.App;

internal sealed class ProcessesPage : Page
{
	private sealed class Row
	{
		public uint Pid;

		public string Exe = "";

		public string Path = "";

		public Engine.ProcInfo Info = new Engine.ProcInfo();

		public bool Watched;
	}

	private Table _table;

	private StackPanel _info;

	private Button _apply;

	private Button _reset;

	private List<Row> _rows = new List<Row>();

	private Row _sel;

	private ulong _edit;

	private ulong _loaded;

	private bool _dirty;

	private Topo _topo;

	public override string Title => L.T("Processes");

	public (uint Pid, string Exe) Picked
	{
		get
		{
			if (_sel != null)
			{
				return (Pid: _sel.Pid, Exe: _sel.Exe);
			}
			return (Pid: 0u, Exe: "");
		}
	}

	public ProcessesPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		_topo = TopoBuilder.Live();
		_table = new Table(("", 24.0), (L.T("Process"), 0.0), (L.T("PID"), 62.0), (L.T("Class"), 96.0), (L.T("Affinity"), 110.0), (L.T("Processors"), 96.0), (L.T("Watch"), 58.0));
		_table.SelectionChanged += delegate
		{
			_sel = _table.Selected as Row;
			LoadEdit();
			BuildInfo();
		};
		_info = new StackPanel();
		_apply = Ui.Btn(L.T("Apply"), Apply);
		_reset = Ui.Btn(L.T("Reset"), Reset);
		StackPanel element = Ui.Row(Ui.Btn(L.T("Refresh"), Refresh), _apply, _reset);
		Grid grid = new Grid();
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(318.0)
		});
		Border border = Ui.Card(Ui.Scroll(_info));
		border.Margin = new Thickness(12.0, 0.0, 0.0, 12.0);
		Grid.SetColumn(border, 1);
		grid.Children.Add(_table.Root);
		grid.Children.Add(border);
		DockPanel dockPanel = new DockPanel();
		DockPanel.SetDock(element, Dock.Top);
		dockPanel.Children.Add(element);
		dockPanel.Children.Add(grid);
		_sel = null;
		_dirty = false;
		BuildInfo();
		return dockPanel;
	}

	public override void Shown()
	{
		Refresh();
	}

	private static string ClassText(uint cls)
	{
		return cls switch
		{
			64u => L.T("Idle"), 
			16384u => L.T("Below normal"), 
			32u => L.T("Normal"), 
			32768u => L.T("Above normal"), 
			128u => L.T("High"), 
			256u => L.T("Realtime"), 
			_ => "", 
		};
	}

	private void Refresh()
	{
		S.Busy(L.T("Reading processes…"), delegate
		{
			List<Row> list2 = new List<Row>();
			foreach (var (num2, tuple2) in H.Engine.ProcessesNow())
			{
				if ((num2 != 0 && num2 != 4) || 1 == 0)
				{
					Row row = new Row
					{
						Pid = num2,
						Exe = tuple2.Item1,
						Watched = H.Watch.Contains(tuple2.Item1)
					};
					row.Info = H.Engine.ReadProcess(num2, tuple2.Item1);
					if (!row.Info.Denied)
					{
						row.Path = Native.ImagePath(num2);
					}
					list2.Add(row);
				}
			}
			return list2.OrderByDescending((Row r) => r.Watched).ThenBy<Row, string>((Row r) => r.Exe, StringComparer.OrdinalIgnoreCase).ThenBy((Row r) => r.Pid)
				.ToList();
		}, delegate(List<Row> list)
		{
			_rows = list ?? new List<Row>();
			int px = (int)Math.Round(14.72 * S.DpiScaleX);
			_table.SetRows(_rows.Select(delegate(Row r)
			{
				Image image = new Image
				{
					Width = 14.72,
					Height = 14.72
				};
				IconCache.Load(r.Path, Math.Max(16, px), image);
				string brush = (r.Info.Denied ? "Faint" : (r.Watched ? "Text" : "Dim"));
				return ((object, UIElement[]))(r, new UIElement[7]
				{
					image,
					Ui.Text(r.Exe + (r.Info.Changed ? "  •" : ""), 0.0, brush),
					Ui.Text(r.Pid.ToString(CultureInfo.InvariantCulture), Ui.FsNote, "Dim"),
					Ui.Text(r.Info.Denied ? L.T("denylist") : (r.Info.Ok ? ClassText(r.Info.Class) : ""), Ui.FsNote, "Dim"),
					Ui.Text(r.Info.Ok ? Ui.Hex(r.Info.Affinity) : "", Ui.FsNote, "Dim"),
					Ui.Text(r.Info.Ok ? Masks.Ranges(r.Info.Affinity) : "", Ui.FsNote, "Dim"),
					Ui.Text(r.Watched ? "✓" : "", 0.0, "On")
				});
			}));
			_sel = null;
			_dirty = false;
			BuildInfo();
		});
	}

	private void LoadEdit()
	{
		_loaded = ((_sel != null && _sel.Info.Ok && _topo != null) ? (_sel.Info.Affinity & _topo.FullMask()) : 0);
		_edit = _loaded;
		_dirty = false;
	}

	private void BuildInfo()
	{
		_info.Children.Clear();
		_apply.IsEnabled = false;
		_reset.IsEnabled = false;
		if (_sel == null)
		{
			_info.Children.Add(Ui.Note(L.T("Select a process. Only processes on the watchlist are changed; denylisted ones are never opened.")));
			return;
		}
		Row r = _sel;
		_info.Children.Add(Ui.Head(r.Exe));
		_info.Children.Add(Ui.Pair(L.T("PID"), r.Pid.ToString(CultureInfo.InvariantCulture)));
		if (r.Info.Denied)
		{
			_info.Children.Add(Ui.Note(L.T("On the denylist (system-critical or integrity-sensitive): never opened, never changed.")));
			return;
		}
		CheckRow checkRow = new CheckRow(L.T("On the watchlist"), L.T("The program changes scheduling only for watched processes."), r.Watched);
		checkRow.Changed += delegate(bool on)
		{
			H.SetWatched(r.Exe, on);
			foreach (Row item in _rows.Where((Row x) => x.Exe.Equals(r.Exe, StringComparison.OrdinalIgnoreCase)))
			{
				item.Watched = on;
			}
			BuildInfo();
			S.Refresh();
		};
		_info.Children.Add(checkRow);
		if (!r.Info.Ok || _topo == null)
		{
			_info.Children.Add(Ui.Note(L.T("The process cannot be read (no access).")));
			return;
		}
		SplitPlan split = _topo.Split;
		_info.Children.Add(Ui.Pair(L.T("Affinity"), Ui.Hex(_edit) + "  " + Masks.Ranges(_edit)));
		if (split.CanSplit)
		{
			_info.Children.Add(Ui.Pair(L.T("Check"), ((_edit & ~split.Game) == 0L) ? L.T("only game cores") : L.T("also non-game cores")));
		}
		_info.Children.Add(Ui.LpMap(_topo, (CpuSet lp) => ((Brush Fill, Brush Ring, string Tip))(Fill: (lp.LP >= 64 || ((_edit >> (int)lp.LP) & 1) == 0) ? Ui.B("Panel2") : ((split.CanSplit && ((split.Game >> (int)lp.LP) & 1) == 0L) ? Ui.NonGameC : Ui.GameC), Ring: null, Tip: null), r.Watched ? ((Action<CpuSet>)delegate(CpuSet lp)
		{
			ulong num = _edit ^ (ulong)(1L << (int)lp.LP);
			if (num != 0L)
			{
				_edit = num;
				_dirty = _edit != _loaded;
				BuildInfo();
			}
		}) : null, 24.0));
		if (split.CanSplit)
		{
			_info.Children.Add(Leg.Line(new Leg
			{
				C = Ui.GameC,
				Text = L.T("game cores")
			}, new Leg
			{
				C = Ui.NonGameC,
				Text = L.T("non-game cores")
			}));
		}
		_info.Children.Add(Ui.Note(r.Watched ? L.T("Click processors to edit the mask, then Apply. The mask is saved as this program's rule and kept while it runs.") : L.T("Put the process on the watchlist to edit its mask.")));
		string text = H.Eligibility(r.Exe);
		_apply.IsEnabled = _dirty && text == null && !S.AutoOn;
		_reset.IsEnabled = r.Watched && !S.AutoOn;
		if (S.AutoOn)
		{
			_info.Children.Add(Ui.Note(L.T("Auto is on: masks are set by Auto.")));
		}
	}

	private void Apply()
	{
		if (_sel != null && _dirty)
		{
			Row r = _sel;
			ulong mask = _edit;
			Settings.SaveAffRule(r.Exe, Ui.Hex(mask));
			H.Configure();
			H.Paused = false;
			H.Opt.Run(() => H.Sys.Pass(H.Auto.GamePid), delegate
			{
				S.Did(L.F("{0}: mask {1} ({2}) saved and applied", r.Exe, Ui.Hex(mask), Masks.Ranges(mask)));
				Refresh();
			});
		}
	}

	private void Reset()
	{
		if (_sel != null)
		{
			Row r = _sel;
			Settings.SaveAffRule(r.Exe, "");
			H.Configure();
			H.Opt.Run(() => H.Engine.Revert(r.Pid), delegate(int n)
			{
				S.Did(L.F("{0}: rule removed, {1} values put back", r.Exe, n));
				Refresh();
			});
		}
	}
}
