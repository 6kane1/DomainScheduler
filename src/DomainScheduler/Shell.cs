using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Reflection;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Media;
using System.Windows.Shell;
using Path = System.Windows.Shapes.Path;

namespace DomainScheduler.App;

/// <summary>
/// The window (1.14.0, HUD look): our own title bar over WindowChrome (logo, status pill with the session time, the
/// Advanced mode switch, settings, the caption buttons), then either the simple home screen with its footer or the
/// advanced layout (numbered page list at the left, the page, status and activity log at the right).
/// </summary>
internal sealed class Shell : Window
{
	private readonly Hub _h;

	private readonly List<Page> _pages = new List<Page>();

	private readonly ContentControl _host = new ContentControl
	{
		Focusable = false
	};

	// ---- header
	private readonly Border _frame = new Border();

	private readonly TextBlock _crumb = new TextBlock
	{
		FontFamily = Ui.Mono,
		FontSize = Ui.FsSmall,
		FontWeight = FontWeights.SemiBold,
		VerticalAlignment = VerticalAlignment.Center,
		Margin = new Thickness(28.0, 0.0, 0.0, 0.0),
		TextTrimming = TextTrimming.CharacterEllipsis
	};

	private readonly Border _statusPill;

	private readonly Border _dot = new Border
	{
		Width = 7.0,
		Height = 7.0,
		HorizontalAlignment = HorizontalAlignment.Center,
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly Border _halo = new Border
	{
		Width = 13.0,
		Height = 13.0,
		VerticalAlignment = VerticalAlignment.Center,
		Margin = new Thickness(0.0, 0.0, 8.0, 0.0)
	};

	private readonly TextBlock _pill = new TextBlock
	{
		FontFamily = Ui.Mono,
		FontSize = Ui.FsSmall,
		FontWeight = FontWeights.Medium,
		VerticalAlignment = VerticalAlignment.Center,
		TextTrimming = TextTrimming.CharacterEllipsis
	};

	private readonly Run _pillKey = new Run();

	private readonly Run _pillWord = new Run
	{
		FontWeight = FontWeights.SemiBold
	};

	private readonly Run _pillSep = new Run();

	private readonly Run _pillTime = new Run();

	private readonly Button _modeBtn = new Button();

	private readonly Border _switch = new Border
	{
		Width = 26.0,
		Height = 14.0,
		BorderThickness = new Thickness(1.0),
		CornerRadius = new CornerRadius(2.0),
		Margin = new Thickness(0.0, 0.0, 9.0, 0.0),
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly Border _knob = new Border
	{
		Width = 8.0,
		Height = 8.0,
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly TextBlock _modeText = new TextBlock
	{
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly Button _gear = new Button();

	private readonly Button _back = new Button();

	private DateTime _activeSince;

	// ---- advanced layout
	private readonly Grid _advRoot = new Grid();

	private readonly StackPanel _navList = new StackPanel
	{
		Margin = new Thickness(0.0, 6.0, 0.0, 10.0)
	};

	private readonly Border _statusBox;

	private readonly TextBlock _stateBig;

	private readonly TextBlock _stateSub;

	private readonly TextBlock _nThreads;

	private readonly TextBlock _nLowered;

	private readonly TextBlock _nChanged;

	private readonly Button _toggle;

	private readonly Button _undo;

	private readonly StackPanel _steps = new StackPanel();



	private readonly TextBlock _capStatus;

	private readonly TextBlock _capSteps;

	private readonly TextBlock _capChanged;


	private readonly TextBlock _foot;

	private readonly StackPanel _kv;

	/// <summary>"This system" in the advanced layout's right column (where the journal was).</summary>
	private readonly SystemPanel _sysAdv = new SystemPanel();

	private readonly Border _overlay;

	private readonly TextBlock _overlayText = Ui.Text("", Ui.FsBody);

	private int _current;


	private AutoState _dotState = (AutoState)(-1);

	private bool _closedShown;

	private string _stepsKey = "";

	// ---- simple layout
	private readonly HomeView _home;

	private readonly Grid _simpleRoot = new Grid();

	private readonly ContentControl _simpleHost = new ContentControl
	{
		Focusable = false
	};

	private readonly Button _simpleUndo = new Button();

	private readonly TextBlock _simpleUndoText = new TextBlock
	{
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly TextBlock _simpleFoot = new TextBlock
	{
		FontFamily = Ui.Mono,
		FontSize = Ui.FsSmall,
		FontWeight = FontWeights.Medium,
		Foreground = Ui.B("Dim"),
		VerticalAlignment = VerticalAlignment.Center,
		TextTrimming = TextTrimming.CharacterEllipsis
	};

	private static string _gpuName = "";

	private bool _simple;

	private bool _simpleSettings;

	private bool _dwmBorder;

	/// <summary>Advanced mode shows every page; the simple mode only the home screen and the settings.</summary>
	public bool Simple => _simple;

	public double DpiScaleX => VisualTreeHelper.GetDpi(this).DpiScaleX;

	public bool AutoOn => _h.Auto.On;

	public Shell(Hub hub)
	{
		_h = hub;
		base.Title = "Domain OS";
		base.Width = 1340.0;
		base.Height = 840.0;
		base.MinWidth = 1180.0;
		base.MinHeight = 700.0;
		base.Background = Ui.B("Bg");
		base.Foreground = Ui.B("Text");
		base.FontSize = Ui.FsBody;
		base.FontFamily = Ui.Font();
		base.UseLayoutRounding = true;
		base.SnapsToDevicePixels = true;
		base.WindowStartupLocation = WindowStartupLocation.Manual;
		Chrome.Attach(this);
		_pages.Add(new QuickPage(hub, this));
		_pages.Add(new MasksPage(hub, this));
		_pages.Add(new GameThreadsPage(hub, this));
		_pages.Add(new ProcessesPage(hub, this));
		_pages.Add(new ChangesPage(hub, this));
		_pages.Add(new GpuPage(hub, this));
		_pages.Add(new DriversPage(hub, this));
		_pages.Add(new PowerPage(hub, this));
		_pages.Add(new DevicesPage(hub, this));
		_pages.Add(new SettingsPage(hub, this));
		_pages.Add(new AboutPage(hub, this));

		// ---- status block of the advanced layout (Scheduler section)
		_stateBig = new TextBlock
		{
			FontSize = Ui.FsBody * 1.6,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong"),
			Margin = new Thickness(0.0, 3.0, 0.0, 1.0)
		};
		_stateSub = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		_stateSub.Margin = new Thickness(0.0, 0.0, 0.0, 10.0);
		_toggle = new Button
		{
			Height = 34.0,
			FontSize = Ui.FsBody,
			FontWeight = FontWeights.Medium
		};
		_toggle.Click += delegate
		{
			_toggle.IsEnabled = false;
			_h.Paused = false;
			_h.Opt.Queue(_h.Auto.Click, skipIfBusy: false, reports: true);
		};
		_undo = new Button
		{
			Content = "",
			Height = 30.0,
			FontSize = Ui.FsBody,
			Margin = new Thickness(0.0, 6.0, 0.0, 0.0)
		};
		_undo.Click += delegate
		{
			UndoEverything();
		};
		_nThreads = Ui.Text("0");
		_nLowered = Ui.Text("0");
		_nChanged = Ui.Text("0");
		_capStatus = Ui.Caps("");
		_capSteps = Ui.Caps("");
		_capSteps.Margin = new Thickness(0.0, 14.0, 0.0, 4.0);
		_capChanged = Ui.Caps("");
		_capChanged.Margin = new Thickness(0.0, 12.0, 0.0, 0.0);
		_kv = new StackPanel();
		_statusBox = new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(0.0, 0.0, 0.0, 12.0),
			Margin = new Thickness(0.0, 0.0, 0.0, 12.0),
			Child = Ui.Col(_capStatus, _stateBig, _stateSub, _toggle, _undo, _capSteps, _steps, _capChanged, _kv)
		};
		_foot = new TextBlock
		{
			FontFamily = Ui.Mono,
			FontSize = Ui.FsSmall * 0.95,
			Foreground = Ui.B("Dim"),
			TextWrapping = TextWrapping.Wrap,
			LineHeight = Ui.FsSmall * 1.6
		};
		BuildAdvanced();

		_overlay = new Border
		{
			Background = Ui.B("Overlay"),
			Visibility = Visibility.Collapsed,
			Child = _overlayText
		};
		_overlayText.HorizontalAlignment = HorizontalAlignment.Center;
		_overlayText.FontFamily = Ui.Mono;

		// ---- header
		_statusPill = new Border
		{
			CornerRadius = new CornerRadius(3.0),
			Padding = new Thickness(10.0, 5.0, 12.0, 5.0),
			Margin = new Thickness(22.0, 0.0, 12.0, 0.0),
			BorderThickness = new Thickness(1.0),
			BorderBrush = Ui.B("Line"),
			VerticalAlignment = VerticalAlignment.Center,
			HorizontalAlignment = HorizontalAlignment.Left,
			Child = new DockPanel
			{
				Children = { _halo, _pill }
			}
		};
		DockPanel.SetDock(_halo, Dock.Left);
		_halo.Child = _dot;
		_pill.Inlines.Add(_pillKey);
		_pill.Inlines.Add(_pillWord);
		_pill.Inlines.Add(_pillSep);
		_pill.Inlines.Add(_pillTime);
		_pillKey.Foreground = Ui.B("Dim");
		_pillSep.Foreground = Ui.B("Faint");
		_pillTime.Foreground = Ui.B("Silver");
		Border header = BuildHeader();

		// ---- simple layout
		_home = new HomeView(hub, this);
		BuildSimple();

		Grid body = new Grid();
		body.Children.Add(_advRoot);
		body.Children.Add(_simpleRoot);
		body.Children.Add(_overlay);
		Grid root = new Grid();
		root.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(Chrome.CaptionHeight)
		});
		root.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(1.0, GridUnitType.Star)
		});
		Grid.SetRow(body, 1);
		root.Children.Add(header);
		root.Children.Add(body);
		_frame.Background = Ui.GridBackground();
		_frame.BorderBrush = Ui.B("Line");
		_frame.Child = root;
		base.Content = _frame;

		base.SourceInitialized += delegate
		{
			_dwmBorder = Chrome.Dwm(this);
			Frame();
		};
		base.IsVisibleChanged += delegate
		{
			UpdateOnScreen();
			if (base.IsVisible)
			{
				Refresh();
			}
		};
		base.StateChanged += delegate
		{
			Frame();
			UpdateOnScreen();
			if (base.WindowState != WindowState.Minimized)
			{
				Refresh();
			}
		};
		RestoreGeometry();
		L.Changed += OnLanguage;
		Texts();
		BuildNav();
		Show(0);
		SetSimple(!Settings.GetBool("Advanced"), resize: false);
		Refresh();
		// The graphics card name for the footer: read once, off the UI thread.
		if (_gpuName.Length == 0)
		{
			Task.Run(delegate
			{
				try
				{
					Hardware.Facts facts = Hardware.Detect();
					_gpuName = facts?.Gpus.FirstOrDefault((Hardware.Gpu g) => !g.Basic)?.Name ?? facts?.Gpus.FirstOrDefault()?.Name ?? "";
					base.Dispatcher.BeginInvoke((Action)delegate { _sysAdv.SetFacts(facts); _home.SysPanel.SetFacts(facts); });
				}
				catch (Exception)
				{
				}
				base.Dispatcher.BeginInvoke((Action)SimpleTexts);
			});
		}
	}

	/// <summary>The window is on screen (not hidden in the tray, not minimized): only then anything is drawn.</summary>
	private bool OnScreen => base.IsVisible && base.WindowState != WindowState.Minimized;

	private void UpdateOnScreen()
	{
		_h.UiVisible = OnScreen;
		if (!OnScreen)
		{
			_home.SetVisible(on: false);
		}
	}

	/// <summary>Maximized: keep the content on the screen. Normal: our own 1 px line where Windows 10 draws no border.</summary>
	private void Frame()
	{
		bool max = base.WindowState == WindowState.Maximized;
		_frame.Padding = max ? Chrome.MaximizedPadding(this) : new Thickness(0.0);
		_frame.BorderThickness = (max || _dwmBorder) ? new Thickness(0.0) : new Thickness(1.0);
	}

	private Border BuildHeader()
	{
		Grid g = new Grid();
		foreach (GridLength w in new GridLength[7]
		{
			GridLength.Auto,
			GridLength.Auto,
			new GridLength(1.0, GridUnitType.Star),
			GridLength.Auto,
			GridLength.Auto,
			GridLength.Auto,
			GridLength.Auto
		})
		{
			g.ColumnDefinitions.Add(new ColumnDefinition
			{
				Width = w
			});
		}
		StackPanel brand = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			VerticalAlignment = VerticalAlignment.Center,
			Margin = new Thickness(16.0, 0.0, 0.0, 0.0)
		};
		brand.Children.Add(Chrome.Logo());
		brand.Children.Add(new TextBlock
		{
			Text = HudMath.Spaced("Domain OS Toolbox", CultureInfo.InvariantCulture),
			FontFamily = Ui.Mono,
			FontSize = 12.5,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong"),
			VerticalAlignment = VerticalAlignment.Center,
			Margin = new Thickness(12.0, 0.0, 0.0, 0.0)
		});
		g.Children.Add(brand);
		Grid.SetColumn(_crumb, 1);
		g.Children.Add(_crumb);
		Grid.SetColumn(_statusPill, 2);
		g.Children.Add(_statusPill);

		_back.Height = 30.0;
		_back.Margin = new Thickness(0.0, 0.0, 8.0, 0.0);
		_back.VerticalAlignment = VerticalAlignment.Center;
		_back.Click += delegate
		{
			ShowSimpleSettings(on: false);
		};
		Grid.SetColumn(_back, 3);
		g.Children.Add(_back);

		_switch.Child = _knob;
		_modeBtn.Height = 30.0;
		_modeBtn.Padding = new Thickness(10.0, 0.0, 12.0, 0.0);
		_modeBtn.Margin = new Thickness(0.0, 0.0, 8.0, 0.0);
		_modeBtn.VerticalAlignment = VerticalAlignment.Center;
		_modeBtn.Foreground = Ui.B("Silver");
		_modeBtn.Content = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			Children = { _switch, _modeText }
		};
		_modeBtn.Click += delegate
		{
			SetSimple(!_simple);
		};
		Grid.SetColumn(_modeBtn, 4);
		g.Children.Add(_modeBtn);

		_gear.Width = 32.0;
		_gear.Height = 30.0;
		_gear.Padding = new Thickness(0.0);
		_gear.VerticalAlignment = VerticalAlignment.Center;
		_gear.Foreground = Ui.B("Silver");
		_gear.Content = GearIcon();
		_gear.Click += delegate
		{
			if (_simple)
			{
				ShowSimpleSettings(!_simpleSettings);
			}
			else
			{
				ShowPage<SettingsPage>();
			}
		};
		Grid.SetColumn(_gear, 5);
		g.Children.Add(_gear);

		StackPanel caps = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			Margin = new Thickness(10.0, 0.0, 0.0, 0.0),
			VerticalAlignment = VerticalAlignment.Top
		};
		caps.Children.Add(Chrome.CapButton(this, Chrome.Cap.Minimize, () => L.T("Minimize")));
		caps.Children.Add(Chrome.CapButton(this, Chrome.Cap.Maximize, () => (base.WindowState == WindowState.Maximized) ? L.T("Restore") : L.T("Maximize")));
		caps.Children.Add(Chrome.CapButton(this, Chrome.Cap.Close, () => L.T("Close to the tray")));
		Grid.SetColumn(caps, 6);
		g.Children.Add(caps);

		foreach (UIElement e in new UIElement[3] { _back, _modeBtn, _gear })
		{
			WindowChrome.SetIsHitTestVisibleInChrome(e, true);
		}
		return new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Background = Ui.B("None"),
			Child = g
		};
	}

	/// <summary>The settings gear of the mockup: a ring with eight short rays, thin strokes.</summary>
	private static UIElement GearIcon()
	{
		Geometry geo = Geometry.Parse("M10,6.5 A3.5,3.5 0 1 1 9.99,6.5 Z M10,0.5 L10,3.5 M10,16.5 L10,19.5 M0.5,10 L3.5,10 M16.5,10 L19.5,10 M3.3,3.3 L5.4,5.4 M14.6,14.6 L16.7,16.7 M3.3,16.7 L5.4,14.6 M14.6,5.4 L16.7,3.3");
		geo.Freeze();
		return new Path
		{
			Data = geo,
			Stroke = Ui.B("Silver"),
			StrokeThickness = 1.4,
			Width = 20.0,
			Height = 20.0,
			Stretch = Stretch.Uniform,
			RenderTransformOrigin = new Point(0.5, 0.5),
			LayoutTransform = new ScaleTransform(0.75, 0.75),
			HorizontalAlignment = HorizontalAlignment.Center,
			VerticalAlignment = VerticalAlignment.Center
		};
	}

	private void BuildAdvanced()
	{
		_advRoot.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(216.0)
		});
		_advRoot.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		_advRoot.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(288.0)
		});
		// The page list, numbered like the mockup, with the machine at the bottom.
		DockPanel side = new DockPanel();
		Border foot = new Border
		{
			Padding = new Thickness(18.0, 10.0, 14.0, 14.0),
			Child = _foot
		};
		DockPanel.SetDock(foot, Dock.Bottom);
		side.Children.Add(foot);
		side.Children.Add(new ScrollViewer
		{
			Content = _navList,
			VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
			HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
			Focusable = false
		});
		Border sideBox = new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 1.0, 0.0),
			Child = side
		};
		_advRoot.Children.Add(sideBox);
		Border page = new Border
		{
			Padding = new Thickness(20.0, 14.0, 20.0, 8.0),
			Child = _host
		};
		Grid.SetColumn(page, 1);
		_advRoot.Children.Add(page);
		DockPanel right = new DockPanel();
		DockPanel.SetDock(_statusBox, Dock.Top);
		right.Children.Add(_statusBox);
		right.Children.Add(_sysAdv.Root);
		Border rightBox = new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(1.0, 0.0, 0.0, 0.0),
			Background = Ui.B("Panel"),
			Padding = new Thickness(16.0, 14.0, 12.0, 12.0),
			Child = right
		};
		Grid.SetColumn(rightBox, 2);
		_advRoot.Children.Add(rightBox);
	}

	private void BuildSimple()
	{
		_simpleRoot.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(1.0, GridUnitType.Star)
		});
		_simpleRoot.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(36.0)
		});
		Geometry undo = Geometry.Parse("M1,2 L1,7 L6,7 M1.5,7 A6,6 0 1 1 3.2,10.8");
		undo.Freeze();
		_simpleUndo.Height = 26.0;
		_simpleUndo.FontSize = Ui.FsNote;
		_simpleUndo.Padding = new Thickness(10.0, 0.0, 12.0, 0.0);
		_simpleUndo.VerticalAlignment = VerticalAlignment.Center;
		_simpleUndo.BorderBrush = Ui.B("AccentLine");
		_simpleUndo.Foreground = Ui.B("Strong");
		_simpleUndo.Content = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			Children =
			{
				new Path
				{
					Data = undo,
					Stroke = Ui.B("Accent"),
					StrokeThickness = 1.6,
					Width = 14.0,
					Height = 14.0,
					Stretch = Stretch.Uniform,
					Margin = new Thickness(0.0, 0.0, 7.0, 0.0),
					VerticalAlignment = VerticalAlignment.Center
				},
				_simpleUndoText
			}
		};
		_simpleUndo.Click += delegate
		{
			UndoEverything();
		};
		DockPanel foot = new DockPanel
		{
			Margin = new Thickness(16.0, 0.0, 12.0, 0.0)
		};
		DockPanel.SetDock(_simpleUndo, Dock.Right);
		foot.Children.Add(_simpleUndo);
		foot.Children.Add(_simpleFoot);
		Border bottom = new Border
		{
			BorderBrush = Ui.B("Line"),
			BorderThickness = new Thickness(0.0, 1.0, 0.0, 0.0),
			Child = foot
		};
		Grid.SetRow(bottom, 1);
		_simpleRoot.Children.Add(_simpleHost);
		_simpleRoot.Children.Add(bottom);
		_simpleHost.Content = _home.Root;
		SimpleTexts();
	}

	private void UndoEverything()
	{
		_h.Paused = true;
		_h.Opt.Run(() => _h.Auto.RevertEverything(L.T("Button")), delegate
		{
			Did(L.T("Everything was put back"));
		});
	}

	private void SimpleTexts()
	{
		_modeText.Text = L.T("Advanced mode");
		_back.Content = "← " + L.T("Back");
		_gear.ToolTip = L.T("Settings");
		_simpleUndoText.Text = L.T("Undo everything");
		Topo topo = TopoBuilder.Live();
		List<string> parts = new List<string>
		{
			L.F("Version {0}", AppVersion.Display),
			Native.CpuName()
		};
		if (topo != null)
		{
			parts.Add(L.F("{0} cores / {1} threads", topo.PhysicalCount, topo.LogicalCount));
		}
		if (_gpuName.Length > 0)
		{
			parts.Add(_gpuName);
		}
		_simpleFoot.Text = string.Join("   ", parts.Where((string p) => !string.IsNullOrWhiteSpace(p))).ToUpper(CultureInfo.CurrentCulture);
		_simpleFoot.ToolTip = string.Join(" · ", parts);
		_foot.Text = L.F("Version {0} · user mode, no driver", AppVersion.Display) + "\n" + Native.CpuName() + ((topo != null) ? ("\n" + L.F("{0} cores / {1} threads", topo.PhysicalCount, topo.LogicalCount)) : "") + ((_gpuName.Length > 0) ? ("\n" + _gpuName) : "");
		_home.Texts();
	}

	/// <summary>Switches between the simple home screen and Advanced mode (every page); remembered in the settings.</summary>
	public void SetSimple(bool simple, bool resize = true)
	{
		_simple = simple;
		Settings.SetBool("Advanced", !simple);
		_simpleRoot.Visibility = simple ? System.Windows.Visibility.Visible : System.Windows.Visibility.Collapsed;
		_advRoot.Visibility = simple ? System.Windows.Visibility.Collapsed : System.Windows.Visibility.Visible;
		_crumb.Visibility = simple ? System.Windows.Visibility.Collapsed : System.Windows.Visibility.Visible;
		_switch.BorderBrush = Ui.B(simple ? "Dim" : "Accent");
		_knob.Background = Ui.B(simple ? "Dim" : "Accent");
		_knob.HorizontalAlignment = simple ? HorizontalAlignment.Left : HorizontalAlignment.Right;
		_knob.Margin = new Thickness(2.0, 0.0, 2.0, 0.0);
		_modeBtn.ToolTip = simple ? L.T("Advanced mode") : L.T("Simple mode");
		base.MinWidth = simple ? 860.0 : 1180.0;
		base.MinHeight = simple ? 620.0 : 700.0;
		if (resize && base.WindowState == WindowState.Normal)
		{
			base.Width = simple ? 1000.0 : 1340.0;
			base.Height = simple ? 700.0 : 840.0;
		}
		else if (!resize && simple)
		{
			base.Width = 1000.0;
			base.Height = 700.0;
		}
		if (simple)
		{
			ShowSimpleSettings(on: false);
		}
		else
		{
			_home.SetVisible(on: false);
			_back.Visibility = System.Windows.Visibility.Collapsed;
			Show(_current);
		}
		Refresh();
	}

	private void ShowSimpleSettings(bool on)
	{
		_simpleSettings = on;
		_back.Visibility = on ? System.Windows.Visibility.Visible : System.Windows.Visibility.Collapsed;
		if (on)
		{
			_home.SetVisible(on: false);
			int i = _pages.FindIndex((Page p) => p is SettingsPage);
			try
			{
				_simpleHost.Content = new Border
				{
					Padding = new Thickness(24.0, 16.0, 24.0, 6.0),
					Child = _pages[i].Build()
				};
				_pages[i].Shown();
			}
			catch (Exception ex)
			{
				AppLog.Add(L.F("Page {0}: {1}", _pages[i].Title, ex.Message), AppLog.Sev.Error);
				_simpleHost.Content = Ui.Text(ex.Message, 0.0, "Error", bold: false, wrap: true);
			}
		}
		else
		{
			_simpleHost.Content = _home.Root;
		}
	}

	private static string SectionTitle(string section)
	{
		return section switch
		{
			"Scheduler" => "DomainScheduler",
			"System" => L.T("System"),
			"Hardware" => L.T("Hardware"),
			_ => L.T("Settings"),
		};
	}

	private void Texts()
	{
		_capStatus.Text = HudMath.Spaced(L.T("Status"));
		_capSteps.Text = HudMath.Spaced(L.T("Steps done"));
		_capChanged.Text = HudMath.Spaced(L.T("Changed right now"));
		_undo.Content = L.T("Undo everything");
		_sysAdv.Texts();
		_kv.Children.Clear();
		_kv.Children.Add(Ui.Kv(L.T("Threads placed"), _nThreads));
		_kv.Children.Add(Ui.Kv(L.T("Programs lowered"), _nLowered));
		_kv.Children.Add(Ui.Kv(L.T("Programs changed"), _nChanged));
		_pillKey.Text = HudMath.Spaced(L.T("Optimization")) + " ";
		_dotState = (AutoState)(-1);
	}

	private void OnLanguage()
	{
		// Every part is rebuilt on its own: one failing part must not leave the rest (the open settings page above
		// all) in the old language until the user leaves it. The settings page goes first because it is on screen.
		base.Dispatcher.BeginInvoke((Action)delegate
		{
			void Step(string what, Action a)
			{
				try { a(); }
				catch (Exception ex) { AppLog.Add(L.F("Language switch, {0}: {1}", what, ex.Message), AppLog.Sev.Warn); }
			}
			Step("font", () => base.FontFamily = Ui.Font());
			if (_simple && _simpleSettings) Step("settings", () => ShowSimpleSettings(on: true));
			else Step("page", () => Show(_current));
			Step("texts", Texts);
			Step("home", SimpleTexts);
			Step("menu", BuildNav);
			Step("mode", () => _modeBtn.ToolTip = _simple ? L.T("Advanced mode") : L.T("Simple mode"));
			if (_simple && !_simpleSettings) Step("home", () => ShowSimpleSettings(on: false));
			Step("refresh", Refresh);
		}, System.Windows.Threading.DispatcherPriority.Background);
	}

	/// <summary>The page list of the advanced layout: section captions, then numbered pages; pages closed while Auto runs are left out.</summary>
	private void BuildNav()
	{
		_navList.Children.Clear();
		string current = _pages[_current].Section;
		int number = 0;
		foreach (string section in _pages.Select((Page p) => p.Section).Distinct())
		{
			TextBlock cap = Ui.Caps(SectionTitle(section), Ui.FsSmall * 0.9, (section == current) ? "Silver" : "Faint");
			cap.Margin = new Thickness(18.0, (number == 0) ? 10.0 : 16.0, 12.0, 6.0);
			_navList.Children.Add(cap);
			for (int i = 0; i < _pages.Count; i++)
			{
				if (_pages[i].Section != section || (_pages[i].ClosedInAuto && AutoOn))
				{
					continue;
				}
				number++;
				_navList.Children.Add(NavRow(i, number));
			}
		}
		_statusBox.Visibility = (current == "Scheduler") ? System.Windows.Visibility.Visible : System.Windows.Visibility.Collapsed;
		_crumb.Inlines.Clear();
		_crumb.Inlines.Add(new Run(HudMath.Spaced(L.T("Advanced")) + "  /  ")
		{
			Foreground = Ui.B("Dim")
		});
		_crumb.Inlines.Add(new Run(HudMath.Spaced(_pages[_current].Title))
		{
			Foreground = Ui.B("Accent")
		});
	}

	private UIElement NavRow(int index, int number)
	{
		bool on = index == _current;
		Grid row = new Grid
		{
			Height = 34.0,
			Background = Ui.B(on ? "Sel" : "None"),
			Cursor = System.Windows.Input.Cursors.Hand
		};
		row.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(46.0)
		});
		row.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		if (on)
		{
			row.Children.Add(new Border
			{
				Width = 2.0,
				Background = Ui.B("Accent"),
				HorizontalAlignment = HorizontalAlignment.Left
			});
		}
		TextBlock n = new TextBlock
		{
			Text = number.ToString("00", CultureInfo.InvariantCulture),
			FontFamily = Ui.Mono,
			FontSize = Ui.FsSmall,
			Foreground = Ui.B(on ? "Accent" : "Faint"),
			VerticalAlignment = VerticalAlignment.Center,
			Margin = new Thickness(18.0, 0.0, 0.0, 0.0)
		};
		TextBlock t = Ui.Text(_pages[index].Title, Ui.FsBody, on ? "Strong" : "Text");
		t.FontWeight = on ? FontWeights.Medium : FontWeights.Normal;
		t.Margin = new Thickness(0.0, 0.0, 10.0, 0.0);
		Grid.SetColumn(t, 1);
		row.Children.Add(n);
		row.Children.Add(t);
		if (!on)
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
			Show(index);
		};
		return row;
	}

	public void Show(int index)
	{
		if (index < 0 || index >= _pages.Count)
		{
			index = 0;
		}
		if (_pages[index].ClosedInAuto && AutoOn)
		{
			index = 0;
		}
		_current = index;
		try
		{
			_host.Content = _pages[index].Build();
			_pages[index].Shown();
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Page {0}: {1}", _pages[index].Title, ex.Message), AppLog.Sev.Error);
			_host.Content = Ui.Text(ex.Message, 0.0, "Error", bold: false, wrap: true);
		}
		BuildNav();
	}

	public void ShowPage<T>() where T : Page
	{
		if (_simple && _simpleSettings && typeof(T) == typeof(SettingsPage))
		{
			ShowSimpleSettings(on: true);
			return;
		}
		Show(_pages.FindIndex((Page p) => p is T));
	}

	public T Get<T>() where T : Page
	{
		return _pages.OfType<T>().First();
	}

	/// <summary>The status pill: "OPTIMIZATION WORKING | 00:42:17" (time since the game was optimized).</summary>
	private void Pill(AutoController auto)
	{
		AutoState s = auto.State;
		if (s != _dotState)
		{
			_dotState = s;
			string color = s switch
			{
				AutoState.Armed => "Warn",
				AutoState.Active => "Accent",
				AutoState.Reverting => "Dim",
				AutoState.Faulted => "Error",
				_ => "Faint",
			};
			_dot.Background = Ui.B(color);
			_halo.Background = Ui.B((s == AutoState.Active) ? "AccentSoft" : "None");
			_pillWord.Foreground = Ui.B((s == AutoState.Idle) ? "Dim" : color);
			_pillWord.Text = HudMath.Spaced(s switch
			{
				AutoState.Armed => L.T("Waiting for the game"),
				AutoState.Active => L.T("Working"),
				AutoState.Reverting => L.T("Putting things back"),
				AutoState.Faulted => L.T("Error"),
				_ => L.T("turned off"),
			});
			_statusPill.BorderBrush = Ui.B((s == AutoState.Active) ? "AccentLine" : "Line");
			if (s == AutoState.Active)
			{
				_activeSince = DateTime.UtcNow;
			}
		}
		string time = "";
		if (s == AutoState.Active)
		{
			DateTime since = auto.AppliedAt;
			if (since == default(DateTime) || since > DateTime.UtcNow)
			{
				since = _activeSince;
			}
			time = HudMath.Elapsed(DateTime.UtcNow - since);
		}
		string sep = (time.Length > 0) ? "  |  " : "";
		if (_pillSep.Text != sep)
		{
			_pillSep.Text = sep;
		}
		if (_pillTime.Text != time)
		{
			_pillTime.Text = time;
		}
		string tip = auto.Status;
		if (!Equals(_statusPill.ToolTip, tip))
		{
			_statusPill.ToolTip = tip;
		}
	}

	private UIElement Step(string text, bool done, bool warn = false)
	{
		Border element = new Border
		{
			Width = 14.0,
			Height = 14.0,
			CornerRadius = new CornerRadius(2.0),
			BorderThickness = new Thickness(1.0),
			Margin = new Thickness(0.0, 0.0, 9.0, 0.0),
			VerticalAlignment = VerticalAlignment.Center,
			Background = Ui.B(done ? "Accent" : "None"),
			BorderBrush = Ui.B(done ? "Accent" : (warn ? "Warn" : "Line")),
			Child = new TextBlock
			{
				Text = (done ? "✓" : (warn ? "!" : "")),
				FontFamily = Ui.Mono,
				FontSize = Ui.FsSmall * 0.85,
				Foreground = Ui.B(done ? "Ink" : "Warn"),
				HorizontalAlignment = HorizontalAlignment.Center,
				VerticalAlignment = VerticalAlignment.Center
			}
		};
		DockPanel obj = new DockPanel
		{
			Margin = new Thickness(0.0, 3.0, 0.0, 3.0)
		};
		DockPanel.SetDock(element, Dock.Left);
		obj.Children.Add(element);
		obj.Children.Add(Ui.Text(text, Ui.FsNote, done ? "Text" : (warn ? "Warn" : "Dim"), bold: false, wrap: true));
		return obj;
	}

	public void Refresh()
	{
		if (!OnScreen)
		{
			_h.NoteUiSkipped();
			return;
		}
		long timestamp = Stopwatch.GetTimestamp();
		try
		{
			RefreshCore();
		}
		finally
		{
			_h.NoteUi(Stopwatch.GetElapsedTime(timestamp).TotalMilliseconds);
		}
	}

	private void RefreshCore()
	{
		AutoController auto = _h.Auto;
		Pill(auto);
		bool autoOn = AutoOn;
		if (autoOn != _closedShown)
		{
			_closedShown = autoOn;
			if (_pages[_current].ClosedInAuto && autoOn)
			{
				Show(0);
			}
			else
			{
				BuildNav();
			}
		}
		if (_simple)
		{
			if (_simpleSettings)
			{
				try
				{
					_pages.OfType<SettingsPage>().First().Tick();
				}
				catch (Exception ex2)
				{
					AppLog.Add(L.F("Page {0}: {1}", L.T("Settings"), ex2.Message), AppLog.Sev.Error);
				}
			}
			else
			{
				_home.Tick();
			}
			return;
		}
		TextBlock stateBig = _stateBig;
		stateBig.Text = auto.State switch
		{
			AutoState.Armed => L.T("Waiting"),
			AutoState.Active => L.T("Active"),
			AutoState.Reverting => L.T("Reverting"),
			AutoState.Faulted => L.T("Error"),
			_ => L.T("Off"),
		};
		_stateBig.Foreground = Ui.B((auto.State == AutoState.Active) ? "AccentText" : ((auto.State == AutoState.Faulted) ? "Error" : "Strong"));
		_stateSub.Text = ((auto.State == AutoState.Active) ? auto.GameExe : ((auto.State == AutoState.Armed) ? L.T("Start the game and keep it in front") : L.T("Press Turn on, then start the game")));
		_toggle.Content = (auto.On ? L.T("Turn off") : L.T("Turn on"));
		_toggle.IsEnabled = auto.ButtonEnabled;
		Ui.Primary(_toggle, !auto.On);
		bool flag = auto.State == AutoState.Active;
		string blocked = auto.Blocked;
		int num = (auto.Top5Mode ? auto.Dynamic.View().Count : auto.PlanView().Count(((ThreadRole Role, uint Tid, string Name, uint LP, bool Pinned) p) => p.Pinned));
		bool flag2 = flag && auto.MaskNow != 0;
		bool flag3 = _h.Engine.BackgroundCount > 0;
		bool flag4 = flag && auto.BaselineP99 > 0.0;
		string text = $"{L.Lang}|{flag}|{flag2}|{num > 0}|{flag3}|{flag4}|{blocked}";
		if (text != _stepsKey)
		{
			_stepsKey = text;
			_steps.Children.Clear();
			_steps.Children.Add(Step(L.T("Game found"), flag));
			_steps.Children.Add(Step(L.T("Cores assigned"), flag2));
			_steps.Children.Add(Step(L.T("Threads placed"), flag && num > 0));
			_steps.Children.Add(Step(L.T("Background lowered"), flag3));
			_steps.Children.Add(Step(L.T("Frame time checked"), flag4));
			_steps.Children.Add((blocked.Length > 0) ? Step(blocked, done: false, warn: true) : Step(L.T("Nothing blocks the game"), done: false));
		}
		_nThreads.Text = num.ToString(CultureInfo.CurrentCulture);
		_nLowered.Text = _h.Engine.BackgroundCount.ToString(CultureInfo.CurrentCulture);
		_nChanged.Text = _h.Engine.ChangedPids().Count.ToString(CultureInfo.CurrentCulture);
		try
		{
			_pages[_current].Tick();
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Page {0}: {1}", _pages[_current].Title, ex.Message), AppLog.Sev.Error);
		}
	}

	public void Busy<T>(string text, Func<T> work, Action<T> done)
	{
		_overlayText.Text = text;
		_overlay.Visibility = System.Windows.Visibility.Visible;
		_h.Opt.Run(work, delegate(T r)
		{
			_overlay.Visibility = System.Windows.Visibility.Collapsed;
			try
			{
				done(r);
			}
			catch (Exception ex)
			{
				AppLog.Add(L.F("Error: {0}", AppLog.Describe(ex)), AppLog.Sev.Error);
			}
		});
	}

	public void BusyTask<T>(string text, Func<T> work, Action<T> done)
	{
		_overlayText.Text = text;
		_overlay.Visibility = System.Windows.Visibility.Visible;
		Task.Run(delegate
		{
			T r = default(T);
			try
			{
				r = work();
			}
			catch (Exception ex)
			{
				AppLog.Add(L.F("Error: {0}", AppLog.Describe(ex)), AppLog.Sev.Error);
			}
			base.Dispatcher.BeginInvoke((Action)delegate
			{
				_overlay.Visibility = System.Windows.Visibility.Collapsed;
				try
				{
					done(r);
				}
				catch (Exception ex2)
				{
					AppLog.Add(L.F("Error: {0}", AppLog.Describe(ex2)), AppLog.Sev.Error);
				}
				Refresh();
			});
		});
	}

	public void Did(string message, AppLog.Sev sev = AppLog.Sev.Info)
	{
		AppLog.Add(message, sev);
		Refresh();
	}

	protected override void OnClosing(CancelEventArgs e)
	{
		e.Cancel = true;
		SaveGeometry();
		Hide();
	}

	private void RestoreGeometry()
	{
		double result;
		bool num = double.TryParse(Settings.Get("WindowLeft"), NumberStyles.Float, CultureInfo.InvariantCulture, out result);
		double result2;
		bool flag = double.TryParse(Settings.Get("WindowTop"), NumberStyles.Float, CultureInfo.InvariantCulture, out result2);
		if (num && flag && result >= SystemParameters.VirtualScreenLeft && result2 >= SystemParameters.VirtualScreenTop && result < SystemParameters.VirtualScreenLeft + SystemParameters.VirtualScreenWidth - 100.0 && result2 < SystemParameters.VirtualScreenTop + SystemParameters.VirtualScreenHeight - 100.0)
		{
			base.Left = result;
			base.Top = result2;
		}
		else
		{
			base.WindowStartupLocation = WindowStartupLocation.CenterScreen;
		}
	}

	public void SaveGeometry()
	{
		try
		{
			if (base.WindowState == WindowState.Normal && !double.IsNaN(base.Left))
			{
				Settings.Set("WindowLeft", base.Left.ToString("0", CultureInfo.InvariantCulture));
				Settings.Set("WindowTop", base.Top.ToString("0", CultureInfo.InvariantCulture));
			}
		}
		catch (Exception)
		{
		}
	}
}
