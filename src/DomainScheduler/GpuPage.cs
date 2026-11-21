using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;

namespace DomainScheduler.App;

/// <summary>
/// The graphics card page in the HUD look (1.14.0). Same logic as 1.13.0 (feature flag, sliders that change nothing
/// until Apply, Apply with confirmation, Reset to stock, profiles with save/load/apply/rename/delete/at start, the
/// crash-guard warning, the values read back from the driver), laid out as readings tiles, an overclocking panel
/// with segmented sliders and step buttons, and the profiles as a table. 1.15.0 Test 1.5: AMD Radeon cards through
/// AmdOc (same sliders for the clock offsets, plus the power limit and the voltage offset instead of the voltage boost).
/// </summary>
internal sealed class GpuPage : Page
{
	private StackPanel _tiles;

	private StackPanel _info;

	private StackPanel _oc;

	private StackPanel _profiles;

	private Grid _main;

	private Border _ocBox;

	private StackPanel _side;

	private TextBlock _temp;

	private int _core;

	private int _mem;

	private int _volt;

	private List<NvClock> _clocks;

	private bool _loaded;

	private bool _hasVolt;

	/// <summary>"nvidia" or "amd": whose overclocking the page shows (NVIDIA first, as before 1.15.0 Test 1.5).</summary>
	private string _vendor = "";

	/// <summary>The AMD card's values as read (null on NVIDIA); the clocks are also in _clocks in the NVIDIA shape.</summary>
	private AmdState _amd;

	private int _power;

	private int _voltMv;

	private GpuRange _powerRange = new GpuRange(0, 0, 1);

	private GpuRange _voltMvRange = new GpuRange(0, 0, 5);

	private bool Amd => _vendor == GpuOc.Amd;

	private string OcError => GpuOc.Error(_vendor);

	private GpuProfiles _store = new GpuProfiles();

	private GpuRange _coreRange = GpuOcLogic.CoreFallback;

	private GpuRange _memRange = GpuOcLogic.MemFallback;

	private string _selected = "";

	/// <summary>Puts the current values into the sliders (after a profile was loaded).</summary>
	private readonly List<Action> _sync = new List<Action>();

	public override string Title => L.T("Graphics card");

	public override string Section => "Hardware";

	public GpuPage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		_tiles = new StackPanel();
		_info = new StackPanel();
		_oc = new StackPanel();
		_profiles = new StackPanel();
		_temp = null;
		_loaded = false;
		TextBlock about = Ui.Text(L.T("The card, its driver and the official download pages. Overclocking is an advanced function behind its own feature flag."), Ui.FsNote, "Dim", bold: false, wrap: true);
		about.Margin = new Thickness(0.0, 0.0, 0.0, 10.0);

		// Overclocking at the left; profiles and the card's driver at the right (under it when the page is narrow).
		_main = new Grid();
		_main.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		_main.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(0.0)
		});
		_main.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(0.0)
		});
		_main.RowDefinitions.Add(new RowDefinition
		{
			Height = GridLength.Auto
		});
		_main.RowDefinitions.Add(new RowDefinition
		{
			Height = GridLength.Auto
		});
		_ocBox = new Border
		{
			Child = _oc
		};
		_side = new StackPanel
		{
			Children = { _profiles, _info }
		};
		_main.Children.Add(_ocBox);
		_main.Children.Add(_side);
		Layout(wide: true);
		ScrollViewer root = Ui.Scroll(Ui.Col(_tiles, about, _main));
		root.SizeChanged += delegate(object _, SizeChangedEventArgs e)
		{
			Layout(e.NewSize.Width >= 740.0);
		};
		return root;
	}

	/// <summary>Two columns when there is room for the sliders and the table side by side, one otherwise.</summary>
	private void Layout(bool wide)
	{
		_main.ColumnDefinitions[1].Width = new GridLength(wide ? 12.0 : 0.0);
		_main.ColumnDefinitions[2].Width = new GridLength(wide ? 290.0 : 0.0);
		Grid.SetColumn(_side, wide ? 2 : 0);
		Grid.SetRow(_side, wide ? 0 : 1);
	}

	public override void Shown()
	{
		S.BusyTask(L.T("Reading the hardware…"), delegate
		{
			Hardware.Facts facts = Hardware.Detect();
			List<NvClock> item = null;
			int? item2 = null;
			AmdState amd = null;
			string vendor = Vendor(facts);
			GpuProfiles store = Hub.GpuProfilesLoad();
			if (Flags.Open("gpu-oc") && vendor == GpuOc.Nvidia)
			{
				item = NvOc.Read();
				item2 = NvOc.GetVoltage();
			}
			else if (Flags.Open("gpu-oc") && vendor == GpuOc.Amd)
			{
				amd = AmdOc.Read();
				item = AmdOc.Clocks(amd);
			}
			return (facts: facts, clocks: item, volt: item2, store: store, vendor: vendor, amd: amd);
		}, delegate((Hardware.Facts facts, List<NvClock> clocks, int? volt, GpuProfiles store, string vendor, AmdState amd) r)
		{
			_store = r.store ?? new GpuProfiles();
			_vendor = r.vendor;
			_amd = r.amd;
			_info.Children.Clear();
			if (r.facts != null)
			{
				foreach (Hardware.Gpu g2 in r.facts.Gpus)
				{
					TextBlock name = Ui.Text(g2.Name, 0.0, "Strong", bold: false, wrap: true);
					name.FontWeight = FontWeights.Medium;
					FactGrid factGrid = new FactGrid(1).Add(L.T("Driver"), g2.Basic ? L.T("the real driver is missing") : (g2.Version + " · " + g2.Date));
					WrapPanel links = new WrapPanel
					{
						Margin = new Thickness(0.0, 8.0, 0.0, 0.0)
					};
					if (g2.Vendor == "nvidia")
					{
						Button nv = Ui.Btn(L.T("NVCleanstall (official page)"), delegate
						{
							Hardware.OpenLink("https://www.techpowerup.com/download/techpowerup-nvcleanstall/");
						}, 0.0);
						nv.Margin = new Thickness(0.0, 0.0, 6.0, 6.0);
						links.Children.Add(nv);
					}
					if (Hardware.GpuLink(g2.Vendor).Length > 0)
					{
						Button maker = Ui.Btn(L.T("Drivers on the maker's site"), delegate
						{
							Hardware.OpenLink(Hardware.GpuLink(g2.Vendor));
						}, 0.0);
						maker.Margin = new Thickness(0.0, 0.0, 6.0, 6.0);
						links.Children.Add(maker);
					}
					_info.Children.Add(Ui.Card(Ui.Col(Ui.Head(L.T("Graphics card")), name, factGrid, links), 11.0));
				}
				if (r.facts.Gpus.Count == 0)
				{
					_info.Children.Add(Ui.Card(Ui.Note(L.T("No graphics card was found.")), 11.0));
				}
				_clocks = r.clocks;
				_coreRange = GpuOcLogic.CoreRange(_clocks?.FirstOrDefault((NvClock c) => c.Domain == 0));
				_memRange = GpuOcLogic.MemRange(_clocks?.FirstOrDefault((NvClock c) => c.Domain == 4));
				// The sliders start at what the driver reports now (read back, not what was asked for).
				_core = (_clocks?.FirstOrDefault((NvClock c) => c.Domain == 0)?.DeltaKHz).GetValueOrDefault() / 1000;
				_mem = (_clocks?.FirstOrDefault((NvClock c) => c.Domain == 4)?.DeltaKHz).GetValueOrDefault() / 1000;
				_volt = r.volt.GetValueOrDefault();
				_hasVolt = r.volt.HasValue;
				// AMD: the power limit and the voltage as offsets from the driver's stock values.
				_powerRange = GpuOcLogic.PowerRange(_amd?.Power);
				_voltMvRange = GpuOcLogic.VoltMvRange(_amd?.Volt);
				_power = (_amd != null && _amd.Power.Editable) ? _amd.Power.Offset : 0;
				_voltMv = (_amd != null && _amd.Volt.Editable) ? Math.Min(0, _amd.Volt.Offset) : 0;
				_loaded = true;
				DrawTiles();
				DrawOc(r.facts);
			}
		});
	}

	/// <summary>Whose overclocking the page offers: NVIDIA when an NVIDIA card with its driver is there, else AMD.</summary>
	private static string Vendor(Hardware.Facts facts)
	{
		if (facts == null)
		{
			return "";
		}
		if (facts.Gpus.Any((Hardware.Gpu g) => g.Vendor == "nvidia" && !g.Basic))
		{
			return GpuOc.Nvidia;
		}
		if (facts.Gpus.Any((Hardware.Gpu g) => g.Vendor == "amd" && !g.Basic))
		{
			return GpuOc.Amd;
		}
		return "";
	}

	public override void Tick()
	{
		if (_temp != null && _loaded && Flags.Open("gpu-oc"))
		{
			int num = Amd ? AmdOc.Temperature() : NvOc.Temperature();
			if (num > 0)
			{
				Ui.SetValue(_temp, num.ToString(CultureInfo.CurrentCulture), "°C", "Accent");
			}
			else
			{
				Ui.SetValue(_temp, "—", null, "Faint");
			}
		}
	}

	private static string Mhz(int v)
	{
		return v.ToString("+0;-0;0", CultureInfo.InvariantCulture) + " MHz";
	}

	private static string Signed(int v)
	{
		return v.ToString("+0;-0;0", CultureInfo.InvariantCulture);
	}

	/// <summary>The readings across the top: the clocks the driver reports (stock + offset), temperature, voltage boost.
	/// Only what the existing driver calls give; nothing new is asked from the card.</summary>
	private void DrawTiles()
	{
		_tiles.Children.Clear();
		_temp = null;
		if (_clocks == null || !Flags.Open("gpu-oc"))
		{
			return;
		}
		NvClock core = _clocks.FirstOrDefault((NvClock c) => c.Domain == 0);
		NvClock mem = _clocks.FirstOrDefault((NvClock c) => c.Domain == 4);
		List<UIElement> tiles = new List<UIElement>();
		if (core != null)
		{
			tiles.Add(Tile(L.T("Core"), ((core.BaseKHz + core.DeltaKHz) / 1000).ToString("N0", CultureInfo.CurrentCulture), L.T("MHz"), L.F("stock {0} · offset {1}", core.BaseKHz / 1000, Signed(core.DeltaKHz / 1000)), out _));
		}
		if (mem != null)
		{
			tiles.Add(Tile(L.T("Memory"), ((mem.BaseKHz + mem.DeltaKHz) / 1000).ToString("N0", CultureInfo.CurrentCulture), L.T("MHz"), L.F("stock {0} · offset {1}", mem.BaseKHz / 1000, Signed(mem.DeltaKHz / 1000)), out _));
		}
		tiles.Add(Tile(L.T("Temperature"), "—", null, L.T("read every second"), out _temp));
		if (Amd)
		{
			bool pw = _amd != null && _amd.Power.Editable;
			tiles.Add(Tile(L.T("Power limit"), pw ? Signed(_amd.Power.Offset) : "—", pw ? "%" : null, pw ? L.T("offset from stock") : L.T("not offered"), out _));
		}
		else
		{
			tiles.Add(Tile(L.T("Voltage boost"), _hasVolt ? _volt.ToString(CultureInfo.InvariantCulture) : "—", _hasVolt ? "%" : null, _hasVolt ? L.T("of what the maker allows") : L.T("not offered"), out _));
		}
		UniformGrid grid = new UniformGrid
		{
			Columns = tiles.Count,
			Rows = 1,
			Margin = new Thickness(-5.0, 0.0, -5.0, 10.0)
		};
		foreach (UIElement t in tiles)
		{
			grid.Children.Add(t);
		}
		_tiles.Children.Add(grid);
		Tick();
	}

	private static Border Tile(string caption, string value, string unit, string sub, out TextBlock valueText)
	{
		valueText = new TextBlock
		{
			FontFamily = Ui.Mono,
			FontSize = 22.0,
			FontWeight = FontWeights.SemiBold,
			Margin = new Thickness(0.0, 6.0, 0.0, 2.0),
			TextTrimming = TextTrimming.CharacterEllipsis
		};
		Ui.SetValue(valueText, value, unit);
		TextBlock s = Ui.Text(sub, Ui.FsSmall, "Faint");
		s.FontFamily = Ui.Mono;
		Border tile = Ui.Panel(Ui.Col(Ui.Caps(caption), valueText, s), new Thickness(14.0, 11.0, 14.0, 11.0));
		tile.Margin = new Thickness(5.0, 0.0, 5.0, 0.0);
		return tile;
	}

	/// <summary>
	/// A segmented slider with − / + step buttons and the value between them; it only changes the value in the page —
	/// nothing is written until "Apply".
	/// </summary>
	private UIElement SliderRow(string label, string unit, GpuRange range, Func<int> get, Action<int> set, string hint)
	{
		TextBlock value = new TextBlock
		{
			FontFamily = Ui.Mono,
			FontSize = 17.0,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong"),
			MinWidth = 104.0,
			TextAlignment = TextAlignment.Center,
			HorizontalAlignment = HorizontalAlignment.Center,
			VerticalAlignment = VerticalAlignment.Center
		};
		set(range.Snap(get()));
		SegSlider slider = new SegSlider(range.Min, range.Max, range.Step, get(), (int v) => range.Snap(v))
		{
			Margin = new Thickness(0.0, 10.0, 0.0, 0.0)
		};
		slider.ValueChanged += delegate(int v)
		{
			set(range.Snap(v));
			Show();
		};
		_sync.Add(delegate
		{
			slider.Value = range.Snap(get());
			Show();
		});
		Show();
		Button minus = StepButton("−", delegate
		{
			slider.Value = range.Snap(get() - range.Step);
		});
		Button plus = StepButton("+", delegate
		{
			slider.Value = range.Snap(get() + range.Step);
		});
		Grid stepper = new Grid
		{
			Height = 34.0
		};
		stepper.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(34.0)
		});
		stepper.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = GridLength.Auto
		});
		stepper.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(34.0)
		});
		Border mid = new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 1.0, 0.0, 1.0),
			Background = Ui.B("Panel"),
			Padding = new Thickness(8.0, 0.0, 8.0, 0.0),
			Child = value
		};
		Grid.SetColumn(mid, 1);
		Grid.SetColumn(plus, 2);
		stepper.Children.Add(minus);
		stepper.Children.Add(mid);
		stepper.Children.Add(plus);

		TextBlock name = Ui.Text(label, Ui.FsBody * 1.05, "Strong");
		TextBlock span = Ui.Text(Signed(range.Min) + "…" + Signed(range.Max), Ui.FsSmall, "Faint");
		span.FontFamily = Ui.Mono;
		span.Margin = new Thickness(0.0, 2.0, 0.0, 0.0);
		Grid head = new Grid();
		head.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		head.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = GridLength.Auto
		});
		StackPanel left = new StackPanel
		{
			VerticalAlignment = VerticalAlignment.Center,
			Margin = new Thickness(0.0, 0.0, 10.0, 0.0),
			Children = { name, span }
		};
		Grid.SetColumn(stepper, 1);
		head.Children.Add(left);
		head.Children.Add(stepper);

		// Scale under the bar: the ends, and zero where it falls inside the range.
		Grid scale = new Grid
		{
			Margin = new Thickness(0.0, 4.0, 0.0, 0.0)
		};
		double f = (range.Max > range.Min) ? Math.Clamp((0.0 - range.Min) / (double)(range.Max - range.Min), 0.0, 1.0) : 0.0;
		scale.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(Math.Max(f, 0.0001), GridUnitType.Star)
		});
		scale.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(Math.Max(1.0 - f, 0.0001), GridUnitType.Star)
		});
		TextBlock lo = ScaleText(Signed(range.Min));
		scale.Children.Add(lo);
		TextBlock hi = ScaleText(Signed(range.Max) + ((unit == "%" || unit == "±%") ? " %" : ((unit == "mV") ? " mV" : "")));
		hi.HorizontalAlignment = HorizontalAlignment.Right;
		Grid.SetColumn(hi, 1);
		scale.Children.Add(hi);
		if (range.Min < 0 && range.Max > 0 && f > 0.08 && f < 0.92)
		{
			TextBlock zero = ScaleText("0");
			Grid.SetColumn(zero, 1);
			scale.Children.Add(zero);
		}
		TextBlock note = Ui.Text(hint, Ui.FsSmall, "Faint", bold: false, wrap: true);
		note.Margin = new Thickness(0.0, 3.0, 0.0, 0.0);
		return new StackPanel
		{
			Margin = new Thickness(0.0, 14.0, 0.0, 0.0),
			Children = { head, slider, scale, note }
		};
		void Show()
		{
			// "%": 0–100 (NVIDIA voltage boost); "±%": a signed percent offset (AMD power limit); "mV": AMD voltage offset.
			Ui.SetValue(value, (unit == "%") ? get().ToString(CultureInfo.InvariantCulture) : Signed(get()), (unit == "%" || unit == "±%") ? "%" : ((unit == "mV") ? "mV" : L.T("MHz")));
		}
	}

	private static TextBlock ScaleText(string s)
	{
		return new TextBlock
		{
			Text = s,
			FontFamily = Ui.Mono,
			FontSize = Ui.FsSmall * 0.95,
			Foreground = Ui.B("Faint")
		};
	}

	private static Button StepButton(string glyph, Action click)
	{
		Button b = new Button
		{
			Content = glyph,
			FontFamily = Ui.Mono,
			FontSize = 15.0,
			Padding = new Thickness(0.0),
			Background = Ui.B("Panel")
		};
		b.Click += delegate
		{
			click();
		};
		return b;
	}

	private void SyncSliders()
	{
		foreach (Action item in _sync)
		{
			item();
		}
	}

	/// <summary>Writes the values, reads them back, logs what is in force and redraws the page from the driver.</summary>
	private void Apply(int coreMHz, int memMHz, int volt, string profile)
	{
		int power = Amd ? _power : 0;
		int voltMv = Amd ? _voltMv : 0;
		S.BusyTask(L.T("Writing…"), () => H.GpuOcApply(coreMHz * 1000, memMHz * 1000, Amd ? 0 : volt, true, power, voltMv), delegate(List<string> log)
		{
			if (profile != null)
			{
				S.Did(L.F("GPU profile \"{0}\" applied", profile));
			}
			foreach (string item in log ?? new List<string>())
			{
				S.Did(item, (OcError.Length > 0 && item.Contains(OcError)) ? AppLog.Sev.Warn : AppLog.Sev.Info);
			}
			Shown();
		});
	}

	/// <summary>Saves the profile file; false (with a log line) when it cannot be written.</summary>
	private bool SaveStore()
	{
		try
		{
			_store.Save();
			return true;
		}
		catch (Exception ex)
		{
			S.Did(L.F("GPU profiles were not saved: {0}", ex.Message), AppLog.Sev.Warn);
			return false;
		}
	}

	/// <summary>Fills the sliders with a profile, cut to what this card allows. True when a value had to be cut.</summary>
	private bool Load(GpuProfile p)
	{
		_core = _coreRange.Snap(p.CoreMHz);
		_mem = _memRange.Snap(p.MemMHz);
		_volt = _hasVolt ? GpuOcLogic.Volt.Snap(p.VoltPct) : 0;
		_power = Amd ? _powerRange.Snap(p.PowerPct) : 0;
		_voltMv = Amd ? _voltMvRange.Snap(p.VoltMv) : 0;
		SyncSliders();
		bool cut = _core != p.CoreMHz || _mem != p.MemMHz;
		// Values this maker's card does not have count as cut: NVIDIA has no power limit here, AMD no voltage boost.
		return Amd ? (cut || _power != p.PowerPct || _voltMv != p.VoltMv || p.VoltPct != 0) : (cut || (_hasVolt && _volt != p.VoltPct) || p.PowerPct != 0 || p.VoltMv != 0);
	}

	private void DrawOc(Hardware.Facts facts)
	{
		_oc.Children.Clear();
		_profiles.Children.Clear();
		_sync.Clear();
		TextBlock head = Ui.Head(L.T("Overclocking"));
		if (!Flags.Open("gpu-oc"))
		{
			_oc.Children.Add(Ui.Card(Ui.Col(head, Ui.Note(L.T("Closed. Open the feature flag \"GPU overclocking\" on the Settings page if you know what you are doing: wrong values make the card unstable, and extra voltage shortens its life."))), 11.0));
			return;
		}
		if (_vendor.Length == 0)
		{
			_oc.Children.Add(Ui.Card(Ui.Col(head, Ui.Note(L.T("Available for NVIDIA cards with the NVIDIA driver and for AMD Radeon cards with the AMD driver installed. Intel cards are not supported yet."))), 11.0));
			return;
		}
		if (_clocks == null)
		{
			// Nothing is offered and nothing is written: the driver's own answer says why (no ADL, no overdrive, odd table…).
			_oc.Children.Add(Ui.Card(Ui.Col(head, Ui.Text((OcError.Length > 0) ? OcError : L.T("The driver did not report its clocks."), Ui.FsNote, "Warn", bold: false, wrap: true)), 11.0));
			return;
		}
		NvClock nvClock = _clocks.FirstOrDefault((NvClock c) => c.Domain == 0);
		NvClock nvClock2 = _clocks.FirstOrDefault((NvClock c) => c.Domain == 4);
		StackPanel stackPanel = Ui.Col(head);
		// The crash guard kept a profile from coming back at the start: say so until the user reacts.
		if (_store.Skipped.Length > 0)
		{
			TextBlock skipped = Ui.Text(L.F("Profile \"{0}\" was NOT applied at start ({1}): Windows crashed after it was set (a blue screen or a sudden power-off). Lower the values, apply them by hand and mark the profile for the start again.", _store.Skipped, _store.SkippedUtc.ToLocalTime().ToString("g", CultureInfo.CurrentCulture)), Ui.FsNote, "Warn", bold: false, wrap: true);
			Button ok = Ui.Btn(L.T("Understood"), delegate
			{
				_store.Skipped = "";
				SaveStore();
				DrawOc(facts);
			}, 110.0);
			ok.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
			ok.HorizontalAlignment = HorizontalAlignment.Left;
			Border warn = new Border
			{
				Background = Ui.B("WarnSoft"),
				BorderBrush = Ui.B("WarnLine"),
				BorderThickness = new Thickness(1.0),
				CornerRadius = new CornerRadius(3.0),
				Padding = new Thickness(12.0, 9.0, 12.0, 10.0),
				Margin = new Thickness(0.0, 0.0, 0.0, 10.0),
				Child = Ui.Col(skipped, ok)
			};
			stackPanel.Children.Add(warn);
		}
		stackPanel.Children.Add(Ui.Note(L.T("Offsets are added to the card's own clocks. Raise in small steps and test in a game after each one; artefacts, a black screen or a driver restart mean the value is too high. Moving a slider changes nothing until you press \"Apply\".")));
		if (Amd)
		{
			stackPanel.Children.Add(Ui.Note(L.F("AMD Radeon ({0}): the offsets count from the card's stock maximum clocks that the driver reports; on older cards only the top performance state is raised, the lower states and the voltages stay as they are.", _amd.Kind)));
		}
		TextBlock now = Ui.Text(Amd
			? L.F("In force now (read from the driver): core {0} · memory {1} · power limit {2} · voltage offset {3}", Mhz((nvClock?.DeltaKHz).GetValueOrDefault() / 1000), Mhz((nvClock2?.DeltaKHz).GetValueOrDefault() / 1000), _amd.Power.Editable ? (Signed(_amd.Power.Offset) + " %") : "—", _amd.Volt.Editable ? (Signed(_amd.Volt.Offset) + " mV") : "—")
			: L.F("In force now (read from the driver): core {0} · memory {1} · voltage boost {2}", Mhz((nvClock?.DeltaKHz).GetValueOrDefault() / 1000), Mhz((nvClock2?.DeltaKHz).GetValueOrDefault() / 1000), _hasVolt ? (_volt + " %") : "—"), Ui.FsSmall, "Silver", bold: false, wrap: true);
		now.FontFamily = Ui.Mono;
		now.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
		stackPanel.Children.Add(now);
		if (nvClock != null && nvClock.Editable)
		{
			stackPanel.Children.Add(SliderRow(L.T("Core clock offset"), "MHz", _coreRange, () => _core, delegate(int v)
			{
				_core = v;
			}, L.F("stock {0} MHz · allowed {1}…{2} MHz · step {3} MHz", nvClock.BaseKHz / 1000, _coreRange.Min, _coreRange.Max, _coreRange.Step)));
		}
		else
		{
			stackPanel.Children.Add(Ui.Note(L.T("The driver does not allow changing the core clock on this card.")));
		}
		if (nvClock2 != null && nvClock2.Editable)
		{
			stackPanel.Children.Add(SliderRow(L.T("Memory clock offset"), "MHz", _memRange, () => _mem, delegate(int v)
			{
				_mem = v;
			}, L.F("stock {0} MHz · allowed {1}…{2} MHz · step {3} MHz", nvClock2.BaseKHz / 1000, _memRange.Min, _memRange.Max, _memRange.Step)));
		}
		else
		{
			stackPanel.Children.Add(Ui.Note(L.T("The driver does not allow changing the memory clock on this card.")));
		}
		if (Amd)
		{
			if (_amd.Power.Editable)
			{
				stackPanel.Children.Add(SliderRow(L.T("Power limit"), "±%", _powerRange, () => _power, delegate(int v)
				{
					_power = v;
				}, L.F("offset from the stock power limit · allowed {0}…{1} % (reported by the driver)", Signed(_powerRange.Min), Signed(_powerRange.Max))));
			}
			else
			{
				stackPanel.Children.Add(Ui.Note(L.T("This card or driver does not offer a power limit.")));
			}
			if (_amd.Volt.Editable && _voltMvRange.Min < 0)
			{
				stackPanel.Children.Add(SliderRow(L.T("Voltage offset"), "mV", _voltMvRange, () => _voltMv, delegate(int v)
				{
					_voltMv = v;
				}, L.F("only lowering (undervolt) · allowed {0}…0 mV · step {1} mV", _voltMvRange.Min, _voltMvRange.Step)));
			}
			else
			{
				stackPanel.Children.Add(Ui.Note(L.T("This card or driver does not offer a voltage offset.")));
			}
		}
		else if (_hasVolt)
		{
			stackPanel.Children.Add(SliderRow(L.T("Voltage boost"), "%", GpuOcLogic.Volt, () => _volt, delegate(int v)
			{
				_volt = v;
			}, L.T("0–100% of the extra voltage the card's maker allows")));
		}
		else
		{
			stackPanel.Children.Add(Ui.Note(L.T("This card or driver does not offer a voltage boost.")));
		}
		TextBlock limits = Ui.Note(Amd ? L.T("Fan control and memory timings are not offered here.") : L.T("Power limit and fan control are not offered here."));
		limits.Margin = new Thickness(0.0, 12.0, 0.0, 0.0);
		stackPanel.Children.Add(limits);
		TextBlock risk = Ui.Text("▲  " + L.T("Extra voltage raises temperature and wear. Neither this program nor its authors answer for damage: use it at your own risk."), Ui.FsSmall, "Warn", bold: false, wrap: true);
		risk.FontFamily = Ui.Mono;
		risk.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
		stackPanel.Children.Add(risk);
		Button button = Ui.ConfirmBtn(L.T("Apply"), delegate
		{
			Apply(_core, _mem, _volt, null);
		});
		Ui.Primary(button);
		button.Height = 34.0;
		button.MinWidth = 130.0;
		button.Margin = new Thickness(0.0);
		Button button2 = Ui.Btn(L.T("Reset to stock"), delegate
		{
			S.BusyTask(L.T("Writing…"), delegate
			{
				Settings.Set("GpuOcAppliedUtc", "");
				return Amd ? AmdOc.Reset(force: true) : NvOc.Reset(force: true);
			}, delegate(bool ok)
			{
				S.Did(ok ? (Amd ? L.T("GPU overclock: clocks, power limit and voltage are back to the driver's stock values") : L.T("GPU overclock: clocks and voltage are back to stock")) : L.F("GPU overclock: {0}", OcError), (!ok) ? AppLog.Sev.Warn : AppLog.Sev.Info);
				if (_store.AtStart.Length > 0)
				{
					_store.SetAtStart(null);
					if (SaveStore())
					{
						S.Did(L.T("GPU overclock: apply at start off"));
					}
				}
				Shown();
			});
		}, 130.0);
		button2.Height = 34.0;
		button2.Margin = new Thickness(0.0, 0.0, 10.0, 0.0);
		StackPanel buttons = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			HorizontalAlignment = HorizontalAlignment.Right,
			Children = { button2, button }
		};
		Border bar = new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 1.0, 0.0, 0.0),
			Margin = new Thickness(0.0, 14.0, 0.0, 0.0),
			Padding = new Thickness(0.0, 12.0, 0.0, 0.0),
			Child = buttons
		};
		stackPanel.Children.Add(bar);
		TextBlock stay = Ui.Note(Amd ? L.T("The values stay in the driver when the program closes. Whether the AMD driver keeps them after a restart of Windows depends on the driver; after a crash it usually returns the card to stock by itself. \"Reset to stock\" always writes the driver's own stock values.") : L.T("The values stay in the driver when the program closes. A restart of Windows or of the driver returns the card to stock until they are applied again."));
		stay.Margin = new Thickness(0.0, 10.0, 0.0, 0.0);
		stackPanel.Children.Add(stay);
		_oc.Children.Add(Ui.Card(stackPanel, 11.0));
		_profiles.Children.Add(Ui.Card(Profiles(facts), 11.0));
	}

	/// <summary>Named profiles as a table: pick a row, then load, apply, mark for the start, rename or delete it.</summary>
	private StackPanel Profiles(Hardware.Facts facts)
	{
		StackPanel col = Ui.Col(Ui.Head(L.T("Profiles")));
		GpuProfile sel = _store.Find(_selected);
		Grid header = ProfileGrid();
		AddCell(header, Ui.Caps(L.T("Name"), Ui.FsSmall * 0.9, "Faint"), 0);
		AddCell(header, Ui.Caps(L.T("Core"), Ui.FsSmall * 0.9, "Faint"), 1, right: true);
		AddCell(header, Ui.Caps(L.T("Mem."), Ui.FsSmall * 0.9, "Faint"), 2, right: true);
		AddCell(header, Ui.Caps(Amd ? "P" : "U", Ui.FsSmall * 0.9, "Faint"), 3, right: true);
		col.Children.Add(new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(10.0, 0.0, 8.0, 6.0),
			Child = header
		});
		if (_store.Profiles.Count == 0)
		{
			TextBlock none = Ui.Note(L.T("No profiles yet."));
			none.Margin = new Thickness(10.0, 10.0, 0.0, 4.0);
			col.Children.Add(none);
		}
		foreach (GpuProfile p in _store.Profiles.ToList())
		{
			col.Children.Add(ProfileRow(p, p == sel, facts));
		}
		if (sel != null)
		{
			col.Children.Add(Actions(sel, facts));
		}
		else if (_store.Profiles.Count > 0)
		{
			TextBlock pick = Ui.Note(L.T("Click a profile to load, apply, rename or delete it."));
			pick.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
			col.Children.Add(pick);
		}

		// Save the sliders: a dashed box like "+ save current" in the mockup.
		string name = "";
		TextBox box = Ui.Input("", 150.0, delegate(string t)
		{
			name = t;
		});
		box.Margin = new Thickness(0.0, 0.0, 6.0, 0.0);
		Button save = Ui.Btn("+ " + L.T("Save sliders as profile"), delegate
		{
			string n = GpuProfiles.CleanName(name);
			if (n == null)
			{
				S.Did(L.F("A profile name needs 1–{0} characters", GpuProfiles.MaxName), AppLog.Sev.Warn);
				return;
			}
			bool existed = _store.Find(n) != null;
			_store.Put(n, _core, _mem, Amd ? 0 : _volt, Amd ? _power : 0, Amd ? _voltMv : 0, _vendor);
			if (SaveStore())
			{
				S.Did(existed ? L.F("GPU profile \"{0}\" overwritten", n) : L.F("GPU profile \"{0}\" saved", n));
			}
			_selected = n;
			DrawOc(facts);
		}, 0.0);
		save.Margin = new Thickness(0.0);
		save.HorizontalAlignment = HorizontalAlignment.Stretch;
		DockPanel add = new DockPanel();
		DockPanel.SetDock(box, Dock.Top);
		box.Width = double.NaN;
		box.Margin = new Thickness(0.0, 0.0, 0.0, 6.0);
		add.Children.Add(box);
		add.Children.Add(save);
		col.Children.Add(new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(1.0),
			CornerRadius = new CornerRadius(3.0),
			Padding = new Thickness(8.0),
			Margin = new Thickness(0.0, 12.0, 0.0, 0.0),
			Child = add
		});
		TextBlock about = Ui.Note(L.T("The profile marked \"at start\" is set again every time the program starts (the driver forgets overclocking at every restart). It is NOT re-applied only after Windows crashed (a blue screen or a sudden power-off) — that keeps an unstable value from coming back; normal restarts keep it."));
		about.Margin = new Thickness(0.0, 10.0, 0.0, 0.0);
		col.Children.Add(about);
		return col;
	}

	private static Grid ProfileGrid()
	{
		Grid g = new Grid();
		g.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		g.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(50.0)
		});
		g.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(58.0)
		});
		g.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(34.0)
		});
		return g;
	}

	private static void AddCell(Grid g, TextBlock t, int col, bool right = false)
	{
		if (right)
		{
			t.HorizontalAlignment = HorizontalAlignment.Right;
		}
		t.VerticalAlignment = VerticalAlignment.Center;
		Grid.SetColumn(t, col);
		g.Children.Add(t);
	}

	private UIElement ProfileRow(GpuProfile p, bool selected, Hardware.Facts facts)
	{
		bool atStart = _store.StartProfile == p;
		Grid g = ProfileGrid();
		TextBlock name = Ui.Text(p.Name, Ui.FsBody, selected ? "AccentText" : "Strong");
		name.FontWeight = FontWeights.Medium;
		name.TextTrimming = TextTrimming.None;   // long names ("Saved values") wrap instead of "…"
		name.TextWrapping = TextWrapping.Wrap;
		if (atStart)
		{
			name.Inlines.Add(new System.Windows.Documents.Run("  ▶ " + L.T("at start"))
			{
				FontSize = Ui.FsSmall,
				Foreground = Ui.B("Accent")
			});
		}
		AddCell(g, name, 0);
		AddCell(g, Num(Signed(p.CoreMHz), selected), 1, right: true);
		AddCell(g, Num(Signed(p.MemMHz), selected), 2, right: true);
		// The last column: the voltage boost of an NVIDIA profile, the power limit offset of an AMD one.
		AddCell(g, Num((p.Maker == GpuOc.Amd) ? Signed(p.PowerPct) : p.VoltPct.ToString(CultureInfo.InvariantCulture), selected), 3, right: true);
		Grid row = new Grid
		{
			MinHeight = 36.0,
			Background = Ui.B(selected ? "Sel" : "None"),
			Cursor = System.Windows.Input.Cursors.Hand,
			ToolTip = (p.Maker == GpuOc.Amd) ? L.F("AMD · core {0} · memory {1} · power limit {2} % · voltage offset {3} mV", Mhz(p.CoreMHz), Mhz(p.MemMHz), Signed(p.PowerPct), Signed(p.VoltMv)) : L.F("core {0} · memory {1} · voltage boost {2} %", Mhz(p.CoreMHz), Mhz(p.MemMHz), p.VoltPct)
		};
		if (selected)
		{
			row.Children.Add(new Border
			{
				Width = 2.0,
				Background = Ui.B("Accent"),
				HorizontalAlignment = HorizontalAlignment.Left
			});
		}
		row.Children.Add(new Border
		{
			BorderBrush = Ui.B("Line2"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(10.0, 6.0, 8.0, 6.0),
			Child = g
		});
		if (!selected)
		{
			row.MouseEnter += delegate
			{
				row.Background = Ui.B("Hover");
			};
			row.MouseLeave += delegate
			{
				row.Background = Ui.B("None");
			};
		}
		row.MouseLeftButtonUp += delegate
		{
			_selected = selected ? "" : p.Name;
			DrawOc(facts);
		};
		return row;
	}

	private static TextBlock Num(string s, bool selected)
	{
		TextBlock t = Ui.Text(s, Ui.FsNote, selected ? "AccentText" : "Text");
		t.FontFamily = Ui.Mono;
		return t;
	}

	/// <summary>What can be done with the selected profile (the same actions as the 1.13 rows).</summary>
	private UIElement Actions(GpuProfile p, Hardware.Facts facts)
	{
		bool atStart = _store.StartProfile == p;
		WrapPanel actions = new WrapPanel
		{
			Margin = new Thickness(0.0, 10.0, 0.0, 0.0)
		};
		void Add(Button b)
		{
			b.Margin = new Thickness(0.0, 0.0, 6.0, 6.0);
			actions.Children.Add(b);
		}
		Add(Ui.Btn(L.T("Load into sliders"), delegate
		{
			bool cut = Load(p);
			S.Did(cut ? L.F("GPU profile \"{0}\" loaded; some values were cut to what this card allows", p.Name) : L.F("GPU profile \"{0}\" loaded into the sliders", p.Name));
		}, 0.0));
		Button apply = Ui.ConfirmBtn(L.T("Apply"), delegate
		{
			Load(p);
			Apply(_core, _mem, _volt, p.Name);
		});
		apply.MinWidth = 0.0;
		Ui.Primary(apply);
		Add(apply);
		Add(Ui.Btn(atStart ? L.T("Do not apply at start") : L.T("Apply at start"), delegate
		{
			// The same values (or higher) already crashed Windows: ask first, "No" is the default (1.16.0).
			GpuCrash crash = atStart ? null : GpuOcLogic.CrashedBefore(p, _store.Crashes);
			if (crash != null)
			{
				string q = L.F("Windows crashed on {0} with these values or lower (core {1:+0;-0;0} MHz, memory {2:+0;-0;0} MHz). Applied at every start, they can crash it again.\n\nLower at least one value first. Apply \"{3}\" at start anyway?", crash.Utc.ToLocalTime().ToString("g", CultureInfo.CurrentCulture), crash.CoreMHz, crash.MemMHz, p.Name);
				if (MessageBox.Show(q, L.T("Domain OS Toolbox: GPU profile"), MessageBoxButton.YesNo, MessageBoxImage.Warning, MessageBoxResult.No) != MessageBoxResult.Yes)
				{
					S.Did(L.F("GPU profile \"{0}\": not marked for the start (it crashed Windows before)", p.Name));
					return;
				}
				AppLog.Add(L.F("GPU profile \"{0}\": marked for the start although Windows crashed with these values on {1}", p.Name, crash.Utc.ToLocalTime().ToString("g", CultureInfo.CurrentCulture)), AppLog.Sev.Warn);
			}
			_store.SetAtStart(atStart ? null : p.Name);
			if (SaveStore())
			{
				S.Did(atStart ? L.T("GPU overclock: apply at start off") : L.F("GPU profile \"{0}\": apply at start on", p.Name));
			}
			DrawOc(facts);
		}, 0.0));
		StackPanel renameRow = Ui.Row();
		renameRow.Visibility = Visibility.Collapsed;
		string newName = p.Name;
		renameRow.Children.Add(Ui.Input(p.Name, 160.0, delegate(string t)
		{
			newName = t;
		}));
		renameRow.Children.Add(Ui.Btn(L.T("Rename"), delegate
		{
			string old = p.Name;
			if (!_store.Rename(old, newName))
			{
				S.Did(L.T("That name is empty, too long or already used by another profile"), AppLog.Sev.Warn);
				return;
			}
			if (SaveStore())
			{
				S.Did(L.F("GPU profile \"{0}\" renamed to \"{1}\"", old, p.Name));
			}
			_selected = p.Name;
			DrawOc(facts);
		}, 90.0));
		Add(Ui.Btn(L.T("Rename…"), delegate
		{
			renameRow.Visibility = (renameRow.Visibility == Visibility.Visible) ? Visibility.Collapsed : Visibility.Visible;
		}, 0.0));
		Button delete = Ui.ConfirmBtn(L.T("Delete"), delegate
		{
			_store.Delete(p.Name);
			if (SaveStore())
			{
				S.Did(L.F("GPU profile \"{0}\" deleted", p.Name));
			}
			_selected = "";
			DrawOc(facts);
		});
		delete.MinWidth = 0.0;
		Add(delete);
		if (_vendor.Length > 0 && p.Maker != _vendor)
		{
			TextBlock other = Ui.Note(L.T("This profile was saved for a card of another maker: it is never applied at start on this card. Loading it into the sliders keeps only what this card allows."));
			other.Margin = new Thickness(0.0, 4.0, 0.0, 0.0);
			return Ui.Col(actions, renameRow, other);
		}
		GpuCrash crashed = GpuOcLogic.CrashedBefore(p, _store.Crashes);
		if (crashed != null)
		{
			TextBlock warn = Ui.Text(L.F("Windows crashed on {0} with these values or lower (core {1:+0;-0;0} MHz, memory {2:+0;-0;0} MHz). Lower at least one value before you apply this profile at start.", crashed.Utc.ToLocalTime().ToString("g", CultureInfo.CurrentCulture), crashed.CoreMHz, crashed.MemMHz), Ui.FsNote, "Warn", bold: false, wrap: true);
			warn.Margin = new Thickness(0.0, 4.0, 0.0, 0.0);
			return Ui.Col(actions, renameRow, warn);
		}
		return Ui.Col(actions, renameRow);
	}
}
