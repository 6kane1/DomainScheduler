using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Media;

namespace DomainScheduler.App;

internal sealed class FrameChart : FrameworkElement
{
	public const int Seconds = 60;

	private List<double> _points = new List<double>();

	private double _before;

	private double _appliedAgo = -1.0;

	private string _markText = "";

	private string _beforeText = "";

	public void Set(List<double> intervalsMs, double beforeP99, double appliedSecondsAgo, string markText, string beforeText)
	{
		List<double> list = new List<double>();
		double num = 0.0;
		double num2 = 0.0;
		foreach (double intervalsM in intervalsMs)
		{
			num2 = Math.Max(num2, intervalsM);
			for (num += intervalsM; num >= 250.0; num -= 250.0)
			{
				list.Add(num2);
				num2 = 0.0;
			}
		}
		_points = list;
		_before = beforeP99;
		_appliedAgo = appliedSecondsAgo;
		_markText = markText ?? "";
		_beforeText = beforeText ?? "";
		InvalidateVisual();
	}

	private FormattedText Label(string s, string brush)
	{
		return new FormattedText(s, CultureInfo.CurrentUICulture, FlowDirection.LeftToRight, new Typeface(Ui.Font(), FontStyles.Normal, FontWeights.Normal, FontStretches.Normal), Ui.FsSmall, Ui.B(brush), VisualTreeHelper.GetDpi(this).PixelsPerDip);
	}

	protected override void OnRender(DrawingContext dc)
	{
		double w = base.ActualWidth;
		double h = base.ActualHeight;
		if (w < 20.0 || h < 20.0)
		{
			return;
		}
		Pen pen = new Pen(Ui.B("Line"), 1.0);
		pen.Freeze();
		for (int j = 0; j <= 3; j++)
		{
			dc.DrawLine(pen, new Point(0.0, Math.Round(h * (double)j / 3.0) + 0.5), new Point(w, Math.Round(h * (double)j / 3.0) + 0.5));
		}
		if (_points.Count < 2)
		{
			return;
		}
		double top = Math.Max(_points.Max(), _before) * 1.15;
		int slots;
		if (!(top <= 0.0))
		{
			slots = 240;
			double num = ((_appliedAgo >= 0.0 && _appliedAgo < 60.0) ? (w - _appliedAgo / 60.0 * w) : (-1.0));
			if (_before > 0.0)
			{
				Pen pen2 = new Pen(Ui.B("Faint"), 1.0);
				pen2.DashStyle = new DashStyle(new double[2] { 3.0, 4.0 }, 0.0);
				Pen pen3 = pen2;
				pen3.Freeze();
				dc.DrawLine(pen3, new Point(0.0, Y(_before)), new Point(w, Y(_before)));
				FormattedText formattedText = Label(_beforeText, "Dim");
				dc.DrawText(formattedText, new Point(w - formattedText.Width, Math.Max(0.0, Y(_before) - formattedText.Height - 1.0)));
			}
			if (num >= 0.0)
			{
				Pen pen4 = new Pen(Ui.B("AccentLine"), 1.0);
				pen4.Freeze();
				dc.DrawLine(pen4, new Point(num, 0.0), new Point(num, h));
				FormattedText formattedText2 = Label(_markText, "Accent");
				dc.DrawText(formattedText2, new Point(Math.Min(num + 6.0, w - formattedText2.Width), 0.0));
			}
			int num2 = ((!(num < 0.0)) ? Math.Clamp(_points.Count - 1 - (int)Math.Round(_appliedAgo * 4.0), 0, _points.Count - 1) : ((!(_appliedAgo >= 60.0)) ? (_points.Count - 1) : 0));
			if (_appliedAgo < 0.0)
			{
				num2 = _points.Count - 1;
			}
			Trace(0, num2, "Steel");
			Trace(num2, _points.Count - 1, "Accent");
		}
		void Trace(int from, int to, string brush)
		{
			if (to - from >= 1)
			{
				StreamGeometry streamGeometry = new StreamGeometry();
				using (StreamGeometryContext streamGeometryContext = streamGeometry.Open())
				{
					streamGeometryContext.BeginFigure(new Point(X(from), Y(_points[from])), isFilled: false, isClosed: false);
					for (int k = from + 1; k <= to; k++)
					{
						streamGeometryContext.LineTo(new Point(X(k), Y(_points[k])), isStroked: true, isSmoothJoin: true);
					}
				}
				streamGeometry.Freeze();
				Pen pen5 = new Pen(Ui.B(brush), 1.4)
				{
					LineJoin = PenLineJoin.Round
				};
				pen5.Freeze();
				dc.DrawGeometry(null, pen5, streamGeometry);
			}
		}
		double X(int i)
		{
			return w - (double)(_points.Count - 1 - i) * (w / (double)(slots - 1));
		}
		double Y(double ms)
		{
			return h - Math.Clamp(ms / top, 0.0, 1.0) * (h - 4.0) - 2.0;
		}
	}
}
