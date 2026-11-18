using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;

namespace DomainScheduler.App;

internal sealed class QuickPage : Page
{
	private StackPanel _map;

	private TextBlock _game;

	private TextBlock _bg;

	private TextBlock _undo;

	private TextBlock _layer;

	private TextBlock _p99;

	private TextBlock _before;

	private TextBlock _low1;

	private TextBlock _low01;

	private TextBlock _frameNote;

	private FrameChart _chart;

	private Topo _topo;

	private string _mapKey = "";

	private readonly List<(FrameworkElement Bar, List<int> Lps, double Last)> _bars = new List<(FrameworkElement, List<int>, double)>();

	public override string Title => L.T("Overview");

	public QuickPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	private static TextBlock Big()
	{
		return new TextBlock
		{
			FontFamily = Ui.Mono,
			FontSize = Ui.FsBody * 1.6,
			Foreground = Ui.B("Strong"),
			HorizontalAlignment = HorizontalAlignment.Right
		};
	}

	private static UIElement Read(string caption, TextBlock value)
	{
		TextBlock textBlock = Ui.Text(caption, Ui.FsSmall, "Dim");
		textBlock.HorizontalAlignment = HorizontalAlignment.Right;
		StackPanel stackPanel = Ui.Col(textBlock, value);
		stackPanel.Margin = new Thickness(26.0, 0.0, 0.0, 0.0);
		return stackPanel;
	}

	private static UIElement Column(string head, TextBlock body)
	{
		TextBlock textBlock = Ui.Text(head, 0.0, "Strong");
		textBlock.FontWeight = FontWeights.Medium;
		textBlock.Margin = new Thickness(0.0, 0.0, 0.0, 3.0);
		body.LineHeight = Ui.FsNote * 1.5;
		return Ui.Col(textBlock, body);
	}

	public override FrameworkElement Build()
	{
		_topo = TopoBuilder.Live();
		_mapKey = "";
		TextBlock textBlock = new TextBlock
		{
			Text = L.T("Processor map"),
			FontSize = Ui.FsBody * 1.3,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong")
		};
		TextBlock textBlock2 = Ui.Text(L.T("Where the game's threads and the background programs are placed now"), Ui.FsNote, "Dim");
		textBlock2.Margin = new Thickness(0.0, 2.0, 0.0, 14.0);
		_map = new StackPanel();
		WrapPanel wrapPanel = new WrapPanel
		{
			Margin = new Thickness(0.0, 10.0, 0.0, 12.0)
		};
		wrapPanel.Children.Add(new Leg
		{
			C = Ui.B("Accent"),
			Text = L.T("Key thread of the game")
		}.Build());
		wrapPanel.Children.Add(new Leg
		{
			C = Ui.B("Steel"),
			Text = L.T("Worker thread")
		}.Build());
		wrapPanel.Children.Add(Ui.Text(L.T("Bar under a core = its load"), Ui.FsNote, "Dim"));
		_game = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		_bg = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		_undo = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		UniformGrid uniformGrid = new UniformGrid
		{
			Columns = 3
		};
		uniformGrid.Children.Add(Column(L.T("Game"), _game));
		uniformGrid.Children.Add(Column(L.T("Background"), _bg));
		uniformGrid.Children.Add(Column(L.T("Undo"), _undo));
		_layer = Ui.Note("");
		StackPanel stackPanel = Ui.Row(Ui.Btn(L.T("Boost"), Boost), Ui.Btn(L.T("Reapply"), Reapply), Ui.Btn(L.T("Resync"), Resync), Ui.Btn(L.T("Revert"), Revert));
		stackPanel.Margin = new Thickness(0.0, 0.0, 0.0, 6.0);
		_p99 = Big();
		_p99.Foreground = Ui.B("Accent");
		_before = Big();
		_low1 = Big();
		_low01 = Big();
		StackPanel stackPanel2 = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			HorizontalAlignment = HorizontalAlignment.Right
		};
		stackPanel2.Children.Add(Read(L.T("Worst frames now (p99), ms"), _p99));
		stackPanel2.Children.Add(Read(L.T("Before, ms"), _before));
		stackPanel2.Children.Add(Read(L.T("1% low, FPS"), _low1));
		stackPanel2.Children.Add(Read(L.T("0.1% low, FPS"), _low01));
		TextBlock textBlock3 = Ui.Text(L.T("Frame time, ms"), 0.0, "Strong");
		textBlock3.FontWeight = FontWeights.Medium;
		_frameNote = Ui.Text("", Ui.FsSmall, "Dim");
		StackPanel element = Ui.Col(textBlock3, _frameNote);
		DockPanel dockPanel = new DockPanel
		{
			Margin = new Thickness(0.0, 0.0, 0.0, 10.0)
		};
		DockPanel.SetDock(stackPanel2, Dock.Right);
		dockPanel.Children.Add(stackPanel2);
		dockPanel.Children.Add(element);
		_chart = new FrameChart
		{
			Height = 96.0
		};
		StackPanel child = Ui.Col(textBlock, textBlock2, _map, wrapPanel, Ui.Card(uniformGrid), Ui.Card(Ui.Col(dockPanel, _chart)), stackPanel, _layer);
		Tick();
		return Ui.Scroll(child);
	}

	private static string RoleText(ThreadRole r)
	{
		return r switch
		{
			ThreadRole.Main => L.T("Main"), 
			ThreadRole.Render => L.T("Render"), 
			ThreadRole.Audio => L.T("Audio"), 
			_ => L.T("Worker"), 
		};
	}

	private void DrawMap()
	{
		if (_topo == null)
		{
			if (_map.Children.Count == 0)
			{
				_map.Children.Add(Ui.Text(L.T("CPU: the topology could not be read"), 0.0, "Warn"));
			}
			return;
		}
		AutoController auto = H.Auto;
		bool flag = auto.State == AutoState.Active;
		ulong mask = auto.MaskNow;
		Dictionary<uint, ThreadRole> roles = new Dictionary<uint, ThreadRole>();
		if (flag)
		{
			if (auto.Top5Mode)
			{
				foreach (Top5Slot item in auto.Dynamic.View())
				{
					roles[item.LP] = item.Role;
				}
			}
			else
			{
				foreach (var item2 in from p in auto.PlanView()
					where p.Pinned
					select p)
				{
					roles[item2.LP] = item2.Role;
				}
			}
		}
		double[] load = H.CpuLoad;
		string text = $"{flag}|{mask}|{string.Join(",", from r in roles
			orderby r.Key
			select r.Key + ":" + (int)r.Value)}";
		if (text == _mapKey)
		{
			for (int i = 0; i < _bars.Count; i++)
			{
				(FrameworkElement, List<int>, double) tuple = _bars[i];
				double num = (from lp in tuple.Item2
					where lp < load.Length
					select load[lp]).DefaultIfEmpty(0.0).Average();
				if (!(Math.Abs(num - tuple.Item3) < 0.02))
				{
					Ui.SetBar(tuple.Item1, num);
					_bars[i] = (tuple.Item1, tuple.Item2, num);
				}
			}
			return;
		}
		_mapKey = text;
		_bars.Clear();
		_map.Children.Clear();
		byte b = _topo.FirstCore();
		List<IGrouping<byte, CpuSet>> list = (from s in _topo.Sets
			where s.Group == 0
			group s by s.LLC into g
			orderby g.Key
			select g).ToList();
		UniformGrid uniformGrid = new UniformGrid
		{
			Columns = Math.Min(2, Math.Max(1, list.Count))
		};
		int num2 = 0;
		foreach (IGrouping<byte, CpuSet> item3 in list)
		{
			ulong num3 = TopoBuilder.Mask(item3);
			bool flag2 = flag && mask != 0L && (num3 & mask) != 0;
			bool flag3 = mask == 0L || (mask & _topo.FullMask()) == _topo.FullMask();
			ulong value;
			ulong num4 = (_topo.L3ByLlc.TryGetValue(item3.Key, out value) ? value : 0);
			string s2 = L.F("Cache domain {0}", item3.Key) + ((!flag || flag3) ? "" : (flag2 ? (" — " + L.T("game")) : (" — " + L.T("background"))));
			string s3 = ((num4 != 0) ? ((num4 >> 20).ToString(CultureInfo.InvariantCulture) + " MB L3") : "") + ((_topo.BigCacheLlc == item3.Key) ? (" (" + L.T("big cache") + ")") : "");
			DockPanel dockPanel = new DockPanel
			{
				Margin = new Thickness(0.0, 0.0, 0.0, 10.0)
			};
			TextBlock element = Ui.Text(s3, Ui.FsSmall, "Dim");
			DockPanel.SetDock(element, Dock.Right);
			dockPanel.Children.Add(element);
			TextBlock textBlock = Ui.Text(s2, 0.0, "Strong");
			textBlock.FontWeight = FontWeights.Medium;
			dockPanel.Children.Add(textBlock);
			List<IGrouping<byte, CpuSet>> list2 = (from s in item3
				group s by s.Core into g
				orderby g.Key
				select g).ToList();
			UniformGrid uniformGrid2 = new UniformGrid
			{
				Columns = ((list.Count == 1) ? Math.Min(8, Math.Max(4, list2.Count)) : 4)
			};
			foreach (IGrouping<byte, CpuSet> item4 in list2)
			{
				List<CpuSet> list3 = item4.OrderBy((CpuSet s) => s.LP).ToList();
				List<ThreadRole> list4 = (from s in list3
					where roles.ContainsKey(s.LP)
					select roles[s.LP]).ToList();
				bool flag4 = list3.Any((CpuSet s) => s.LP < 64 && ((mask >> (int)s.LP) & 1) != 0);
				double num5 = (from s in list3
					where s.LP < load.Length
					select load[s.LP]).DefaultIfEmpty(0.0).Average();
				string text2;
				if (list3.Count <= 1)
				{
					text2 = list3[0].LP.ToString(CultureInfo.InvariantCulture);
				}
				else
				{
					text2 = list3[0].LP + "–" + list3[list3.Count - 1].LP;
				}
				string text3 = text2;
				DockPanel dockPanel2 = new DockPanel
				{
					LastChildFill = false
				};
				TextBlock element2 = Ui.Text(L.F("Core {0} · {1}", item4.Key, text3), Ui.FsSmall * 0.9, "Faint");
				DockPanel.SetDock(element2, Dock.Top);
				dockPanel2.Children.Add(element2);
				UIElement element3;
				if (list4.Count > 0)
				{
					ThreadRole threadRole = list4.OrderBy((ThreadRole r) => (int)r).First();
					string text4 = RoleText(threadRole);
					bool accent = (uint)threadRole <= 2u;
					Border border = Ui.Pill(text4, accent);
					border.Margin = new Thickness(0.0, 6.0, 0.0, 0.0);
					element3 = border;
				}
				else
				{
					TextBlock textBlock2 = Ui.Text((!flag) ? "" : ((item4.Key == b && (flag4 || flag3)) ? L.T("Left to Windows") : ((flag3 || flag4) ? L.T("Free") : L.T("Background"))), Ui.FsSmall, "Faint", bold: false, wrap: true);
					textBlock2.Margin = new Thickness(0.0, 6.0, 0.0, 0.0);
					element3 = textBlock2;
				}
				DockPanel.SetDock(element3, Dock.Top);
				dockPanel2.Children.Add(element3);
				FrameworkElement frameworkElement = Ui.Bar(num5, list4.Count > 0);
				_bars.Add((frameworkElement, ((IEnumerable<CpuSet>)list3).Select((Func<CpuSet, int>)((CpuSet s) => s.LP)).ToList(), num5));
				DockPanel.SetDock(frameworkElement, Dock.Bottom);
				dockPanel2.Children.Add(frameworkElement);
				uniformGrid2.Children.Add(new Border
				{
					Height = 78.0,
					Margin = new Thickness(0.0, 0.0, 7.0, 7.0),
					Padding = new Thickness(9.0, 8.0, 9.0, 9.0),
					CornerRadius = new CornerRadius(3.0),
					Background = Ui.B("Well"),
					BorderThickness = new Thickness(1.0),
					BorderBrush = Ui.B((list4.Count > 0) ? "AccentLine" : "Line"),
					Child = dockPanel2
				});
			}
			Border border2 = new Border();
			border2.CornerRadius = new CornerRadius(3.0);
			border2.Padding = new Thickness(14.0, 13.0, 7.0, 7.0);
			border2.Margin = new Thickness((num2 % 2 != 0) ? 7 : 0, 0.0, (num2 % 2 == 0 && list.Count > 1) ? 7 : 0, 0.0);
			border2.Background = Ui.B((flag2 && !flag3) ? "AccentSoft" : "Panel");
			border2.BorderThickness = new Thickness(1.0);
			border2.BorderBrush = Ui.B((flag2 && !flag3) ? "AccentLine" : "Line");
			border2.Child = Ui.Col(dockPanel, uniformGrid2);
			Border element4 = border2;
			uniformGrid.Children.Add(element4);
			num2++;
		}
		_map.Children.Add(uniformGrid);
	}

	private static double Percentile(List<double> sorted, double p)
	{
		if (sorted.Count != 0)
		{
			return sorted[Math.Min(sorted.Count - 1, Math.Max(0, (int)Math.Ceiling((double)sorted.Count * p) - 1))];
		}
		return 0.0;
	}

	private void DrawFrames()
	{
		AutoController auto = H.Auto;
		bool num = auto.State == AutoState.Active;
		string text = "—";
		if (!num || H.Frames == null || !H.Frames.Available)
		{
			TextBlock p = _p99;
			TextBlock before = _before;
			TextBlock low = _low1;
			string text3 = (_low01.Text = text);
			string text5 = (low.Text = text3);
			string text7 = (before.Text = text5);
			p.Text = text7;
			_frameNote.Text = ((H.Frames != null && !H.Frames.Available) ? L.F("Not measured: {0}", H.Frames.Error) : L.T("Shown while a game is active"));
			_chart.Set(new List<double>(), 0.0, -1.0, "", "");
			return;
		}
		DateTime utcNow = DateTime.UtcNow;
		List<double> list = H.Frames.Intervals(utcNow.AddSeconds(-60.0), utcNow);
		List<double> list2 = (from x in H.Frames.Intervals(utcNow.AddSeconds(-10.0), utcNow)
			orderby x
			select x).ToList();
		List<double> list3 = list.OrderBy((double x) => x).ToList();
		double baselineP = auto.BaselineP99;
		double appliedSecondsAgo = ((auto.AppliedAt == default(DateTime)) ? (-1.0) : (utcNow - auto.AppliedAt).TotalSeconds);
		_p99.Text = ((list2.Count >= 50) ? Percentile(list2, 0.99).ToString("0.0", CultureInfo.CurrentCulture) : text);
		_before.Text = ((baselineP > 0.0) ? baselineP.ToString("0.0", CultureInfo.CurrentCulture) : text);
		_low1.Text = ((list3.Count >= 200) ? (1000.0 / Percentile(list3, 0.99)).ToString("0", CultureInfo.CurrentCulture) : text);
		_low01.Text = ((list3.Count >= 2000) ? (1000.0 / Percentile(list3, 0.999)).ToString("0", CultureInfo.CurrentCulture) : text);
		_frameNote.Text = ((list.Count == 0) ? L.T("The game sends no frames to measure (it is not in front, or it does not use DXGI)") : L.T("Last 60 seconds · lower and flatter is better"));
		_chart.Set(list, baselineP, appliedSecondsAgo, L.T("Turned on here"), L.T("worst frames before (p99)"));
	}

	public override void Tick()
	{
		if (_map != null)
		{
			AutoController auto = H.Auto;
			DrawMap();
			DrawFrames();
			bool flag = auto.State == AutoState.Active;
			ulong maskNow = auto.MaskNow;
			_game.Text = (flag ? (L.T("Priority class: Above normal") + "\n" + ((maskNow != 0L) ? L.F("Cores: processors {0}", Masks.Ranges(maskNow)) : L.T("Cores: not limited"))) : L.T("No game is handled now"));
			int backgroundCount = H.Engine.BackgroundCount;
			_bg.Text = ((backgroundCount > 0) ? L.F("{0} programs: lower priority, I/O and memory priority, EcoQoS", backgroundCount) : L.T("Nothing is lowered now"));
			_undo.Text = L.T("Everything returns when the game closes, on Turn off, on Undo everything, or on exit");
			_layer.Text = (H.Paused ? L.T("Presets are paused after Revert: press Reapply to resume.") : L.F("Presets: {0} processes changed, re-applied every {1} s.", H.Engine.ChangedPids().Count, auto.On ? 5 : H.ReapplyTicks));
		}
	}

	private (uint Pid, string Exe) Game()
	{
		if (H.Auto.GamePid != 0)
		{
			return (Pid: H.Auto.GamePid, Exe: H.Auto.GameExe);
		}
		foreach (var (item, tuple2) in H.Engine.ProcessesNow())
		{
			if (H.Watch.Contains(tuple2.Item1))
			{
				return (Pid: item, Exe: tuple2.Item1);
			}
		}
		return (Pid: 0u, Exe: "");
	}

	private void Boost()
	{
		H.Opt.Run(delegate
		{
			List<string> list = new List<string>();
			var (num, text) = Game();
			if (num == 0)
			{
				list.Add(L.T("Boost: no running process of the watchlist"));
				return list;
			}
			string text2 = H.Eligibility(text);
			if (text2 != null)
			{
				list.Add(text2);
				return list;
			}
			bool flag = !H.Engine.ReadProcess(num, text).BoostOff;
			if (H.Engine.ProcessBoost((int)num, flag, list))
			{
				list.Add(flag ? L.F("{0}: dynamic priority boost off", text) : L.F("{0}: dynamic priority boost on", text));
			}
			return list;
		}, delegate(List<string> log)
		{
			foreach (string item in log ?? new List<string>())
			{
				S.Did(item);
			}
		});
	}

	private void Reapply()
	{
		H.Paused = false;
		H.Opt.Run(delegate
		{
			H.Sys.Invalidate();
			return H.Sys.Pass(H.Auto.GamePid);
		}, delegate(int n)
		{
			S.Did(L.F("Reapplied: {0} processes", n));
		});
	}

	private void Resync()
	{
		H.Opt.Run(delegate
		{
			TopoBuilder.Invalidate();
			GameMask.InvalidateIrq();
			Tables.Reload();
			H.LoadPresets();
			H.Configure();
			return H.Presets.Count;
		}, delegate(int n)
		{
			S.Did(L.F("Resynced: {0} presets, watchlist {1}", n, string.Join(", ", H.Watch)));
		});
	}

	private void Revert()
	{
		H.Paused = true;
		H.Opt.Run(() => H.Auto.RevertEverything(L.T("Button")), delegate
		{
			S.Did(L.T("Everything was put back"));
		});
	}

}
