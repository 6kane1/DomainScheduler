using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;

namespace DomainScheduler.App;

internal static class Ui
{
	// The HUD palette (1.14.0, "Cockpit"): near-black, thin lines, pink accent, amber for interrupts and warnings.
	private static readonly Dictionary<string, Color> Palette = new Dictionary<string, Color>
	{
		["Bg"] = Color.FromRgb(6, 6, 8),
		["Pop"] = Color.FromRgb(19, 20, 25),
		["Ink"] = Color.FromRgb(42, 24, 32),
		["Panel"] = Color.FromRgb(14, 15, 19),
		["Panel2"] = Color.FromArgb(10, byte.MaxValue, byte.MaxValue, byte.MaxValue),
		["Well"] = Color.FromArgb(90, 0, 0, 0),
		["Line"] = Color.FromRgb(34, 35, 42),
		["Line2"] = Color.FromRgb(23, 24, 29),
		["Hover"] = Color.FromArgb(14, 244, 182, 203),
		["Sel"] = Color.FromArgb(30, 244, 182, 203),
		["Strong"] = Color.FromRgb(230, 232, 236),
		["Text"] = Color.FromRgb(201, 206, 214),
		["Dim"] = Color.FromRgb(125, 131, 141),
		["Faint"] = Color.FromRgb(93, 99, 108),
		["Silver"] = Color.FromRgb(201, 206, 214),
		["Steel"] = Color.FromRgb(143, 147, 160),
		["Accent"] = Color.FromRgb(244, 182, 203),
		["AccentText"] = Color.FromRgb(247, 198, 214),
		["AccentSoft"] = Color.FromArgb(33, 244, 182, 203),
		["AccentLine"] = Color.FromArgb(90, 244, 182, 203),
		["Bracket"] = Color.FromArgb(204, 244, 182, 203),
		["On"] = Color.FromRgb(244, 182, 203),
		["Warn"] = Color.FromRgb(232, 194, 122),
		["WarnSoft"] = Color.FromArgb(22, 232, 194, 122),
		["WarnLine"] = Color.FromArgb(64, 232, 194, 122),
		["Error"] = Color.FromRgb(239, 138, 138),
		["CloseHover"] = Color.FromRgb(122, 46, 54),
		["Game"] = Color.FromArgb(92, 244, 182, 203),
		["NonGame"] = Color.FromArgb(56, 143, 147, 160),
		["Irq"] = Color.FromRgb(232, 194, 122),
		["Overlay"] = Color.FromArgb(200, 6, 6, 8),
		["None"] = Color.FromArgb(0, 0, 0, 0),
		["HazePink"] = Color.FromArgb(33, 244, 182, 203),
		["HazeSilver"] = Color.FromArgb(26, 190, 198, 214)
	};

	private static readonly Dictionary<string, SolidColorBrush> Brushes = new Dictionary<string, SolidColorBrush>();

	public static readonly double FsSmall = Pt(8.5);

	public static readonly double FsNote = Pt(8.75);

	public static readonly double FsBody = Pt(10.0);

	public static readonly double FsHero = Pt(48.0);

	public static readonly Thickness MTop = new Thickness(0.0, 15.0, 0.0, 0.0);

	public static readonly Thickness MCell = new Thickness(0.0, 1.0, 6.0, 0.0);

	public static readonly Thickness MRow = new Thickness(0.0, 0.0, 0.0, 6.0);

	public const double RowH = 32.0;

	public const double Gap = 12.0;

	public const double IconScale = 0.46;

	public const double Radius = 4.0;

	/// <summary>The embedded fonts (SIL OFL, Fonts/*.ttf as WPF resources); null when they could not be found.</summary>
	private static readonly Uri FontBase = MakeBase();

	private static readonly bool Embedded = HasEmbeddedFonts();

	/// <summary>True when the embedded Inter / JetBrains Mono fonts were found (else the system fonts are used).</summary>
	public static bool EmbeddedFonts => Embedded;

	private static Uri MakeBase()
	{
		try
		{
			return new Uri("pack://application:,,,/");
		}
		catch (Exception)
		{
			return null;
		}
	}

	/// <summary>JetBrains Mono for numbers and captions; Inter covers the narrow spaces of the captions; then the system.</summary>
	public static readonly FontFamily Mono = MakeMono();

	private static bool HasEmbeddedFonts()
	{
		try
		{
			return FontBase != null && Fonts.GetFontFamilies(FontBase, "./Fonts/").Count > 0;
		}
		catch (Exception)
		{
			return false;
		}
	}

	private static FontFamily MakeMono()
	{
		try
		{
			return Embedded ? new FontFamily(FontBase, "./Fonts/#JetBrains Mono, ./Fonts/#Inter, Consolas, Courier New") : new FontFamily("Consolas, Courier New");
		}
		catch (Exception)
		{
			return new FontFamily("Consolas");
		}
	}

	public static Brush GameC => B("Game");

	public static Brush NonGameC => B("NonGame");

	public static SolidColorBrush B(string key)
	{
		if (Brushes.TryGetValue(key, out var value))
		{
			return value;
		}
		value = new SolidColorBrush(Palette.TryGetValue(key, out var value2) ? value2 : Palette["Text"]);
		value.Freeze();
		return Brushes[key] = value;
	}

	private static double Pt(double pt)
	{
		return pt * 96.0 / 72.0;
	}

	public static Color C(string key)
	{
		if (!Palette.TryGetValue(key, out var value))
		{
			return Palette["Text"];
		}
		return value;
	}

	public static Brush Backdrop(bool pink)
	{
		RadialGradientBrush radialGradientBrush = new RadialGradientBrush();
		radialGradientBrush.GradientOrigin = (pink ? new Point(0.1, 0.0) : new Point(1.0, 1.0));
		radialGradientBrush.Center = (pink ? new Point(0.1, 0.0) : new Point(1.0, 1.0));
		radialGradientBrush.RadiusX = 0.75;
		radialGradientBrush.RadiusY = 0.75;
		radialGradientBrush.GradientStops.Add(new GradientStop(C(pink ? "HazePink" : "HazeSilver"), 0.0));
		radialGradientBrush.GradientStops.Add(new GradientStop(C("None"), 1.0));
		radialGradientBrush.Freeze();
		return radialGradientBrush;
	}

	public static Button Primary(Button b, bool on = true)
	{
		b.Background = B(on ? "Accent" : "Panel2");
		b.Foreground = B(on ? "Ink" : "Text");
		b.BorderBrush = B(on ? "Accent" : "Line");
		if (on)
		{
			b.FontWeight = FontWeights.SemiBold;
		}
		return b;
	}

	public static Border Pill(string text, bool accent)
	{
		return new Border
		{
			CornerRadius = new CornerRadius(2.0),
			Padding = new Thickness(6.0, 2.0, 6.0, 2.0),
			HorizontalAlignment = HorizontalAlignment.Left,
			Background = B(accent ? "AccentSoft" : "Panel2"),
			BorderBrush = B(accent ? "AccentLine" : "Line"),
			BorderThickness = new Thickness(1.0),
			Child = new TextBlock
			{
				Text = HudMath.Spaced(text),
				FontSize = FsSmall * 0.95,
				FontFamily = Mono,
				FontWeight = FontWeights.Medium,
				Foreground = B(accent ? "AccentText" : "Text"),
				TextTrimming = TextTrimming.CharacterEllipsis
			}
		};
	}

	public static UIElement Kv(string k, TextBlock v)
	{
		DockPanel dockPanel = new DockPanel
		{
			Margin = new Thickness(0.0, 0.0, 0.0, 0.0)
		};
		// The value block is reused when the texts are rebuilt (language switch): take it out of its old row first,
		// or WPF refuses ("already the logical child of another element").
		if (v.Parent is Panel old) old.Children.Remove(v);
		v.FontFamily = Mono;
		v.Foreground = B("Strong");
		DockPanel.SetDock(v, Dock.Right);
		dockPanel.Children.Add(v);
		dockPanel.Children.Add(Text(k));
		return new Border
		{
			BorderBrush = B("Line"),
			BorderThickness = new Thickness(0.0, 0.0, 0.0, 1.0),
			Padding = new Thickness(0.0, 8.0, 0.0, 8.0),
			Child = dockPanel
		};
	}

	public static void SetBar(FrameworkElement bar, double share)
	{
		if (bar is Grid grid && grid.ColumnDefinitions.Count == 2)
		{
			share = Math.Clamp(share, 0.0, 1.0);
			grid.ColumnDefinitions[0].Width = new GridLength(share, GridUnitType.Star);
			grid.ColumnDefinitions[1].Width = new GridLength(1.0 - share, GridUnitType.Star);
		}
	}

	public static FrameworkElement Bar(double share, bool accent, double height = 3.0)
	{
		Grid obj = new Grid
		{
			Height = height,
			ColumnDefinitions = 
			{
				new ColumnDefinition
				{
					Width = new GridLength(Math.Clamp(share, 0.0, 1.0), GridUnitType.Star)
				},
				new ColumnDefinition
				{
					Width = new GridLength(1.0 - Math.Clamp(share, 0.0, 1.0), GridUnitType.Star)
				}
			}
		};
		Border element = new Border
		{
			Background = B("Line")
		};
		Grid.SetColumnSpan(element, 2);
		obj.Children.Add(element);
		obj.Children.Add(new Border
		{
			Background = B(accent ? "Accent" : "Steel"),
			MinWidth = ((share > 0.0) ? height : 0.0)
		});
		return obj;
	}

	public static void Register(ResourceDictionary res)
	{
		foreach (string key in Palette.Keys)
		{
			res[key] = B(key);
		}
	}

	public static FontFamily Font()
	{
		string text = string.Join(", ", L.Fonts);
		try
		{
			if (Embedded)
			{
				return new FontFamily(FontBase, "./Fonts/#Inter" + ((text.Length > 0) ? (", " + text) : ", Segoe UI"));
			}
			return (text.Length > 0) ? new FontFamily(text) : SystemFonts.MessageFontFamily;
		}
		catch (Exception)
		{
			return SystemFonts.MessageFontFamily;
		}
	}

	public static string Hex(ulong m)
	{
		return "0x" + m.ToString("X", CultureInfo.InvariantCulture);
	}

	public static string Size(long bytes)
	{
		if (bytes < 1073741824)
		{
			if (bytes < 1048576)
			{
				return ((double)bytes / 1024.0).ToString("0", CultureInfo.CurrentCulture) + " KB";
			}
			return ((double)bytes / 1048576.0).ToString("0.0", CultureInfo.CurrentCulture) + " MB";
		}
		return ((double)bytes / 1073741824.0).ToString("0.0", CultureInfo.CurrentCulture) + " GB";
	}

	public static TextBlock Text(string s, double size = 0.0, string brush = "Text", bool bold = false, bool wrap = false)
	{
		return new TextBlock
		{
			Text = (s ?? ""),
			FontSize = ((size > 0.0) ? size : FsBody),
			Foreground = B(brush),
			FontWeight = (bold ? FontWeights.SemiBold : FontWeights.Normal),
			TextWrapping = ((!wrap) ? TextWrapping.NoWrap : TextWrapping.Wrap),
			TextTrimming = ((!wrap) ? TextTrimming.CharacterEllipsis : TextTrimming.None),
			VerticalAlignment = VerticalAlignment.Center
		};
	}

	/// <summary>A panel caption in the HUD style: a pink bar, then spaced capitals in the mono font.</summary>
	public static TextBlock Head(string s)
	{
		TextBlock t = new TextBlock
		{
			FontSize = FsSmall,
			FontFamily = Mono,
			FontWeight = FontWeights.SemiBold,
			Foreground = B("Dim"),
			Margin = new Thickness(0.0, 0.0, 0.0, 10.0),
			TextTrimming = TextTrimming.CharacterEllipsis
		};
		SetHead(t, s);
		return t;
	}

	/// <summary>Sets the text of a caption made by Head (the bar and the capitals stay).</summary>
	public static void SetHead(TextBlock t, string s)
	{
		t.Inlines.Clear();
		t.Inlines.Add(new System.Windows.Documents.Run("▍ ")
		{
			Foreground = B("Accent")
		});
		t.Inlines.Add(new System.Windows.Documents.Run(HudMath.Spaced(s)));
	}

	/// <summary>Spaced capitals in the mono font (captions, tiles, the header).</summary>
	public static TextBlock Caps(string s, double size = 0.0, string brush = "Dim", bool semi = true)
	{
		return new TextBlock
		{
			Text = HudMath.Spaced(s),
			FontFamily = Mono,
			FontSize = ((size > 0.0) ? size : FsSmall),
			FontWeight = (semi ? FontWeights.SemiBold : FontWeights.Medium),
			Foreground = B(brush),
			TextTrimming = TextTrimming.CharacterEllipsis,
			VerticalAlignment = VerticalAlignment.Center
		};
	}

	/// <summary>A big mono number with a small unit after it ("436 FPS").</summary>
	public static void SetValue(TextBlock t, string value, string unit, string brush = "Strong")
	{
		// Called every second with mostly the same text: rebuild (and re-measure) only when something changed.
		string key = value + "\u0001" + unit + "\u0001" + brush + "\u0001" + t.FontSize.ToString(CultureInfo.InvariantCulture);
		if (t.Tag is string old && old == key)
		{
			return;
		}
		t.Tag = key;
		t.Inlines.Clear();
		t.Inlines.Add(new System.Windows.Documents.Run(value ?? "")
		{
			Foreground = B(brush)
		});
		if (!string.IsNullOrEmpty(unit))
		{
			t.Inlines.Add(new System.Windows.Documents.Run(" " + unit)
			{
				FontSize = Math.Max(FsSmall, t.FontSize * 0.42),
				FontWeight = FontWeights.Medium,
				Foreground = B("Dim")
			});
		}
	}

	public static TextBlock Note(string s)
	{
		return Text(s, FsNote, "Dim", bold: false, wrap: true);
	}

	public static Button Btn(string text, Action click, double minWidth = 96.0)
	{
		Button button = new Button();
		button.Content = text;
		button.MinWidth = minWidth;
		button.Height = 30.0;
		button.Margin = new Thickness(0.0, 0.0, 6.0, 0.0);
		button.FontSize = FsBody;
		button.Click += delegate
		{
			click?.Invoke();
		};
		return button;
	}

	public static Button ConfirmBtn(string text, Action act)
	{
		Button b = new Button
		{
			Content = text,
			MinWidth = 96.0,
			Height = 30.0,
			Margin = new Thickness(0.0, 0.0, 6.0, 0.0),
			FontSize = FsBody
		};
		bool armed = false;
		object fg0 = null;
		object bg0 = null;
		object bd0 = null;
		DispatcherTimer timer = new DispatcherTimer
		{
			Interval = TimeSpan.FromSeconds(4.0)
		};
		timer.Tick += delegate
		{
			Disarm();
		};
		b.Click += delegate
		{
			if (!armed)
			{
				armed = true;
				fg0 = b.ReadLocalValue(Control.ForegroundProperty);
				bg0 = b.ReadLocalValue(Control.BackgroundProperty);
				bd0 = b.ReadLocalValue(Control.BorderBrushProperty);
				b.Content = L.T("Click again to confirm");
				b.Foreground = B("Warn");
				b.Background = B("WarnSoft");
				b.BorderBrush = B("Warn");
				timer.Start();
			}
			else
			{
				Disarm();
				act?.Invoke();
			}
		};
		b.Unloaded += delegate
		{
			timer.Stop();
		};
		return b;
		void Disarm()
		{
			timer.Stop();
			armed = false;
			b.Content = text;
			Restore(Control.ForegroundProperty, fg0);
			Restore(Control.BackgroundProperty, bg0);
			Restore(Control.BorderBrushProperty, bd0);
		}
		void Restore(DependencyProperty p, object v)
		{
			if (v is Brush brush)
			{
				b.SetValue(p, brush);
			}
			else
			{
				b.ClearValue(p);
			}
		}
	}

	/// <summary>A HUD panel: dark fill, a thin line, pink corner brackets top-left and bottom-right. Callers only set
	/// the Margin of the returned border.</summary>
	public static Border Card(UIElement child, double pad = 12.0)
	{
		Border border = Panel(child, new Thickness(pad + 3.0, pad + 1.0, pad + 3.0, pad + 1.0));
		border.Margin = new Thickness(0.0, 0.0, 0.0, 10.0);
		return border;
	}

	private static Geometry _bracketTl;

	private static Geometry _bracketBr;

	private static Pen _bracketPen;

	/// <summary>The panel without a margin, with the padding given.</summary>
	public static Border Panel(UIElement child, Thickness pad, bool brackets = true)
	{
		Grid grid = new Grid();
		grid.Children.Add(new Border
		{
			Background = B("Panel"),
			BorderBrush = B("Line"),
			BorderThickness = new Thickness(1.0),
			CornerRadius = new CornerRadius(Radius),
			Padding = pad,
			Child = child
		});
		if (brackets)
		{
			grid.Children.Add(Bracket(top: true));
			grid.Children.Add(Bracket(top: false));
		}
		return new Border
		{
			Child = grid
		};
	}

	/// <summary>A 10 px corner bracket drawn over the panel's line (not hit-testable).</summary>
	public static FrameworkElement Bracket(bool top)
	{
		if (_bracketPen == null)
		{
			_bracketPen = new Pen(B("Bracket"), 1.0);
			_bracketPen.Freeze();
			_bracketTl = Geometry.Parse("M0.5,10 L0.5,0.5 L10,0.5");
			_bracketTl.Freeze();
			_bracketBr = Geometry.Parse("M0,9.5 L9.5,9.5 L9.5,0");
			_bracketBr.Freeze();
		}
		DrawingGroup drawing = new DrawingGroup();
		drawing.Children.Add(new GeometryDrawing(null, _bracketPen, top ? _bracketTl : _bracketBr));
		drawing.Freeze();
		return new Image
		{
			Source = new DrawingImage(drawing),
			Width = 10.0,
			Height = 10.0,
			Stretch = Stretch.None,
			IsHitTestVisible = false,
			SnapsToDevicePixels = true,
			HorizontalAlignment = (top ? HorizontalAlignment.Left : HorizontalAlignment.Right),
			VerticalAlignment = (top ? VerticalAlignment.Top : VerticalAlignment.Bottom)
		};
	}

	private static ImageBrush _grid;

	/// <summary>The window background: near-black with a faint 24 px grid (one frozen 24x24 tile, cheap to draw).</summary>
	public static Brush GridBackground()
	{
		if (_grid != null)
		{
			return _grid;
		}
		try
		{
			const int n = 24;
			Color bg = C("Bg");
			Color line = Color.FromRgb(11, 11, 14);
			byte[] px = new byte[n * n * 4];
			for (int y = 0; y < n; y++)
			{
				for (int x = 0; x < n; x++)
				{
					Color c = (x == 0 || y == 0) ? line : bg;
					int i = (y * n + x) * 4;
					px[i] = c.B;
					px[i + 1] = c.G;
					px[i + 2] = c.R;
					px[i + 3] = byte.MaxValue;
				}
			}
			BitmapSource bmp = BitmapSource.Create(n, n, 96.0, 96.0, PixelFormats.Bgra32, null, px, n * 4);
			bmp.Freeze();
			ImageBrush brush = new ImageBrush(bmp)
			{
				TileMode = TileMode.Tile,
				Viewport = new Rect(0.0, 0.0, n, n),
				ViewportUnits = BrushMappingMode.Absolute,
				Stretch = Stretch.None
			};
			brush.Freeze();
			_grid = brush;
			return brush;
		}
		catch (Exception)
		{
			return B("Bg");
		}
	}

	public static StackPanel Row(params UIElement[] items)
	{
		StackPanel stackPanel = new StackPanel
		{
			Orientation = Orientation.Horizontal,
			Margin = MRow
		};
		foreach (UIElement uIElement in items)
		{
			if (uIElement != null)
			{
				stackPanel.Children.Add(uIElement);
			}
		}
		return stackPanel;
	}

	public static StackPanel Col(params UIElement[] items)
	{
		StackPanel stackPanel = new StackPanel();
		foreach (UIElement uIElement in items)
		{
			if (uIElement != null)
			{
				stackPanel.Children.Add(uIElement);
			}
		}
		return stackPanel;
	}

	public static ScrollViewer Scroll(UIElement child)
	{
		return new ScrollViewer
		{
			Content = child,
			VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
			HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
			Focusable = false
		};
	}

	public static UIElement Pair(string k, string v)
	{
		DockPanel obj = new DockPanel
		{
			Margin = MCell
		};
		TextBlock textBlock = Text(k, FsNote, "Dim");
		textBlock.Margin = new Thickness(0.0, 0.0, 8.0, 0.0);
		DockPanel.SetDock(textBlock, Dock.Left);
		obj.Children.Add(textBlock);
		obj.Children.Add(Text(v, FsBody, "Text", bold: false, wrap: true));
		return obj;
	}

	public static TextBox Input(string text, double width, Action<string> changed)
	{
		TextBox t = new TextBox
		{
			Text = (text ?? ""),
			Width = width,
			Height = 30.0,
			Background = B("Panel2"),
			Foreground = B("Strong"),
			BorderBrush = B("Line"),
			CaretBrush = B("Strong"),
			SelectionBrush = B("Dim"),
			VerticalContentAlignment = VerticalAlignment.Center,
			Padding = new Thickness(6.0, 0.0, 6.0, 0.0),
			FontSize = FsBody,
			Margin = new Thickness(0.0, 0.0, 6.0, 0.0)
		};
		t.TextChanged += delegate
		{
			changed?.Invoke(t.Text);
		};
		return t;
	}

	public static UIElement LpMap(Topo t, Func<CpuSet, (Brush Fill, Brush Ring, string Tip)> look, Action<CpuSet> click = null, double cell = 26.0)
	{
		LayoutTemplate layoutTemplate = LayoutTemplate.For(t);
		StackPanel stackPanel = new StackPanel();
		foreach (IGrouping<byte, CpuSet> item4 in from s in t.Sets
			where s.Group == 0
			group s by s.LLC into g
			orderby g.Key
			select g)
		{
			ulong value;
			ulong num = (t.L3ByLlc.TryGetValue(item4.Key, out value) ? value : 0);
			string s2 = L.F("L3 domain {0}", item4.Key) + ((num != 0) ? (" · " + (num >> 20).ToString(CultureInfo.InvariantCulture) + " MB") : "") + ((t.BigCacheLlc == item4.Key) ? (" · " + L.T("big cache")) : "");
			stackPanel.Children.Add(Text(s2, FsSmall, "Dim"));
			WrapPanel wrapPanel = new WrapPanel
			{
				Margin = new Thickness(0.0, 3.0, 0.0, 8.0)
			};
			foreach (IGrouping<byte, CpuSet> item5 in from s in item4
				group s by s.Core into g
				orderby g.Key
				select g)
			{
				StackPanel stackPanel2 = new StackPanel
				{
					Orientation = (layoutTemplate.Stacked ? Orientation.Vertical : Orientation.Horizontal)
				};
				foreach (CpuSet item6 in item5.OrderBy((CpuSet s) => s.LP))
				{
					(Brush, Brush, string) tuple = look(item6);
					Brush item = tuple.Item1;
					Brush item2 = tuple.Item2;
					string item3 = tuple.Item3;
					Border border = new Border
					{
						Width = cell,
						Height = cell,
						Margin = new Thickness(1.0),
						CornerRadius = new CornerRadius(2.0),
						Background = item,
						BorderBrush = (item2 ?? B("None")),
						BorderThickness = new Thickness(2.0),
						ToolTip = (string.IsNullOrEmpty(item3) ? null : item3),
						Child = new TextBlock
						{
							Text = item6.LP.ToString(CultureInfo.InvariantCulture),
							FontSize = FsSmall,
							Foreground = B("Strong"),
							FontFamily = Mono,
							HorizontalAlignment = HorizontalAlignment.Center,
							VerticalAlignment = VerticalAlignment.Center
						}
					};
					if (click != null)
					{
						CpuSet captured = item6;
						border.Cursor = Cursors.Hand;
						border.MouseLeftButtonUp += delegate
						{
							click(captured);
						};
					}
					stackPanel2.Children.Add(border);
				}
				wrapPanel.Children.Add(new Border
				{
					BorderBrush = B("Line"),
					BorderThickness = new Thickness(1.0),
					CornerRadius = new CornerRadius(3.0),
					Padding = new Thickness(2.0),
					Margin = new Thickness(0.0, 0.0, 5.0, 5.0),
					Child = stackPanel2
				});
			}
			stackPanel.Children.Add(wrapPanel);
		}
		return stackPanel;
	}
}
