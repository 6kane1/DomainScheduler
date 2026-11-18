using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace DomainScheduler.App;

internal sealed class MasksPage : Page
{
	private string _spec = "auto";

	private string _irq = "Core";

	private bool _noSmt;

	private bool _noCore0;

	private bool _dirty;

	private bool _load;

	private ulong _add;

	private ulong _drop;

	private StackPanel _view;

	private Button _apply;

	private Topo _topo;

	public override string Title => L.T("Masks");

	public MasksPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	private void LoadStored()
	{
		_spec = Settings.Get("MaskName");
		_irq = Settings.Get("IrqMode");
		_noSmt = Settings.GetBool("MaskNoSmt");
		_noCore0 = Settings.GetBool("MaskNoCore0");
		GameMask.Edits edits = Hub.MaskEdits();
		bool flag = _topo == null || edits.Sig != GameMask.CpuSig(_topo);
		_add = (flag ? 0 : edits.Add);
		_drop = (flag ? 0 : edits.Drop);
		_dirty = false;
	}

	private string SpecText()
	{
		return _spec + (_noSmt ? "+nosmt" : "") + (_noCore0 ? "+nocore0" : "");
	}

	private GameMaskInfo Info()
	{
		return GameMask.Build(_topo, SpecText(), new GameMask.Edits
		{
			Add = _add,
			Drop = _drop,
			Sig = GameMask.CpuSig(_topo)
		}, _irq);
	}

	public override FrameworkElement Build()
	{
		_topo = TopoBuilder.Live();
		_view = new StackPanel();
		if (_topo == null)
		{
			_view.Children.Add(Ui.Text(L.T("CPU: the topology could not be read"), 0.0, "Warn"));
			return _view;
		}
		LoadStored();
		Draw();
		return Ui.Scroll(_view);
	}

	private void Changed()
	{
		if (!_load)
		{
			_dirty = true;
			Draw();
		}
	}

	private void Draw()
	{
		_load = true;
		_view.Children.Clear();
		SplitPlan split = _topo.Split;
		GameMaskInfo info = Info();
		List<Opt> list = new List<Opt>
		{
			new Opt("auto", L.T("auto — chosen for this processor")),
			new Opt("all", L.T("all processors")),
			new Opt("game", L.T("game cores")),
			new Opt("nongame", L.T("non-game cores")),
			new Opt("vcache", L.T("big-cache cores")),
			new Opt("pcores", L.T("P-cores")),
			new Opt("ecores", L.T("E-cores"))
		};
		for (int i = 0; i < _topo.LLCGroups.Count; i++)
		{
			if (_topo.LLCGroups.Count <= 1)
			{
				break;
			}
			byte llc = _topo.LLCGroups[i];
			string text = L.F("L3 domain {0}", i) + " · " + Masks.Ranges(TopoBuilder.Mask(_topo.Sets.Where((CpuSet s) => s.LLC == llc)));
			if (_topo.BigCacheLlc == llc)
			{
				text = text + " · " + L.T("big cache");
			}
			list.Add(new Opt("ccd" + i.ToString(CultureInfo.InvariantCulture), text));
		}
		ComboBox comboBox = Combo.New(260.0, list.ToArray());
		if (!Combo.Select(comboBox, _spec))
		{
			Combo.Select(comboBox, "auto");
		}
		Combo.OnChange(comboBox, delegate(string k)
		{
			_spec = k;
			_add = (_drop = 0uL);
			Changed();
		});
		ComboBox comboBox2 = Combo.New(260.0, new Opt("Off", L.T("IRQ: do not refine")), new Opt("Thread", L.T("IRQ: exclude the pinned processors")), new Opt("Core", L.T("IRQ: exclude the whole pinned cores")));
		Combo.Select(comboBox2, _irq);
		Combo.OnChange(comboBox2, delegate(string k)
		{
			_irq = k;
			Changed();
		});
		CheckRow checkRow = new CheckRow(L.T("One processor per core (nosmt)"), L.T("Leaves the second SMT thread of every core out of the mask."), _noSmt);
		checkRow.Changed += delegate(bool v)
		{
			_noSmt = v;
			Changed();
		};
		CheckRow checkRow2 = new CheckRow(L.T("Without the first core (nocore0)"), L.T("Windows and drivers load the first core most."), _noCore0);
		checkRow2.Changed += delegate(bool v)
		{
			_noCore0 = v;
			Changed();
		};
		_apply = Ui.Btn(L.T("Apply"), Apply);
		_apply.IsEnabled = _dirty;
		Button button = Ui.Btn(L.T("Reset"), delegate
		{
			LoadStored();
			Draw();
		});
		button.IsEnabled = _dirty;
		_view.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("Mask of watched processes")), Ui.Note(L.T("Used by presets of watched processes that have no rule of their own. Auto picks its mask itself.")), Ui.Row(comboBox, comboBox2), checkRow, checkRow2, Ui.Row(_apply, button))));
		MaskResult maskResult = Masks.Resolve(_topo, SpecText());
		FactGrid factGrid = new FactGrid(1).Add(L.T("Split"), split.CanSplit ? (split.Label + " — " + L.F("game {0}, non-game {1}", Masks.Ranges(split.Game), Masks.Ranges(split.NonGame))) : split.Why()).Add(L.T("Mask"), Ui.Hex(info.Final) + "  " + Masks.Ranges(info.Final) + (info.Adjusted ? ("  · " + L.T("adjusted")) : "")).Add(L.T("Background"), Masks.Ranges(GameMask.Background(_topo, info)));
		StackPanel stackPanel = Ui.Col(Ui.Head(L.T("Result")), factGrid);
		foreach (string note in maskResult.Notes)
		{
			stackPanel.Children.Add(Ui.Text(Masks.NoteText(note), Ui.FsNote, "Warn", bold: false, wrap: true));
		}
		if (info.EditsStale)
		{
			stackPanel.Children.Add(Ui.Text(L.T("The manual edits were made on another processor and are dropped."), Ui.FsNote, "Warn", bold: false, wrap: true));
		}
		foreach (string note2 in info.Notes)
		{
			stackPanel.Children.Add(Ui.Note((note2 == "wide-irq: partial") ? L.T("wide-irq: partial — a device pinned to several cores takes out only its own processors, not whole cores") : note2));
		}
		if (info.IrqIgnored)
		{
			stackPanel.Children.Add(Ui.Note(L.T("IRQ refinement ignored: it would remove more than half of the mask.")));
		}
		foreach (KeyValuePair<string, ulong> irqDevice in info.IrqDevices)
		{
			stackPanel.Children.Add(Ui.Note(L.F("{0}: processors {1}", irqDevice.Key, Masks.Ranges(irqDevice.Value))));
		}
		foreach (KeyValuePair<string, ulong> item3 in info.IrqWide)
		{
			stackPanel.Children.Add(Ui.Note(L.F("{0}: processors {1} (several cores, not excluded)", item3.Key, Masks.Ranges(item3.Value))));
		}
		bool canSplit = split.CanSplit;
		stackPanel.Children.Add(Ui.LpMap(_topo, delegate(CpuSet lp)
		{
			ulong num2 = (ulong)(1L << (int)lp.LP);
			Brush item = ((!split.CanSplit) ? Ui.B("Panel2") : (((split.Game & num2) != 0L) ? Ui.GameC : Ui.NonGameC));
			SolidColorBrush item2 = (((info.IrqDrop & num2) != 0L) ? Ui.B("Irq") : (((info.Final & num2) != 0L) ? Ui.B("Text") : null));
			return ((Brush Fill, Brush Ring, string Tip))(Fill: item, Ring: item2, Tip: null);
		}, canSplit ? ((Action<CpuSet>)delegate(CpuSet lp)
		{
			ulong num = (ulong)(1L << (int)lp.LP);
			if ((info.Base & num) != 0L)
			{
				_drop ^= num;
			}
			else
			{
				_add ^= num;
			}
			Changed();
		}) : null));
		List<Leg> list2 = new List<Leg>();
		if (split.CanSplit)
		{
			string text2 = ((split.Family == "x3d") ? (" (" + (split.UpperHalf ? L.T("big cache, upper half of the numbers") : L.T("big cache, lower half of the numbers")) + ")") : "");
			list2.Add(new Leg
			{
				C = Ui.GameC,
				Text = L.T("game cores") + text2
			});
			list2.Add(new Leg
			{
				C = Ui.NonGameC,
				Text = L.T("non-game cores")
			});
		}
		list2.Add(new Leg
		{
			C = Ui.B("Text"),
			Look = "ring",
			Text = L.T("in the mask")
		});
		list2.Add(new Leg
		{
			C = Ui.B("Irq"),
			Look = "ring",
			Text = L.T("excluded for IRQ")
		});
		stackPanel.Children.Add(Leg.Line(list2.ToArray()));
		stackPanel.Children.Add(Ui.Note(canSplit ? L.T("Click a processor to add it to the mask or take it out.") : split.Why()));
		_view.Children.Add(Ui.Card(stackPanel));
		_view.Children.Add(Ui.Card(Reservation_()));
		_load = false;
	}

	private void Apply()
	{
		Settings.Set("MaskName", _spec);
		Settings.Set("IrqMode", _irq);
		Settings.SetBool("MaskNoSmt", _noSmt);
		Settings.SetBool("MaskNoCore0", _noCore0);
		Settings.Set("MaskAdd", _add.ToString("X", CultureInfo.InvariantCulture));
		Settings.Set("MaskDrop", _drop.ToString("X", CultureInfo.InvariantCulture));
		Settings.Set("MaskSig", GameMask.CpuSig(_topo));
		H.Configure();
		_dirty = false;
		GameMaskInfo gameMaskInfo = Info();
		S.Did(L.F("Mask saved: {0} → {1} ({2})", SpecText(), Ui.Hex(gameMaskInfo.Final), Masks.Ranges(gameMaskInfo.Final)));
		Draw();
	}

	private UIElement Reservation_()
	{
		Reservation.State state = Reservation.Read();
		SplitPlan split = _topo.Split;
		StackPanel stackPanel = Ui.Col(Ui.Head(L.T("Core reservation")), Ui.Note(L.T("Windows keeps reserved cores free of ordinary programs. Works after a reboot; only the non-game cores can be reserved.")));
		string v = ((state.Kind == "error") ? L.F("cannot be read ({0})", state.Error) : ((state.Mask == 0L) ? L.T("not set") : (L.F("processors {0}", Masks.Ranges(state.Mask)) + (state.Pending ? (" · " + L.T("waits for a reboot")) : ""))));
		stackPanel.Children.Add(Ui.Pair(L.T("Now"), v));
		List<string> list = Reservation.Refusals(_topo, split.NonGame);
		if (list.Count > 0)
		{
			stackPanel.Children.Add(Ui.Note(L.F("Cannot be reserved here: {0}", string.Join("; ", list))));
		}
		Button button = Ui.ConfirmBtn(L.T("Reserve the non-game cores"), delegate
		{
			S.BusyTask(L.T("Writing…"), () => Reservation.ApplyNonGame(_topo), delegate(string err)
			{
				S.Did((err == null) ? L.T("Reservation written and read back: it works after a reboot") : L.F("Reservation: {0}", err), (err != null) ? AppLog.Sev.Error : AppLog.Sev.Info);
				Draw();
			});
		});
		button.IsEnabled = list.Count == 0 && !S.AutoOn;
		Button button2 = Ui.ConfirmBtn(L.T("Remove the reservation"), delegate
		{
			S.BusyTask(L.T("Writing…"), Reservation.Revert, delegate(string err)
			{
				S.Did((err == null) ? L.T("Reservation removed: the original value is back") : L.F("Reservation: {0}", err), (err != null) ? AppLog.Sev.Warn : AppLog.Sev.Info);
				Draw();
			});
		});
		button2.IsEnabled = state.HasBackup && !S.AutoOn;
		stackPanel.Children.Add(Ui.Row(button, button2));
		return stackPanel;
	}
}
