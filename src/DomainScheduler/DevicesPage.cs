using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace DomainScheduler.App;

internal sealed class DevicesPage : Page
{
	private sealed class Row
	{
		public PciDevice Dev;

		public IrqConfig Cfg = new IrqConfig();
	}

	private Table _table;

	private StackPanel _info;

	private List<KeyValuePair<string, ulong>> _pins = new List<KeyValuePair<string, ulong>>();

	private Row _sel;

	private Topo _topo;

	private StackPanel _auto;

	public override string Title => L.T("Devices");

	public override string Section => "Hardware";

	public DevicesPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		_topo = TopoBuilder.Live();
		_sel = null;
		_table = new Table((L.T("Device"), 0.0), (L.T("MSI"), 46.0), (L.T("Pinned to"), 90.0), (L.T("Priority"), 70.0));
		_table.SelectionChanged += delegate
		{
			_sel = _table.Selected as Row;
			LoadFacts();
		};
		_info = new StackPanel();
		StackPanel element = Ui.Row(Ui.Btn(L.T("Refresh"), Refresh));
		Grid grid = new Grid();
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		grid.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(380.0)
		});
		Border border = Ui.Card(Ui.Scroll(_info));
		border.Margin = new Thickness(12.0, 0.0, 0.0, 12.0);
		Grid.SetColumn(border, 1);
		grid.Children.Add(_table.Root);
		grid.Children.Add(border);
		DockPanel dockPanel = new DockPanel();
		DockPanel.SetDock(element, Dock.Top);
		dockPanel.Children.Add(element);
		_auto = new StackPanel();
		Border border2 = Ui.Card(_auto);
		border2.Margin = new Thickness(0.0, 0.0, 0.0, 12.0);
		DockPanel.SetDock(border2, Dock.Top);
		dockPanel.Children.Add(border2);
		dockPanel.Children.Add(grid);
		_info.Children.Add(Ui.Note(L.T("Select a device. Interrupt pins are only read here; the game mask steps around dedicated ones.")));
		return dockPanel;
	}

	public override void Shown()
	{
		DrawAuto();
		Refresh();
	}

	private static string RoleText(IrqRole r)
	{
		return r switch
		{
			IrqRole.Mouse => L.T("Mouse"), 
			IrqRole.Controller => L.T("Game controller"), 
			IrqRole.Gpu => L.T("Graphics card"), 
			IrqRole.Net => L.T("Network"), 
			IrqRole.Keyboard => L.T("Keyboard"), 
			_ => "", 
		};
	}

	private void DrawAuto()
	{
		if (_auto == null)
		{
			return;
		}
		_auto.Children.Clear();
		_auto.Children.Add(Ui.Head(L.T("Interrupt layout")));
		// 1.16.0 Test 1.9: no big plan table on the page any more. One line on what it does; the plan itself is shown in
		// the confirmation of "Apply the layout", and nothing is offered when the layout is already in place.
		_auto.Children.Add(Ui.Note(L.T("The mouse, the graphics card and the network card get cores of their own. Works after a restart; every value is backed up and comes back by itself if Windows does not start.")));
		TextBlock wait = Ui.Note(L.T("Reading the devices…"));
		_auto.Children.Add(wait);
		Task.Run(delegate
		{
			try
			{
				return ((List<AutoDevice>, string))(IrqAuto.PlanNow(), null);
			}
			catch (Exception ex)
			{
				return ((List<AutoDevice>, string))(null, ex.Message);
			}
		}).ContinueWith(delegate(Task<(List<AutoDevice>, string)> t)
		{
			Task<(List<AutoDevice>, string)> t2 = t;
			return S.Dispatcher.BeginInvoke((Action)delegate
			{
				_auto.Children.Remove(wait);
				if (t2.Result.Item2 != null)
				{
					_auto.Children.Add(Ui.Note(L.F("Could not read the devices: {0}", t2.Result.Item2)));
				}
				else
				{
					RenderAuto(t2.Result.Item1);
				}
			});
		});
	}

	private static string KindText(AutoKind k) => k switch
	{
		AutoKind.Gpu => L.T("Graphics"),
		AutoKind.Audio => L.T("Audio"),
		AutoKind.Net => L.T("Network"),
		_ => L.T("USB")
	};

	/// <summary>The core a device of the plan gets, as written in the registry (0 = Windows decides).</summary>
	private static ulong Want(AutoDevice d) => d.Slot != null ? d.Slot.Mask : 0UL;

	/// <summary>One line of the confirmation: "Mouse · USB controller …: core 2".</summary>
	private static string PlanLine(AutoDevice d)
	{
		string role = d.Slot != null ? RoleText(d.Slot.Role) : KindText(d.Kind);
		string where = d.Slot != null && d.Slot.Mask != 0L ? L.F("processor {0}", Masks.Ranges(d.Slot.Mask))
			: d.Kind == AutoKind.Net && d.Wifi ? L.T("not touched") : L.T("Windows decides");
		string name = d.Name.Length > 48 ? d.Name[..47] + "…" : d.Name;
		return (role.Length > 0 ? role + " · " : "") + name + ":  " + where;
	}

	private void RenderAuto(List<AutoDevice> plan)
	{
		if (plan.Count == 0)
		{
			_auto.Children.Add(Ui.Note(L.T("No devices with interrupt settings were found.")));
			return;
		}
		var now = plan.Select(d => (Devices.ReadIrq(d.Id).Assign, Want(d))).ToList();
		bool inPlace = IrqAuto.HasBackup && IrqAuto.LayoutInPlace(now);
		int own = plan.Count(d => d.Slot != null && d.Slot.Mask != 0L);
		if (inPlace)
		{
			TextBlock ok = Ui.Text("✓  " + L.F("The layout is in place: {0} devices have cores of their own.", own), Ui.FsBody, "On", bold: false, wrap: true);
			ok.Margin = new Thickness(0.0, 6.0, 0.0, 6.0);
			_auto.Children.Add(ok);
		}
		if (!Flags.Open("devices-write"))
		{
			_auto.Children.Add(Ui.Row(Ui.ConfirmBtn(L.T("Allow device policy changes"), delegate
			{
				Flags.Set("devices-write", open: true);
				DrawAuto();
			})));
			return;
		}
		StackPanel row = Ui.Row();
		if (!inPlace)
		{
			row.Children.Add(Ui.Primary(Ui.Btn(L.T("Apply the layout"), delegate
			{
				string text = L.T("These devices get these processors:") + "\n\n" + string.Join("\n", plan.Select(PlanLine)) + "\n\n"
					+ L.T("Works after a restart of Windows. Every original value is backed up; if Windows does not start, everything is undone by itself.");
				Window owner = Window.GetWindow(_auto);
				MessageBoxResult r = owner != null
					? MessageBox.Show(owner, text, L.T("Apply the layout"), MessageBoxButton.OKCancel, MessageBoxImage.Question, MessageBoxResult.Cancel)
					: MessageBox.Show(text, L.T("Apply the layout"), MessageBoxButton.OKCancel, MessageBoxImage.Question, MessageBoxResult.Cancel);
				if (r != MessageBoxResult.OK)
				{
					return;
				}
				S.BusyTask(L.T("Writing…"), () => IrqAuto.Apply(plan), delegate(List<(string Text, bool Ok)> lines)
				{
					GameMask.InvalidateIrq();
					foreach (var (message, flag) in lines ?? new List<(string, bool)>())
					{
						S.Did(message, (!flag) ? AppLog.Sev.Error : AppLog.Sev.Info);
					}
					S.Did(L.T("Restart Windows: the new interrupt cores start working after the restart"));
					DrawAuto();
					Refresh();
				});
			}, 190.0)));
		}
		if (IrqAuto.HasBackup)
		{
			row.Children.Add(Ui.ConfirmBtn(L.T("Restore all devices"), delegate
			{
				S.BusyTask(L.T("Writing…"), Devices.RevertAll, delegate(string err)
				{
					GameMask.InvalidateIrq();
					S.Did((err == null) ? L.T("Device policies restored from the backup; restart Windows") : L.F("Devices: {0}", err), (err != null) ? AppLog.Sev.Error : AppLog.Sev.Info);
					DrawAuto();
					Refresh();
				});
			}));
		}
		if (row.Children.Count > 0)
		{
			_auto.Children.Add(row);
		}
	}

	private static string PrioText(int? p)
	{
		return p switch
		{
			1 => L.T("low"), 
			2 => L.T("normal"), 
			3 => L.T("high"), 
			_ => "", 
		};
	}

	private void Refresh()
	{
		S.BusyTask(L.T("Reading devices…"), delegate
		{
			List<Row> list = (from d in Devices.List()
				select new Row
				{
					Dev = d,
					Cfg = Devices.ReadIrq(d.Pdo)
				}).OrderBy<Row, string>((Row r) => r.Dev.Name, StringComparer.OrdinalIgnoreCase).ToList();
			List<KeyValuePair<string, ulong>> item = (from r in list
				where r.Cfg.Assign.GetValueOrDefault() != 0
				select new KeyValuePair<string, ulong>(r.Dev.Name, r.Cfg.Assign.Value)).ToList();
			return (rows: list, pins: item);
		}, delegate((List<Row> rows, List<KeyValuePair<string, ulong>> pins) res)
		{
			_pins = res.pins ?? new List<KeyValuePair<string, ulong>>();
			_sel = null;
			_table.SetRows((res.rows ?? new List<Row>()).Select((Row r) => ((object, UIElement[]))(r, new UIElement[4]
			{
				Ui.Text(r.Dev.Name),
				Ui.Text(r.Cfg.Msi ? "✓" : "", 0.0, "On"),
				Ui.Text((r.Cfg.Assign.GetValueOrDefault() != 0L) ? Masks.Ranges(r.Cfg.Assign.Value) : "", Ui.FsNote, "Dim"),
				Ui.Text(PrioText(r.Cfg.Priority), Ui.FsNote, "Dim")
			})));
		});
	}

	private void LoadFacts()
	{
		if (_sel == null)
		{
			return;
		}
		Row row = _sel;
		S.BusyTask(L.T("Reading the device…"), () => Devices.Facts(row.Dev, _pins), delegate(DeviceFacts f)
		{
			if (f != null && row == _sel)
			{
				Draw(row, f);
			}
		});
	}

	private void Draw(Row row, DeviceFacts f)
	{
		_info.Children.Clear();
		_info.Children.Add(Ui.Head(row.Dev.Name));
		FactGrid element = new FactGrid(1).Add(L.T("Driver"), f.DriverFile).Add(L.T("Location"), row.Dev.Location).Add(L.T("Windows started"), (f.Boot == DateTime.MinValue) ? "" : f.Boot.ToLocalTime().ToString("g", CultureInfo.CurrentCulture))
			.Add(L.T("Restarted by the program"), (f.Restarted == DateTime.MinValue) ? L.T("never") : f.Restarted.ToLocalTime().ToString("g", CultureInfo.CurrentCulture));
		_info.Children.Add(element);
		_info.Children.Add(Ui.Head(L.T("Interrupts")));
		if (f.Resources.Count == 0)
		{
			_info.Children.Add(Ui.Note(L.T("No interrupt resources.")));
		}
		foreach (IrqResource resource in f.Resources)
		{
			_info.Children.Add(Ui.Note(L.F("Vector {0} · affinity {1} · group {2} · {3} · {4}", resource.Vector, Ui.Hex(resource.Affinity), resource.Group, resource.Message ? "MSI" : L.T("line"), resource.Kind)));
		}
		if (_topo != null)
		{
			ulong mine = row.Cfg.Assign.GetValueOrDefault();
			ulong others = f.Others.Aggregate(0uL, (ulong a, KeyValuePair<string, ulong> p) => a | p.Value);
			_info.Children.Add(Ui.LpMap(_topo, delegate(CpuSet lp)
			{
				ulong num = (ulong)(1L << (int)lp.LP);
				return ((Brush Fill, Brush Ring, string Tip))(Fill: ((mine & num) != 0L) ? Ui.B("Irq") : Ui.B("Panel2"), Ring: ((others & num) != 0L) ? Ui.B("Dim") : null, Tip: null);
			}, null, 22.0));
			_info.Children.Add(Leg.Line(new Leg
			{
				C = Ui.B("Irq"),
				Text = L.T("this device")
			}, new Leg
			{
				C = Ui.B("Dim"),
				Look = "ring",
				Text = L.T("other devices")
			}));
		}
		if (_topo == null)
		{
			return;
		}
		_info.Children.Add(Ui.Head(L.T("Change the pin (advanced)")));
		if (!Flags.Open("devices-write"))
		{
			_info.Children.Add(Ui.Note(L.T("Writing device policies is switched off (feature flag devices-write).")));
			_info.Children.Add(Ui.Row(Ui.ConfirmBtn(L.T("Allow device policy changes"), delegate
			{
				Flags.Set("devices-write", open: true);
				Draw(row, f);
			})));
			return;
		}
		if (H.Auto.GamePid != 0)
		{
			_info.Children.Add(Ui.Note(L.T("Not while a game runs: the change restarts the device. Close the game and select the device again.")));
			return;
		}
		if (Devices.IsStorage(row.Dev))
		{
			_info.Children.Add(Ui.Note(L.T("Storage controllers are never changed: a wrong value there can stop Windows from starting")));
			return;
		}
		if (!row.Cfg.Msi)
		{
			_info.Children.Add(Ui.Note(L.T("Skipped: the device does not use MSI, a pin would not work reliably.")));
			return;
		}
		List<Opt> list = new List<Opt>
		{
			new Opt("", L.T("no pin"))
		};
		list.AddRange(from s in _topo.Sets
			where s.Group == 0
			orderby s.LP
			select new Opt(s.LP.ToString(CultureInfo.InvariantCulture), L.F("Processor {0}", s.LP)));
		ComboBox lp2 = Combo.New(150.0, list.ToArray());
		ComboBox prio = Combo.New(150.0, new Opt("", L.T("Priority: keep")), new Opt("1", L.T("low")), new Opt("2", L.T("normal")), new Opt("3", L.T("high")));
		_info.Children.Add(Ui.Row(lp2, prio));
		_info.Children.Add(Ui.Row(Ui.ConfirmBtn(L.T("Write and restart the device"), delegate
		{
			IrqConfig want = new IrqConfig
			{
				Msi = row.Cfg.Msi,
				MsgLimit = row.Cfg.MsgLimit,
				Priority = row.Cfg.Priority
			};
			if (int.TryParse(Combo.Key(lp2), out var result))
			{
				want.Assign = (ulong)(1L << result);
			}
			if (int.TryParse(Combo.Key(prio), out var result2))
			{
				want.Priority = result2;
			}
			S.BusyTask(L.T("Writing…"), () => Devices.Write(row.Dev, want), delegate(string err)
			{
				GameMask.InvalidateIrq();
				S.Did((err == null) ? L.F("{0}: policy written, read back, device restarted", row.Dev.Name) : L.F("{0}: {1}", row.Dev.Name, err), (err != null) ? AppLog.Sev.Error : AppLog.Sev.Info);
				Refresh();
			});
		}), Ui.ConfirmBtn(L.T("Restore all devices"), delegate
		{
			S.BusyTask(L.T("Writing…"), Devices.RevertAll, delegate(string err)
			{
				GameMask.InvalidateIrq();
				S.Did((err == null) ? L.T("Device policies restored from the backup") : L.F("Devices: {0}", err), (err != null) ? AppLog.Sev.Error : AppLog.Sev.Info);
				Refresh();
			});
		})));
		_info.Children.Add(Ui.Note(L.T("The device restarts for a moment (a network card drops the connection). The original values are backed up first and put back if the read-back differs.")));
	}
}
