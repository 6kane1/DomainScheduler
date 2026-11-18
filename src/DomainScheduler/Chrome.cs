using System;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using Path = System.Windows.Shapes.Path;
using System.Windows.Shell;

namespace DomainScheduler.App;

/// <summary>
/// Our own title bar (1.14.0): WindowChrome keeps the system's drag, double-click, snap and resize; the caption buttons
/// are drawn in the HUD style; DWM gives the dark frame, the rounded corners and the border colour on Windows 11.
/// </summary>
internal static class Chrome
{
	public const double CaptionHeight = 46.0;

	[DllImport("dwmapi.dll")]
	private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

	[DllImport("user32.dll")]
	private static extern int GetSystemMetricsForDpi(int index, uint dpi);

	[DllImport("user32.dll")]
	private static extern int GetSystemMetrics(int index);

	private const int DWMWA_USE_IMMERSIVE_DARK_MODE = 20;

	private const int DWMWA_WINDOW_CORNER_PREFERENCE = 33;

	private const int DWMWA_BORDER_COLOR = 34;

	private const int DWMWCP_ROUND = 2;

	public static void Attach(Window w)
	{
		WindowChrome.SetWindowChrome(w, new WindowChrome
		{
			CaptionHeight = CaptionHeight,
			ResizeBorderThickness = new Thickness(6.0),
			GlassFrameThickness = new Thickness(0.0),
			CornerRadius = new CornerRadius(0.0),
			UseAeroCaptionButtons = false,
			NonClientFrameEdges = NonClientFrameEdges.None
		});
	}

	/// <summary>Dark frame, round corners, a border in the panel line colour. True when Windows 11 drew the border
	/// (then the window needs no border of its own).</summary>
	public static bool Dwm(Window w)
	{
		try
		{
			IntPtr hwnd = new WindowInteropHelper(w).Handle;
			if (hwnd == IntPtr.Zero)
			{
				return false;
			}
			int dark = 1;
			DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, ref dark, 4);
			int round = DWMWCP_ROUND;
			bool win11 = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, ref round, 4) == 0;
			Color c = Ui.C("Line");
			int colorRef = c.R | (c.G << 8) | (c.B << 16);
			DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, ref colorRef, 4);
			return win11;
		}
		catch (Exception)
		{
			return false;
		}
	}

	/// <summary>
	/// A maximized window with WindowChrome reaches past the screen by the sizing frame; this padding keeps the
	/// content on the screen (sizing frame + padded border at the window's DPI, in device-independent units).
	/// </summary>
	public static Thickness MaximizedPadding(Window w)
	{
		try
		{
			DpiScale dpi = VisualTreeHelper.GetDpi(w);
			uint d = (uint)Math.Round(96.0 * dpi.DpiScaleX);
			int fx, fy, pad;
			try
			{
				fx = GetSystemMetricsForDpi(32, d);
				fy = GetSystemMetricsForDpi(33, d);
				pad = GetSystemMetricsForDpi(92, d);
			}
			catch (EntryPointNotFoundException)
			{
				fx = GetSystemMetrics(32);
				fy = GetSystemMetrics(33);
				pad = GetSystemMetrics(92);
			}
			double x = (fx + pad) / dpi.DpiScaleX;
			double y = (fy + pad) / dpi.DpiScaleY;
			if (x <= 0.0 || y <= 0.0 || x > 32.0 || y > 32.0)
			{
				return new Thickness(8.0);
			}
			return new Thickness(x, y, x, y);
		}
		catch (Exception)
		{
			return new Thickness(8.0);
		}
	}

	public enum Cap { Minimize, Maximize, Close }

	/// <summary>A caption button: thin HUD glyph, a pink-tinted hover, a muted red hover for close.</summary>
	public static Border CapButton(Window w, Cap kind, Func<string> tip)
	{
		Path glyph = new Path
		{
			Stroke = Ui.B("Silver"),
			StrokeThickness = 1.0,
			Width = 10.0,
			Height = 10.0,
			Stretch = Stretch.None,
			SnapsToDevicePixels = true,
			HorizontalAlignment = HorizontalAlignment.Center,
			VerticalAlignment = VerticalAlignment.Center,
			IsHitTestVisible = false
		};
		Border b = new Border
		{
			Width = 44.0,
			Height = CaptionHeight - 1.0,
			Background = Ui.B("None"),
			Child = glyph,
			VerticalAlignment = VerticalAlignment.Top
		};
		WindowChrome.SetIsHitTestVisibleInChrome(b, true);
		void Glyph()
		{
			string data = kind switch
			{
				Cap.Minimize => "M0,5.5 L10,5.5",
				Cap.Close => "M0.5,0.5 L9.5,9.5 M9.5,0.5 L0.5,9.5",
				_ => (w.WindowState == WindowState.Maximized) ? "M2.5,2.5 L2.5,0.5 L9.5,0.5 L9.5,7.5 L7.5,7.5 M0.5,2.5 L7.5,2.5 L7.5,9.5 L0.5,9.5 Z" : "M0.5,0.5 L9.5,0.5 L9.5,9.5 L0.5,9.5 Z"
			};
			Geometry g = Geometry.Parse(data);
			g.Freeze();
			glyph.Data = g;
		}
		Glyph();
		if (kind == Cap.Maximize)
		{
			w.StateChanged += delegate
			{
				Glyph();
			};
		}
		b.MouseEnter += delegate
		{
			b.Background = Ui.B((kind == Cap.Close) ? "CloseHover" : "Sel");
			glyph.Stroke = Ui.B((kind == Cap.Close) ? "Strong" : "Accent");
			b.ToolTip = tip?.Invoke();
		};
		b.MouseLeave += delegate
		{
			b.Background = Ui.B("None");
			glyph.Stroke = Ui.B("Silver");
		};
		b.MouseLeftButtonDown += delegate(object _, MouseButtonEventArgs e)
		{
			b.CaptureMouse();
			e.Handled = true;
		};
		b.MouseLeftButtonUp += delegate(object _, MouseButtonEventArgs e)
		{
			bool inside = b.IsMouseCaptured && e.GetPosition(b) is Point p && p.X >= 0.0 && p.Y >= 0.0 && p.X <= b.ActualWidth && p.Y <= b.ActualHeight;
			b.ReleaseMouseCapture();
			e.Handled = true;
			if (!inside)
			{
				return;
			}
			b.Background = Ui.B("None");
			glyph.Stroke = Ui.B("Silver");
			switch (kind)
			{
			case Cap.Minimize:
				SystemCommands.MinimizeWindow(w);
				break;
			case Cap.Maximize:
				if (w.WindowState == WindowState.Maximized)
				{
					SystemCommands.RestoreWindow(w);
				}
				else
				{
					SystemCommands.MaximizeWindow(w);
				}
				break;
			default:
				SystemCommands.CloseWindow(w);
				break;
			}
		};
		return b;
	}

	/// <summary>The logo: a square cut at the lower-left corner, pink over silver along the diagonal.</summary>
	public static FrameworkElement Logo(double size = 18.0)
	{
		Geometry shape = Geometry.Parse("M0,0 L18,0 L18,18 L5.4,18 L0,12.6 Z");
		shape.Freeze();
		Geometry pink = Geometry.Parse("M0,0 L18,0 L0,18 Z");
		pink.Freeze();
		Geometry silver = Geometry.Parse("M18,0 L18,18 L0,18 Z");
		silver.Freeze();
		DrawingGroup g = new DrawingGroup
		{
			ClipGeometry = shape
		};
		g.Children.Add(new GeometryDrawing(Ui.B("Accent"), null, pink));
		g.Children.Add(new GeometryDrawing(Ui.B("Silver"), null, silver));
		g.Freeze();
		return new Image
		{
			Source = new DrawingImage(g),
			Width = size,
			Height = size,
			Stretch = Stretch.Uniform,
			VerticalAlignment = VerticalAlignment.Center,
			IsHitTestVisible = false
		};
	}
}
