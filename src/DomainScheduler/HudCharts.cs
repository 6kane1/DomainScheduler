using System;
using System.Collections.Generic;
using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace DomainScheduler.App;

/// <summary>Frozen pens and brushes shared by the HUD drawings (made once).</summary>
internal static class HudInk
{
	public static readonly Pen Grid = Make("Line2", 1.0, dash: true);

	public static readonly Pen Base = Make("Line", 1.0);

	public static readonly Pen Low = MakeColor(Color.FromArgb(128, 244, 182, 203), 1.0, new double[] { 5.0, 4.0 });

	public static readonly Pen Trace = Make("Accent", 1.3);

	public static readonly Pen Hitch = Make("Warn", 1.0);

	public static readonly Brush Fill = MakeFill();

	public static readonly Brush Track = Frozen(Color.FromRgb(22, 23, 28));

	public static readonly Brush TrackLine = Frozen(Color.FromRgb(29, 30, 36));

	public static readonly Brush GameBar = Frozen(Ui.C("Accent"));

	public static readonly Brush BgBar = Frozen(Color.FromArgb(150, 201, 206, 214));

	public static readonly Brush IrqBar = Frozen(Color.FromArgb(230, 232, 194, 122));

	private static Pen Make(string key, double w, bool dash = false)
	{
		return MakeColor(Ui.C(key), w, dash ? new double[] { 2.0, 3.0 } : null);
	}

	private static Pen MakeColor(Color c, double w, double[] dash)
	{
		Pen pen = new Pen(Frozen(c), w)
		{
			LineJoin = PenLineJoin.Round
		};
		if (dash != null)
		{
			pen.DashStyle = new DashStyle(dash, 0.0);
		}
		pen.Freeze();
		return pen;
	}

	public static Brush Frozen(Color c)
	{
		SolidColorBrush b = new SolidColorBrush(c);
		b.Freeze();
		return b;
	}

	private static Brush MakeFill()
	{
		LinearGradientBrush b = new LinearGradientBrush
		{
			StartPoint = new Point(0.0, 0.0),
			EndPoint = new Point(0.0, 1.0)
		};
		b.GradientStops.Add(new GradientStop(Color.FromArgb(64, 244, 182, 203), 0.0));
		b.GradientStops.Add(new GradientStop(Color.FromArgb(0, 244, 182, 203), 1.0));
		b.Freeze();
		return b;
	}

	public static FormattedText Text(Visual v, string s, Brush brush, double size, bool mono = true)
	{
		return new FormattedText(s ?? "", CultureInfo.CurrentUICulture, FlowDirection.LeftToRight, new Typeface(mono ? Ui.Mono : Ui.Font(), FontStyles.Normal, FontWeights.Normal, FontStretches.Normal), size, brush, VisualTreeHelper.GetDpi(v).PixelsPerDip);
	}
}

/// <summary>
/// The live frame-time line of the home screen: a minute in fixed buckets, one StreamGeometry for the line and one
/// for the area under it, a dashed 1% low line and amber rings on hitches. Drawn only when Set is called.
/// </summary>
internal sealed class HudFrameChart : FrameworkElement
{
	private const double Left = 42.0;

	private const double Right = 6.0;

	private const double Top = 8.0;

	private const double Bottom = 6.0;

	private double[] _max = Array.Empty<double>();

	private double[] _endMax = Array.Empty<double>();

	private double _median;

	private double _low1Ms;

	private double _top = 5.0;

	private string _unit = "ms";

	private string _lowText = "1% low";

	private string _labelsKey = "";

	private readonly List<FormattedText> _labels = new List<FormattedText>();

	private FormattedText _lowLabel;

	public HudFrameChart()
	{
		base.SnapsToDevicePixels = true;
		base.IsHitTestVisible = false;
	}

	/// <summary>max/endMax: the bucket arrays of HudMath.Buckets (copied); median and 1% low in ms (0 = unknown).</summary>
	public void Set(double[] max, double[] endMax, double medianMs, double low1Ms, string unit, string lowText)
	{
		if (_max.Length != max.Length)
		{
			_max = new double[max.Length];
			_endMax = new double[max.Length];
		}
		Array.Copy(max, _max, max.Length);
		Array.Copy(endMax, _endMax, endMax.Length);
		_median = medianMs;
		_low1Ms = low1Ms;
		_unit = unit ?? "ms";
		if (_lowText != lowText)
		{
			_lowText = lowText ?? "";
			_lowLabel = null;
		}
		double peak = 0.0;
		foreach (double x in _max)
		{
			peak = Math.Max(peak, x);
		}
		_top = HudMath.ChartTop(peak, low1Ms, medianMs);
		InvalidateVisual();
	}

	public void Clear()
	{
		Array.Clear(_max);
		Array.Clear(_endMax);
		InvalidateVisual();
	}

	private void Labels()
	{
		string key = _top.ToString("R", CultureInfo.InvariantCulture) + "|" + _unit;
		if (key == _labelsKey && _labels.Count > 0)
		{
			return;
		}
		_labelsKey = key;
		_labels.Clear();
		for (int i = 0; i <= 5; i++)
		{
			double v = _top * i / 5.0;
			_labels.Add(HudInk.Text(this, v.ToString("0.#", CultureInfo.CurrentCulture) + " " + _unit, Ui.B("Faint"), 9.5));
		}
	}

	protected override void OnRender(DrawingContext dc)
	{
		double w = base.ActualWidth;
		double h = base.ActualHeight;
		if (w < Left + 40.0 || h < 40.0)
		{
			return;
		}
		double x0 = Left;
		double x1 = w - Right;
		double y0 = Top;
		double y1 = h - Bottom;
		Labels();
		for (int i = 0; i <= 5; i++)
		{
			double y = Math.Round(Y(_top * i / 5.0)) + 0.5;
			dc.DrawLine((i == 0) ? HudInk.Base : HudInk.Grid, new Point(x0, y), new Point(x1, y));
			FormattedText t = _labels[i];
			dc.DrawText(t, new Point(0.0, y - t.Height / 2.0));
		}
		int n = _max.Length;
		if (n < 2)
		{
			return;
		}
		double step = (x1 - x0) / (n - 1);
		StreamGeometry line = new StreamGeometry();
		StreamGeometry area = new StreamGeometry();
		int last = -1;
		using (StreamGeometryContext l = line.Open())
		{
			using StreamGeometryContext a = area.Open();
			int i = 0;
			while (i < n)
			{
				if (_max[i] <= 0.0)
				{
					i++;
					continue;
				}
				int from = i;
				Point p = new Point(X(i), Y(_max[i]));
				l.BeginFigure(p, isFilled: false, isClosed: false);
				a.BeginFigure(new Point(p.X, y1), isFilled: true, isClosed: true);
				a.LineTo(p, isStroked: false, isSmoothJoin: false);
				for (i++; i < n && _max[i] > 0.0; i++)
				{
					Point q = new Point(X(i), Y(_max[i]));
					l.LineTo(q, isStroked: true, isSmoothJoin: false);
					a.LineTo(q, isStroked: false, isSmoothJoin: false);
				}
				last = i - 1;
				a.LineTo(new Point(X(last), y1), isStroked: false, isSmoothJoin: false);
				if (last == from)
				{
					// A single point: a short flat stroke so it is visible.
					l.LineTo(new Point(p.X + 1.0, p.Y), isStroked: true, isSmoothJoin: false);
				}
			}
		}
		line.Freeze();
		area.Freeze();
		dc.DrawGeometry(HudInk.Fill, null, area);
		if (_low1Ms > 0.0)
		{
			double ly = Math.Round(Y(_low1Ms)) + 0.5;
			dc.DrawLine(HudInk.Low, new Point(x0, ly), new Point(x1, ly));
			if (_lowLabel == null)
			{
				_lowLabel = HudInk.Text(this, _lowText, Ui.B("Accent"), 9.5);
			}
			dc.DrawText(_lowLabel, new Point(x1 - _lowLabel.Width - 4.0, Math.Max(0.0, ly - _lowLabel.Height - 1.0)));
		}
		dc.DrawGeometry(null, HudInk.Trace, line);
		if (_median > 0.0)
		{
			// Rings on every hitch (at most 40, the latest kept), the value on the three longest.
			List<int> hitches = new List<int>();
			for (int i = n - 1; i >= 0 && hitches.Count < 40; i--)
			{
				if (HudMath.IsHitch(_endMax[i], _median))
				{
					hitches.Add(i);
				}
			}
			hitches.Sort((int a, int b) => _endMax[b].CompareTo(_endMax[a]));
			for (int k = 0; k < hitches.Count; k++)
			{
				int i = hitches[k];
				Point c = new Point(X(i), Y(_endMax[i]));
				dc.DrawEllipse(null, HudInk.Hitch, c, 3.0, 3.0);
				if (k < 3)
				{
					FormattedText t = HudInk.Text(this, _endMax[i].ToString((_endMax[i] < 100.0) ? "0.0" : "0", CultureInfo.CurrentCulture), Ui.B("Warn"), 9.5);
					double tx = Math.Min(c.X + 6.0, x1 - t.Width);
					dc.DrawText(t, new Point(tx, Math.Max(0.0, c.Y - t.Height / 2.0)));
				}
			}
		}
		if (last >= 0)
		{
			dc.DrawEllipse(HudInk.GameBar, null, new Point(X(last), Y(_max[last])), 3.0, 3.0);
		}
		double X(int i)
		{
			return x0 + i * step;
		}
		double Y(double ms)
		{
			return y1 - Math.Clamp(ms / _top, 0.0, 1.0) * (y1 - y0);
		}
	}
}

/// <summary>
/// One bar per logical processor with its load: pink = the game's processors, grey = background, amber = interrupt
/// processors the game is kept off. Segmented like the mockup (3 px lit, 2 px gap), drawn as one geometry per colour.
/// </summary>
internal sealed class CoreMapView : FrameworkElement
{
	private const double LabelH = 16.0;

	private const double Period = 5.0;

	private const double Seg = 3.0;

	private double[] _load = Array.Empty<double>();

	private CoreKind[] _kind = Array.Empty<CoreKind>();

	private int[] _lp = Array.Empty<int>();

	private readonly Dictionary<string, FormattedText> _labels = new Dictionary<string, FormattedText>();

	public CoreMapView()
	{
		base.SnapsToDevicePixels = true;
	}

	/// <summary>lps: the processor numbers shown; load 0..1 per entry (missing = 0).</summary>
	public void Set(int[] lps, double[] load, CoreKind[] kinds)
	{
		bool changed = lps.Length != _lp.Length;
		for (int i = 0; !changed && i < lps.Length; i++)
		{
			changed = lps[i] != _lp[i] || kinds[i] != _kind[i] || Math.Abs(load[i] - _load[i]) > 0.005;
		}
		if (!changed)
		{
			return;
		}
		_lp = (int[])lps.Clone();
		_load = (double[])load.Clone();
		_kind = (CoreKind[])kinds.Clone();
		InvalidateVisual();
	}

	protected override void OnRender(DrawingContext dc)
	{
		double w = base.ActualWidth;
		double h = base.ActualHeight;
		int n = _lp.Length;
		dc.DrawRectangle(Ui.B("None"), null, new Rect(0.0, 0.0, Math.Max(0.0, w), Math.Max(0.0, h)));
		if (n == 0 || w < 20.0 || h < LabelH + 20.0)
		{
			return;
		}
		double gap = (n <= 16) ? 5.0 : ((n <= 32) ? 3.0 : 2.0);
		double bw = Math.Floor((w - gap * (n - 1)) / n);
		if (bw < 2.0)
		{
			bw = 2.0;
			gap = 0.0;
		}
		double total = bw * n + gap * (n - 1);
		double xs = Math.Floor((w - total) / 2.0);
		double barH = h - LabelH - 4.0;
		int segs = Math.Max(1, (int)Math.Floor((barH + (Period - Seg)) / Period));
		double yb = Math.Floor(barH);
		StreamGeometry track = new StreamGeometry();
		StreamGeometry game = new StreamGeometry();
		StreamGeometry bg = new StreamGeometry();
		StreamGeometry irq = new StreamGeometry();
		using (StreamGeometryContext t = track.Open())
		using (StreamGeometryContext g = game.Open())
		using (StreamGeometryContext b = bg.Open())
		using (StreamGeometryContext q = irq.Open())
		{
			for (int i = 0; i < n; i++)
			{
				double x = xs + i * (bw + gap);
				int lit = (int)Math.Round(Math.Clamp(_load[i], 0.0, 1.0) * segs);
				if (_load[i] > 0.01 && lit == 0)
				{
					lit = 1;
				}
				StreamGeometryContext c = _kind[i] switch
				{
					CoreKind.Game => g,
					CoreKind.Interrupt => q,
					_ => b,
				};
				for (int s = 0; s < segs; s++)
				{
					double y = yb - (s + 1) * Period + (Period - Seg);
					Rectangle(s < lit ? c : t, x, y, bw, Seg);
				}
			}
		}
		track.Freeze();
		game.Freeze();
		bg.Freeze();
		irq.Freeze();
		dc.DrawGeometry(HudInk.Track, null, track);
		dc.DrawGeometry(HudInk.BgBar, null, bg);
		dc.DrawGeometry(HudInk.IrqBar, null, irq);
		dc.DrawGeometry(HudInk.GameBar, null, game);
		int every = (bw >= 16.0) ? 1 : ((bw >= 7.0) ? 2 : 4);
		for (int i = 0; i < n; i += every)
		{
			string s = _lp[i].ToString("00", CultureInfo.InvariantCulture);
			string key = s + (_kind[i] == CoreKind.Game ? "g" : "");
			if (!_labels.TryGetValue(key, out var ft))
			{
				ft = HudInk.Text(this, s, Ui.B((_kind[i] == CoreKind.Game) ? "Accent" : "Dim"), 9.5);
				_labels[key] = ft;
			}
			double x = xs + i * (bw + gap) + bw / 2.0 - ft.Width / 2.0;
			dc.DrawText(ft, new Point(x, yb + 4.0));
		}
		static void Rectangle(StreamGeometryContext c, double x, double y, double w, double h)
		{
			c.BeginFigure(new Point(x, y), isFilled: true, isClosed: true);
			c.LineTo(new Point(x + w, y), isStroked: false, isSmoothJoin: false);
			c.LineTo(new Point(x + w, y + h), isStroked: false, isSmoothJoin: false);
			c.LineTo(new Point(x, y + h), isStroked: false, isSmoothJoin: false);
		}
	}
}

/// <summary>
/// The segmented offset slider of the graphics card page: pink segments from zero to the value, dark ones elsewhere,
/// a square marker at the value. Click or drag sets the value, arrow keys move one step. Snap does the rounding.
/// </summary>
internal sealed class SegSlider : FrameworkElement
{
	private const int Segments = 48;

	private const double BarH = 22.0;

	private readonly int _min;

	private readonly int _max;

	private readonly int _step;

	private readonly Func<int, int> _snap;

	private int _value;

	public event Action<int> ValueChanged;

	public int Value
	{
		get
		{
			return _value;
		}
		set
		{
			int v = _snap(Math.Clamp(value, _min, _max));
			if (v != _value)
			{
				_value = v;
				InvalidateVisual();
				ValueChanged?.Invoke(v);
			}
		}
	}

	public SegSlider(int min, int max, int step, int value, Func<int, int> snap)
	{
		_min = min;
		_max = Math.Max(max, min + 1);
		_step = Math.Max(1, step);
		_snap = snap ?? ((int v) => v);
		_value = _snap(Math.Clamp(value, _min, _max));
		base.Height = BarH + 2.0;
		base.Focusable = true;
		base.FocusVisualStyle = null;
		base.Cursor = Cursors.Hand;
		base.SnapsToDevicePixels = true;
	}

	private int FromX(double x)
	{
		double w = Math.Max(1.0, base.ActualWidth);
		return (int)Math.Round(_min + Math.Clamp(x / w, 0.0, 1.0) * (_max - _min));
	}

	protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
	{
		Focus();
		CaptureMouse();
		Value = FromX(e.GetPosition(this).X);
		e.Handled = true;
	}

	protected override void OnMouseMove(MouseEventArgs e)
	{
		if (base.IsMouseCaptured)
		{
			Value = FromX(e.GetPosition(this).X);
		}
	}

	protected override void OnMouseLeftButtonUp(MouseButtonEventArgs e)
	{
		ReleaseMouseCapture();
		e.Handled = true;
	}

	protected override void OnKeyDown(KeyEventArgs e)
	{
		if (e.Key == Key.Left || e.Key == Key.Down)
		{
			Value = _value - _step;
			e.Handled = true;
		}
		else if (e.Key == Key.Right || e.Key == Key.Up)
		{
			Value = _value + _step;
			e.Handled = true;
		}
	}

	protected override void OnGotKeyboardFocus(KeyboardFocusChangedEventArgs e)
	{
		InvalidateVisual();
	}

	protected override void OnLostKeyboardFocus(KeyboardFocusChangedEventArgs e)
	{
		InvalidateVisual();
	}

	protected override void OnRender(DrawingContext dc)
	{
		double w = base.ActualWidth;
		if (w < Segments * 2)
		{
			return;
		}
		// A transparent fill: the whole bar takes clicks, not only the segments.
		dc.DrawRectangle(Ui.B("None"), null, new Rect(0.0, 0.0, w, BarH + 2.0));
		double gap = 2.0;
		double sw = (w - gap * (Segments - 1)) / Segments;
		double zero = Math.Clamp(0, _min, _max);
		double lo = Math.Min(zero, _value);
		double hi = Math.Max(zero, _value);
		double span = _max - _min;
		StreamGeometry on = new StreamGeometry();
		StreamGeometry off = new StreamGeometry();
		using (StreamGeometryContext a = on.Open())
		using (StreamGeometryContext b = off.Open())
		{
			for (int i = 0; i < Segments; i++)
			{
				double vFrom = _min + span * i / Segments;
				double vTo = _min + span * (i + 1) / Segments;
				double mid = (vFrom + vTo) / 2.0;
				bool lit = hi > lo && mid >= lo && mid <= hi;
				double x = Math.Round(i * (sw + gap));
				double x2 = Math.Round(i * (sw + gap) + sw);
				StreamGeometryContext c = lit ? a : b;
				c.BeginFigure(new Point(x, 1.0), isFilled: true, isClosed: true);
				c.LineTo(new Point(x2, 1.0), isStroked: false, isSmoothJoin: false);
				c.LineTo(new Point(x2, BarH + 1.0), isStroked: false, isSmoothJoin: false);
				c.LineTo(new Point(x, BarH + 1.0), isStroked: false, isSmoothJoin: false);
			}
		}
		on.Freeze();
		off.Freeze();
		dc.DrawGeometry(HudInk.Track, null, off);
		dc.DrawGeometry(HudInk.GameBar, null, on);
		// The square marker at the value; outlined in pink while the slider has the keyboard.
		double vx = Math.Round(Math.Clamp((_value - _min) / span, 0.0, 1.0) * (w - 6.0));
		dc.DrawRectangle(Ui.B("Strong"), base.IsKeyboardFocused ? HudInk.Trace : null, new Rect(vx, 0.0, 6.0, BarH + 2.0));
	}
}
