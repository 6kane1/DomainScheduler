using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Media;
using System.Windows.Threading;

namespace DomainScheduler.App;

/// <summary>
/// The simple home screen in the HUD look (1.14.0): the game's frame time of the last minute, the processor map, the
/// Auto / Auto-tune / device optimization cards, the last lines of the log and three plain facts.
/// Nothing here runs while the window is hidden or minimized: Tick is called only by a visible window, and the 4 Hz
/// chart timer runs only while the home screen is on screen, a game is handled and the live charts are on.
/// </summary>
internal sealed class HomeView
{
	private const int Buckets = 240;

	private static readonly TimeSpan Bucket = TimeSpan.FromMilliseconds(250.0);

	/// <summary>ETW hands frames over in batches about every half second: the chart ends a second back so its right edge is complete.</summary>
	private static readonly TimeSpan Lag = TimeSpan.FromSeconds(1.0);

	private readonly Hub _h;

	private readonly Shell _s;

	// ---- frame time panel
	private readonly TextBlock _frameCap = new TextBlock
	{
		FontFamily = Ui.Mono,
		FontSize = Ui.FsSmall,
		FontWeight = FontWeights.SemiBold,
		Foreground = Ui.B("Dim"),
		TextTrimming = TextTrimming.CharacterEllipsis,
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly TextBlock _frameRight = Ui.Caps("", Ui.FsSmall, "Silver", semi: false);

	private readonly TextBlock[] _readKey = new TextBlock[5];

	private readonly TextBlock[] _readVal = new TextBlock[5];

	private readonly HudFrameChart _chart = new HudFrameChart();

	private readonly TextBlock _empty = Ui.Text("", Ui.FsBody, "Dim", bold: false, wrap: true);

	// ---- processor map
	private readonly TextBlock _mapCap = new TextBlock
	{
		FontFamily = Ui.Mono,
		FontSize = Ui.FsSmall,
		FontWeight = FontWeights.SemiBold,
		Foreground = Ui.B("Dim"),
		TextTrimming = TextTrimming.CharacterEllipsis,
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly StackPanel _legend = new StackPanel
	{
		Orientation = Orientation.Horizontal,
		VerticalAlignment = VerticalAlignment.Center
	};

	private readonly CoreMapView _map = new CoreMapView
	{
		Height = 128.0
	};

	// ---- cards
	private readonly Button _auto;

	private readonly TextBlock _autoTitle = CardTitle();

	private readonly Border _autoTag = Tag();

	private readonly TextBlock _autoSub = CardSub();

	private readonly Button _tune;

	private readonly TextBlock _tuneText = CardTitle();

	private readonly Border _tuneTag = Tag();

	private readonly TextBlock _tuneSub = CardSub();

	private readonly Border _banner;

	private readonly TextBlock _bannerText = Ui.Text("", Ui.FsNote, "Text", bold: false, wrap: true);

	private readonly Button _apply;

	private readonly Button _dismiss;

	private readonly Button _dev;

	private readonly TextBlock _devText = CardTitle();

	private readonly Border _devTag = Tag();

	private readonly TextBlock _devNote = CardSub();

	private readonly TextBlock _devBack = new TextBlock
	{
		FontSize = Ui.FsSmall,
		Cursor = System.Windows.Input.Cursors.Hand,
		HorizontalAlignment = HorizontalAlignment.Right,
		Margin = new Thickness(0.0, 5.0, 2.0, 0.0),
		TextDecorations = TextDecorations.Underline
	};

	// ---- journal




	// ---- tiles
	private readonly TextBlock _gameCap = Ui.Caps("");

	private readonly TextBlock _bgCap = Ui.Caps("");

	private readonly TextBlock _lowCap = Ui.Caps("");

	private readonly TextBlock _game = TileValue();

	private readonly TextBlock _bg = TileValue();

	private readonly TextBlock _low = TileValue();

	// ---- state
	private string _key = "";

	private bool _devDone;

	private bool _devBusy;

	private DateTime _devAppliedUtc;

	private DateTime _devAt;

	private bool _visible;

	private readonly DispatcherTimer _timer;

	private readonly List<DateTime> _times = new List<DateTime>(48000);

	private readonly double[] _bMax = new double[Buckets];

	private readonly double[] _bEnd = new double[Buckets];

	private FrameStats _stats;

	private string _ms = "ms";

	private string _lowText = "1% low";

	private ulong _mapGame;

	private ulong _mapIrq;

	private int[] _lps = Array.Empty<int>();

	private DateTime _mapAt;

	private bool _mapBusy;

	private string _mapKey = "";

	private string _maskKey = "";

	public FrameworkElement Root { get; }

	/// <summary>The "Live charts on the home screen" setting, read once and kept here (the chart asks 4 times a second).</summary>
	public static bool LiveCharts { get; set; } = Settings.GetBool("HomeCharts", fallback: true);

	private string _frameKey = "";

	public HomeView(Hub h, Shell s)
	{
		_h = h;
		_s = s;
		SelfThreads.Register(L.N("window"));
		_timer = new DispatcherTimer(DispatcherPriority.Background)
		{
			Interval = Bucket
		};
		_timer.Tick += delegate
		{
			ChartTick();
		};

		// Auto: the main switch.
		_auto = CardButton(_autoTitle, _autoTag, _autoSub);
		_auto.Click += delegate
		{
			_auto.IsEnabled = false;
			_h.Paused = false;
			_h.Opt.Queue(_h.Auto.Click, skipIfBusy: false, reports: true);
		};

		// Auto-tune, with the offer it leaves behind (the banner under it).
		_tune = CardButton(_tuneText, _tuneTag, _tuneSub);
		_tune.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
		_tune.Click += delegate
		{
			bool running = _h.Tuner.Running;
			_h.Opt.Queue(delegate
			{
				string msg = running ? null : _h.TunerStart();
				if (running)
				{
					_h.TunerStop(L.T("stopped by the user"));
				}
				_s.Dispatcher.BeginInvoke((Action)delegate
				{
					if (msg != null)
					{
						_s.Did(msg);
					}
					else
					{
						_s.Refresh();
					}
				});
			}, skipIfBusy: false, reports: true);
		};
		_apply = new Button
		{
			Height = 30.0,
			MinWidth = 100.0,
			Margin = new Thickness(0.0, 0.0, 8.0, 0.0)
		};
		Ui.Primary(_apply);
		_apply.Click += delegate
		{
			_h.Opt.Queue(delegate
			{
				_h.TunerApplyPending();
				_s.Dispatcher.BeginInvoke((Action)delegate
				{
					_s.Refresh();
				});
			}, skipIfBusy: false, reports: true);
		};
		_dismiss = new Button
		{
			Height = 30.0,
			MinWidth = 90.0
		};
		_dismiss.Click += delegate
		{
			_h.TunerDismiss();
			_s.Refresh();
		};
		_bannerText.Margin = new Thickness(0.0, 0.0, 0.0, 9.0);
		_bannerText.LineHeight = Ui.FsNote * 1.45;
		_banner = new Border
		{
			CornerRadius = new CornerRadius(Ui.Radius),
			BorderThickness = new Thickness(1.0),
			Padding = new Thickness(13.0, 10.0, 13.0, 11.0),
			Margin = new Thickness(0.0, 6.0, 0.0, 0.0),
			Visibility = Visibility.Collapsed,
			Child = new StackPanel
			{
				Children =
				{
					_bannerText,
					new StackPanel
					{
						Orientation = Orientation.Horizontal,
						Children = { _apply, _dismiss }
					}
				}
			}
		};

		// Device optimization: the same safe layout, backup and boot guard as the Devices page.
		// "Devices optimized · restart needed" is longer than the right column: two lines instead of "…".
		_devText.TextTrimming = TextTrimming.None;
		_devText.TextWrapping = TextWrapping.Wrap;
		_dev = CardButton(_devText, _devTag, _devNote);
		_dev.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
		_dev.Click += delegate
		{
			if (_devDone || _devBusy || _h.Auto.GamePid != 0)
			{
				return;
			}
			_devBusy = true;
			_dev.IsEnabled = false;
			// Pressing the button is the consent the Devices page asks for with "Allow device policy changes".
			Flags.Set(Flags.DevicesWrite, open: true);
			_s.BusyTask(L.T("Writing…"), () => IrqAuto.Apply(IrqAuto.PlanNow()), delegate(List<(string Text, bool Ok)> lines)
			{
				_devBusy = false;
				GameMask.InvalidateIrq();
				foreach (var (message, ok) in lines ?? new List<(string, bool)>())
				{
					_s.Did(message, ok ? AppLog.Sev.Info : AppLog.Sev.Error);
				}
				_s.Did(L.T("Restart Windows: the new interrupt cores start working after the restart"));
				_devAt = default;
				_mapAt = default;
				Tick();
			});
		};
		_devBack.MouseLeftButtonUp += delegate
		{
			if (_devBusy || _h.Auto.GamePid != 0)
			{
				return;
			}
			_devBusy = true;
			_s.BusyTask(L.T("Writing…"), Devices.RevertAll, delegate(string err)
			{
				_devBusy = false;
				GameMask.InvalidateIrq();
				_s.Did((err == null) ? L.T("Device policies restored from the backup; restart Windows") : L.F("Devices: {0}", err), (err != null) ? AppLog.Sev.Error : AppLog.Sev.Info);
				_devAt = default;
				_mapAt = default;
				Tick();
			});
		};
		_devBack.Foreground = Ui.B("Dim");

		Root = Build();
		Texts();
	}

	// ---- building ------------------------------------------------------------------------------------------------

	private static TextBlock CardTitle()
	{
		return new TextBlock
		{
			FontSize = Ui.FsBody * 1.08,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong"),
			TextTrimming = TextTrimming.CharacterEllipsis,
			VerticalAlignment = VerticalAlignment.Center
		};
	}

	private static TextBlock CardSub()
	{
		return new TextBlock
		{
			FontSize = Ui.FsNote,
			Foreground = Ui.B("Dim"),
			TextWrapping = TextWrapping.Wrap,
			LineHeight = Ui.FsNote * 1.4,
			Margin = new Thickness(0.0, 4.0, 0.0, 0.0)
		};
	}

	private static Border Tag()
	{
		return new Border
		{
			CornerRadius = new CornerRadius(2.0),
			BorderThickness = new Thickness(1.0),
			Padding = new Thickness(6.0, 2.0, 6.0, 2.0),
			Margin = new Thickness(8.0, 0.0, 0.0, 0.0),
			VerticalAlignment = VerticalAlignment.Center,
			Visibility = Visibility.Collapsed,
			Child = new TextBlock
			{
				FontFamily = Ui.Mono,
				FontSize = Ui.FsSmall * 0.92,
				FontWeight = FontWeights.SemiBold
			}
		};
	}

	/// <summary>The tag at the right of a card title: filled pink (on) or outlined.</summary>
	private static void SetTag(Border tag, string text, string look)
	{
		if (string.IsNullOrEmpty(text))
		{
			tag.Visibility = Visibility.Collapsed;
			return;
		}
		tag.Visibility = Visibility.Visible;
		TextBlock t = (TextBlock)tag.Child;
		t.Text = HudMath.Spaced(text);
		switch (look)
		{
		case "on":
			tag.Background = Ui.B("Accent");
			tag.BorderBrush = Ui.B("Accent");
			t.Foreground = Ui.B("Ink");
			break;
		case "accent":
			tag.Background = Ui.B("None");
			tag.BorderBrush = Ui.B("AccentLine");
			t.Foreground = Ui.B("AccentText");
			break;
		default:
			tag.Background = Ui.B("None");
			tag.BorderBrush = Ui.B("Line");
			t.Foreground = Ui.B("Silver");
			break;
		}
	}

	private static Button CardButton(TextBlock title, Border tag, TextBlock sub)
	{
		Grid head = new Grid();
		head.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		head.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = GridLength.Auto
		});
		Grid.SetColumn(tag, 1);
		head.Children.Add(title);
		head.Children.Add(tag);
		return new Button
		{
			HorizontalContentAlignment = HorizontalAlignment.Stretch,
			VerticalContentAlignment = VerticalAlignment.Top,
			Padding = new Thickness(14.0, 11.0, 14.0, 12.0),
			Background = Ui.B("Panel"),
			Content = new StackPanel
			{
				Children = { head, sub }
			}
		};
	}

	private static TextBlock TileValue()
	{
		return new TextBlock
		{
			FontFamily = Ui.Mono,
			FontSize = 18.0,
			FontWeight = FontWeights.SemiBold,
			Foreground = Ui.B("Strong"),
			Margin = new Thickness(0.0, 6.0, 0.0, 0.0),
			TextTrimming = TextTrimming.CharacterEllipsis
		};
	}

	private FrameworkElement Build()
	{
		// Left: frame time (fills), processor map, tiles. Right (270): cards and the journal (fills).
		Grid main = new Grid
		{
			Margin = new Thickness(12.0, 10.0, 12.0, 10.0)
		};
		main.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(1.0, GridUnitType.Star)
		});
		main.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(10.0)
		});
		main.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(270.0)
		});

		Grid left = new Grid();
		left.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(1.0, GridUnitType.Star),
			MinHeight = 200.0
		});
		left.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(10.0)
		});
		left.RowDefinitions.Add(new RowDefinition
		{
			Height = GridLength.Auto
		});
		left.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(10.0)
		});
		left.RowDefinitions.Add(new RowDefinition
		{
			Height = GridLength.Auto
		});
		Border frame = FramePanel();
		Border map = MapPanel();
		Grid.SetRow(map, 2);
		Grid tiles = Tiles();
		Grid.SetRow(tiles, 4);
		left.Children.Add(frame);
		left.Children.Add(map);
		left.Children.Add(tiles);
		main.Children.Add(left);

		Grid right = new Grid();
		for (int i = 0; i < 6; i++)
		{
			right.RowDefinitions.Add(new RowDefinition
			{
				Height = GridLength.Auto
			});
		}
		right.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(1.0, GridUnitType.Star)
		});
		// 1.15.0: a new version, shown only when there is one (and not during a game).
		UpdateCard update = new UpdateCard(always: false);
		Grid.SetRow(_auto, 1);
		Grid.SetRow(_tune, 2);
		Grid.SetRow(_banner, 3);
		Grid.SetRow(_dev, 4);
		Grid.SetRow(_devBack, 5);
		right.Children.Add(update.Root);
		right.Children.Add(_auto);
		right.Children.Add(_tune);
		right.Children.Add(_banner);
		right.Children.Add(_dev);
		right.Children.Add(_devBack);
		// Where the journal was: what the PC is and how Windows is set up (read once at start).
		SysPanel.Root.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
		Grid.SetRow(SysPanel.Root, 6);
		right.Children.Add(SysPanel.Root);
		Grid.SetColumn(right, 2);
		main.Children.Add(right);
		return main;
	}

	private Border FramePanel()
	{
		DockPanel cap = new DockPanel
		{
			LastChildFill = true,
			Margin = new Thickness(0.0, 0.0, 0.0, 8.0)
		};
		DockPanel.SetDock(_frameRight, Dock.Right);
		cap.Children.Add(_frameRight);
		cap.Children.Add(_frameCap);

		// 1% low big and pink, then four smaller readings sharing the rest of the width.
		Grid reads = new Grid
		{
			Margin = new Thickness(0.0, 0.0, 0.0, 6.0)
		};
		reads.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = GridLength.Auto
		});
		reads.ColumnDefinitions.Add(new ColumnDefinition
		{
			Width = new GridLength(24.0)
		});
		for (int i = 1; i < 5; i++)
		{
			reads.ColumnDefinitions.Add(new ColumnDefinition
			{
				Width = new GridLength(1.0, GridUnitType.Star)
			});
		}
		for (int i = 0; i < 5; i++)
		{
			bool big = i == 0;
			_readKey[i] = Ui.Caps("", Ui.FsSmall * 0.95, big ? "Accent" : "Dim");
			_readKey[i].HorizontalAlignment = HorizontalAlignment.Left;
			_readVal[i] = new TextBlock
			{
				FontFamily = Ui.Mono,
				FontSize = big ? 34.0 : 20.0,
				FontWeight = FontWeights.SemiBold,
				Foreground = Ui.B(big ? "Accent" : "Strong"),
				Margin = new Thickness(0.0, 2.0, 0.0, 0.0),
				TextTrimming = TextTrimming.CharacterEllipsis
			};
			StackPanel cell = new StackPanel
			{
				VerticalAlignment = VerticalAlignment.Bottom,
				Children = { _readKey[i], _readVal[i] }
			};
			Grid.SetColumn(cell, big ? 0 : (i + 1));
			reads.Children.Add(cell);
		}

		_empty.TextAlignment = TextAlignment.Center;
		_empty.HorizontalAlignment = HorizontalAlignment.Center;
		_empty.VerticalAlignment = VerticalAlignment.Center;
		_empty.MaxWidth = 420.0;
		Grid chartArea = new Grid
		{
			Margin = new Thickness(0.0, 4.0, 0.0, 0.0),
			ClipToBounds = true
		};
		chartArea.Children.Add(_chart);
		chartArea.Children.Add(_empty);

		Grid body = new Grid();
		body.RowDefinitions.Add(new RowDefinition
		{
			Height = GridLength.Auto
		});
		body.RowDefinitions.Add(new RowDefinition
		{
			Height = GridLength.Auto
		});
		body.RowDefinitions.Add(new RowDefinition
		{
			Height = new GridLength(1.0, GridUnitType.Star)
		});
		Grid.SetRow(reads, 1);
		Grid.SetRow(chartArea, 2);
		body.Children.Add(cap);
		body.Children.Add(reads);
		body.Children.Add(chartArea);
		return Ui.Panel(body, new Thickness(16.0, 13.0, 16.0, 10.0));
	}

	private Border MapPanel()
	{
		DockPanel cap = new DockPanel
		{
			LastChildFill = true,
			Margin = new Thickness(0.0, 0.0, 0.0, 10.0)
		};
		DockPanel.SetDock(_legend, Dock.Right);
		cap.Children.Add(_legend);
		cap.Children.Add(_mapCap);
		StackPanel body = new StackPanel
		{
			Children = { cap, _map }
		};
		return Ui.Panel(body, new Thickness(16.0, 13.0, 16.0, 10.0));
	}

	private Grid Tiles()
	{
		Grid g = new Grid();
		for (int i = 0; i < 5; i++)
		{
			g.ColumnDefinitions.Add(new ColumnDefinition
			{
				Width = (i % 2 == 1) ? new GridLength(10.0) : new GridLength(1.0, GridUnitType.Star)
			});
		}
		Add(_gameCap, _game, 0);
		Add(_bgCap, _bg, 2);
		Add(_lowCap, _low, 4);
		return g;
		void Add(TextBlock cap, TextBlock value, int col)
		{
			Border tile = Ui.Panel(new StackPanel
			{
				Children = { cap, value }
			}, new Thickness(14.0, 10.0, 14.0, 11.0));
			Grid.SetColumn(tile, col);
			g.Children.Add(tile);
		}
	}

	/// <summary>"This system" panel under the cards.</summary>
	public readonly SystemPanel SysPanel = new SystemPanel();

	public void Texts()
	{
		SysPanel.Texts();
		_gameCap.Text = HudMath.Spaced(L.T("Game"));
		_bgCap.Text = HudMath.Spaced(L.T("Background programs"));
		_lowCap.Text = HudMath.Spaced(L.T("Smoothness (1% low)"));
		_frameRight.Text = HudMath.Spaced(L.T("last 60 s"));
		string[] keys = new string[5]
		{
			L.T("1% low"),
			L.T("Average"),
			L.T("0.1% low"),
			L.T("Frame"),
			L.T("Hitches")
		};
		for (int i = 0; i < 5; i++)
		{
			_readKey[i].Text = HudMath.Spaced(keys[i]);
		}
		_ms = L.T("ms");
		_lowText = L.T("1% low");
		_legend.Children.Clear();
		_legend.Children.Add(LegendItem(HudInk.GameBar, L.T("game")));
		_legend.Children.Add(LegendItem(HudInk.BgBar, L.T("background")));
		_legend.Children.Add(LegendItem(HudInk.IrqBar, L.T("interrupts")));
		_apply.Content = L.T("Apply");
		_devBack.Text = L.T("Put back");
		_key = "";
		_mapKey = "";
		_frameKey = "";
		_devAt = default;
		_autoTitle.Text = L.T("Turn on Auto");
	}

	private static UIElement LegendItem(Brush b, string text)
	{
		return new StackPanel
		{
			Orientation = Orientation.Horizontal,
			Margin = new Thickness(12.0, 0.0, 0.0, 0.0),
			Children =
			{
				new Border
				{
					Width = 8.0,
					Height = 8.0,
					Background = b,
					Margin = new Thickness(0.0, 1.0, 5.0, 0.0),
					VerticalAlignment = VerticalAlignment.Center
				},
				new TextBlock
				{
					Text = text,
					FontFamily = Ui.Mono,
					FontSize = Ui.FsSmall * 0.95,
					Foreground = Ui.B("Dim"),
					VerticalAlignment = VerticalAlignment.Center
				}
			}
		};
	}

	// ---- live chart --------------------------------------------------------------------------------------------------

	/// <summary>The shell says whether the home screen is on screen; off stops the chart at once.</summary>
	public void SetVisible(bool on)
	{
		_visible = on;
		if (!on)
		{
			_timer.Stop();
		}
	}

	private bool ChartWanted()
	{
		return _visible && LiveCharts && _h.Auto.State == AutoState.Active && _h.Frames != null && _h.Frames.Available;
	}

	private void ChartTick()
	{
		if (!ChartWanted() || !_s.IsVisible || _s.WindowState == WindowState.Minimized)
		{
			_timer.Stop();
			return;
		}
		long t0 = Stopwatch.GetTimestamp();
		DateTime end = HudMath.AlignEnd(DateTime.UtcNow, Lag, Bucket);
		_h.Frames.Times(end - TimeSpan.FromTicks(Bucket.Ticks * Buckets) - TimeSpan.FromSeconds(2.0), _times);
		HudMath.Buckets(_times, end, Bucket, _bMax, _bEnd);
		_chart.Set(_bMax, _bEnd, _stats.Valid ? _stats.MedianMs : 0.0, _stats.Valid ? _stats.P99Ms : 0.0, _ms, _lowText);
		_h.NoteChart(Stopwatch.GetElapsedTime(t0).TotalMilliseconds);
	}

	// ---- the 1 s update (only while the window is visible) ---------------------------------------------------------

	public void Tick()
	{
		_visible = true;
		AutoController auto = _h.Auto;
		AutoState state = auto.State;
		bool active = state == AutoState.Active;
		bool framesOk = _h.Frames != null && _h.Frames.Available;
		AutoCard(auto, state, active);
		TuneCard(active, framesOk);
		DeviceButton();
		Frame(auto, active, framesOk);
		Map(auto);

		_game.Text = active ? auto.GameExe : (auto.On ? L.T("waiting") : L.T("not handled"));
		_game.Foreground = Ui.B(active ? "Strong" : "Dim");
		int bg = _h.Engine.BackgroundCount;
		if (bg > 0)
		{
			Ui.SetValue(_bg, bg.ToString(CultureInfo.CurrentCulture), L.T("quieted"));
		}
		else
		{
			Ui.SetValue(_bg, L.T("untouched"), null, "Dim");
		}
		if (active && framesOk && _stats.Valid)
		{
			Ui.SetValue(_low, _stats.Low1Fps.ToString("0", CultureInfo.CurrentCulture), L.T("FPS"), "Accent");
		}
		else
		{
			Ui.SetValue(_low, "—", null, "Dim");
		}
	}

	private void AutoCard(AutoController auto, AutoState state, bool active)
	{
		string blocked = auto.Blocked;
		string sub = state switch
		{
			AutoState.Armed => L.T("Start the game and keep it in front: the program finds it by itself."),
			AutoState.Active => L.F("{0}: the game has the best cores, background programs are quieter. Everything comes back when the game closes.", auto.GameExe),
			AutoState.Faulted => L.T("Something went wrong: everything was put back. Open Advanced mode to see the activity log."),
			AutoState.Reverting => L.T("Putting everything back as it was…"),
			_ => L.T("Turn on Auto: the program finds the game by itself, gives it the best cores and quiets background programs. Everything comes back when the game closes."),
		};
		if (blocked.Length > 0 && auto.On)
		{
			sub = sub + "\n" + blocked;
		}
		_autoSub.Text = sub;
		_autoSub.Foreground = Ui.B((state == AutoState.Faulted) ? "Error" : "Dim");
		_auto.IsEnabled = auto.ButtonEnabled;
		_auto.ToolTip = auto.On ? L.T("Turn off Auto") : L.T("Turn on Auto");
		SetTag(_autoTag, auto.On ? L.T("ON") : L.T("OFF"), auto.On ? "on" : "plain");
		_auto.Background = auto.On ? OnGlow : Ui.B("Panel");
		_auto.BorderBrush = Ui.B(auto.On ? "Accent" : "Line");
	}

	private static readonly Brush OnGlow = MakeGlow();

	private static Brush MakeGlow()
	{
		LinearGradientBrush b = new LinearGradientBrush(Color.FromArgb(41, 244, 182, 203), Color.FromArgb(5, 244, 182, 203), 0.0);
		b.Freeze();
		return b;
	}

	private void TuneCard(bool active, bool framesOk)
	{
		AbTuner tuner = _h.Tuner;
		if (tuner.Running)
		{
			_tuneText.Text = L.T("Stop auto-tune");
			int left = (int)Math.Ceiling((tuner.StepCount - tuner.StepIndex) * (AbTuner.PhaseSeconds + AbTuner.LagSeconds) / 60.0);
			_tuneSub.Text = L.F("Step {0} of {1} · about {2} min left · keep playing", tuner.StepIndex + 1, tuner.StepCount, left);
			SetTag(_tuneTag, string.Format(CultureInfo.InvariantCulture, "{0}/{1}", tuner.StepIndex + 1, tuner.StepCount), "accent");
			_tune.IsEnabled = true;
		}
		else
		{
			_tuneText.Text = L.T("Auto-tune");
			_tuneSub.Text = !framesOk ? L.T("frame time cannot be measured here") : (active ? L.T("about 8 minutes while you play") : L.T("turn on Auto and start the game"));
			SetTag(_tuneTag, null, null);
			_tune.IsEnabled = active && framesOk;
		}
		_tune.Background = Ui.B(tuner.Running ? "AccentSoft" : "Panel");
		_tune.BorderBrush = Ui.B(tuner.Running ? "AccentLine" : "Line");
		_tuneText.Foreground = Ui.B(tuner.Running ? "AccentText" : "Strong");

		TunerVariant pending = _h.TunerPending;
		bool noGain = _h.TunerNoGain && !tuner.Running;
		string key = $"{L.Lang}|{pending?.Name}|{noGain}|{tuner.Running}";
		if (key == _key)
		{
			return;
		}
		_key = key;
		if (pending != null && !tuner.Running)
		{
			_banner.Visibility = Visibility.Visible;
			_banner.Background = Ui.B("AccentSoft");
			_banner.BorderBrush = Ui.B("AccentLine");
			_bannerText.Text = _h.TunerOffer + "\n" + L.T("Apply the new values? The current ones stay until you press Apply.");
			_apply.Visibility = Visibility.Visible;
			_dismiss.Content = L.T("Not now");
		}
		else if (noGain)
		{
			_banner.Visibility = Visibility.Visible;
			_banner.Background = Ui.B("Panel");
			_banner.BorderBrush = Ui.B("Line");
			_bannerText.Text = L.T("Auto-tune finished: the current settings were the best, nothing needs to change.");
			_apply.Visibility = Visibility.Collapsed;
			_dismiss.Content = L.T("OK");
		}
		else
		{
			_banner.Visibility = Visibility.Collapsed;
		}
	}

	/// <summary>The device optimization card: offered, blocked by a game, applied (restart needed until the next boot) or active.</summary>
	private void DeviceButton()
	{
		if ((DateTime.UtcNow - _devAt).TotalSeconds >= 3.0)
		{
			_devAt = DateTime.UtcNow;
			_devDone = IrqAuto.HasBackup;
			_devAppliedUtc = _devDone ? BootGuard.PendingAppliedUtc() : default;
		}
		bool game = _h.Auto.GamePid != 0;
		BootGuard.HomeDevices st = BootGuard.HomeState(_devDone, game, _devAppliedUtc, Reservation.BootUtc());
		if (_devDone)
		{
			_devText.Text = st == BootGuard.HomeDevices.AppliedRestartNeeded ? L.T("Devices optimized · restart needed") : L.T("Devices optimized");
			_dev.BorderBrush = Ui.B("AccentLine");
			_devText.Foreground = Ui.B("AccentText");
			_dev.IsHitTestVisible = false;
			_dev.IsEnabled = true;
			SetTag(_devTag, null, null);
		}
		else
		{
			_devText.Text = L.T("Optimize devices");
			_dev.BorderBrush = Ui.B("Line");
			_devText.Foreground = Ui.B(game ? "Dim" : "Strong");
			_dev.IsHitTestVisible = true;
			_dev.IsEnabled = !game && !_devBusy;
			SetTag(_devTag, (st == BootGuard.HomeDevices.Offer) ? L.T("once") : null, "plain");
		}
		string note = st switch
		{
			BootGuard.HomeDevices.Offer => L.T("Press once, then restart your PC — the changes apply after a restart."),
			BootGuard.HomeDevices.BlockedByGame => L.T("Not while a game runs: close the game first."),
			BootGuard.HomeDevices.AppliedRestartNeeded => L.T("Restart your PC so the changes apply."),
			_ => null,
		};
		_devNote.Text = note ?? "";
		_devNote.Visibility = (note == null) ? Visibility.Collapsed : Visibility.Visible;
		_devNote.Foreground = Ui.B((st == BootGuard.HomeDevices.Offer) ? "Dim" : "Warn");
		_devBack.Visibility = (_devDone && !game) ? Visibility.Visible : Visibility.Collapsed;
	}

	private void Frame(AutoController auto, bool active, bool framesOk)
	{
		string exe = active ? (auto.GameExe ?? "") : "";
		string frameKey = L.Lang + "|" + exe;
		if (frameKey != _frameKey)
		{
			_frameKey = frameKey;
			_frameCap.Inlines.Clear();
			_frameCap.Inlines.Add(new Run("▍ ")
			{
				Foreground = Ui.B("Accent")
			});
			_frameCap.Inlines.Add(new Run(HudMath.Spaced(L.T("Frame time"))));
			if (exe.Length > 0)
			{
				_frameCap.Inlines.Add(new Run("  ·  " + exe)
				{
					Foreground = Ui.B("Silver"),
					FontWeight = FontWeights.Medium
				});
			}
		}
		bool measure = active && framesOk;
		_stats = default;
		if (measure)
		{
			DateTime now = DateTime.UtcNow;
			_stats = HudMath.Stats(_h.Frames.Intervals(now.AddSeconds(-60.0), now));
		}
		string fps = L.T("FPS");
		if (_stats.Valid)
		{
			Ui.SetValue(_readVal[0], _stats.Low1Fps.ToString("0", CultureInfo.CurrentCulture), fps, "Accent");
			Ui.SetValue(_readVal[1], _stats.AvgFps.ToString("0", CultureInfo.CurrentCulture), fps);
			Ui.SetValue(_readVal[2], (_stats.Low01Fps > 0.0) ? _stats.Low01Fps.ToString("0", CultureInfo.CurrentCulture) : "—", (_stats.Low01Fps > 0.0) ? fps : null, (_stats.Low01Fps > 0.0) ? "Strong" : "Dim");
			Ui.SetValue(_readVal[3], _stats.FrameMs.ToString((_stats.FrameMs < 10.0) ? "0.00" : "0.0", CultureInfo.CurrentCulture), _ms);
			Ui.SetValue(_readVal[4], _stats.HitchesPerMin.ToString("0", CultureInfo.CurrentCulture), L.T("per min"), (_stats.HitchesPerMin >= 1.0) ? "Warn" : "Strong");
		}
		else
		{
			for (int i = 0; i < 5; i++)
			{
				Ui.SetValue(_readVal[i], "—", null, "Faint");
			}
		}
		bool charts = LiveCharts;
		bool chartOn = measure && charts;
		_chart.Visibility = chartOn ? Visibility.Visible : Visibility.Hidden;
		if (!framesOk)
		{
			_empty.Text = L.T("frame time cannot be measured here");
		}
		else if (!active)
		{
			_empty.Text = L.T("Start a game — its frame time appears here");
		}
		else if (!charts)
		{
			_empty.Text = L.T("Live charts are off in the settings");
		}
		else
		{
			_empty.Text = "";
		}
		_empty.Visibility = (_empty.Text.Length > 0) ? Visibility.Visible : Visibility.Collapsed;
		if (chartOn && ChartWanted())
		{
			if (!_timer.IsEnabled)
			{
				_timer.Start();
				ChartTick();
			}
		}
		else
		{
			_timer.Stop();
			if (!chartOn)
			{
				_chart.Clear();
			}
		}
	}

	private void Map(AutoController auto)
	{
		Topo topo = TopoBuilder.Live();
		if (topo == null)
		{
			return;
		}
		string key = $"{L.Lang}|{topo.PhysicalCount}|{topo.LogicalCount}";
		if (key != _mapKey)
		{
			_mapKey = key;
			_mapCap.Inlines.Clear();
			_mapCap.Inlines.Add(new Run("▍ ")
			{
				Foreground = Ui.B("Accent")
			});
			_mapCap.Inlines.Add(new Run(HudMath.Spaced(L.T("CPU map")) + "  ·  " + HudMath.Spaced(L.F("{0} cores / {1} threads", topo.PhysicalCount, topo.LogicalCount))));
			_lps = topo.Sets.Where((CpuSet s) => s.Group == 0 && s.LP < 64).Select((CpuSet s) => (int)s.LP).OrderBy((int x) => x).ToArray();
		}
		// The masks are read off the UI thread (the interrupt pins come from the registry): when Auto's state or mask
		// changed, otherwise every 30 s (every 5 s while a game runs: the live interrupt cores move).
		ulong now = auto.MaskNow;
		string maskKey = auto.State + "|" + now.ToString(CultureInfo.InvariantCulture);
		if (!_mapBusy && (maskKey != _maskKey || (DateTime.UtcNow - _mapAt).TotalSeconds >= ((auto.State == AutoState.Active) ? 5.0 : 30.0)))
		{
			_mapBusy = true;
			_mapAt = DateTime.UtcNow;
			_maskKey = maskKey;
			ulong fallback = _h.GameMaskValue;
			Task.Run(delegate
			{
				ulong game = now;
				ulong irq = 0uL;
				try
				{
					GameMaskInfo info = Hub.MaskInfo(topo);
					if (game == 0)
					{
						game = (info.Final != 0) ? info.Final : fallback;
					}
					irq = info.IrqDrop;
					ulong? busy = GameMask.BusyHook?.Invoke(topo);
					irq |= busy.GetValueOrDefault();
				}
				catch (Exception)
				{
					if (game == 0)
					{
						game = fallback;
					}
				}
				_s.Dispatcher.BeginInvoke((Action)delegate
				{
					_mapGame = game;
					_mapIrq = irq;
					_mapBusy = false;
				});
			});
		}
		double[] cpu = _h.CpuLoad;
		double[] load = new double[_lps.Length];
		CoreKind[] kinds = new CoreKind[_lps.Length];
		for (int i = 0; i < _lps.Length; i++)
		{
			int lp = _lps[i];
			load[i] = (lp < cpu.Length) ? cpu[lp] : 0.0;
			kinds[i] = HudMath.Classify(lp, _mapGame, _mapIrq);
		}
		_map.Set(_lps, load, kinds);
	}


}
