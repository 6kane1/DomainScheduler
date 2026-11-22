using System;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace DomainScheduler.App;

/// <summary>
/// The black box (1.16.1 Test 1): every stutter of the game Auto handles, explained in plain words with what to do.
/// Recording runs only while Auto handles a game; nothing is changed by it.
/// </summary>
internal sealed class BlackBoxPage : Page
{
	private StackPanel _list;
	private TextBlock _state, _summary;
	private int _shown = -1;

	public override string Title => L.T("Black box");

	public BlackBoxPage(Hub h, Shell s) : base(h, s) { }

	public override FrameworkElement Build()
	{
		var title = new TextBlock { Text = L.T("Black box"), FontSize = Ui.FsBody * 1.3, FontWeight = FontWeights.SemiBold, Foreground = Ui.B("Strong") };
		var sub = Ui.Text(L.T("Like a dashcam for the game: while Auto handles a game, the last 30 seconds are kept in memory. When a stutter happens, Toolbox shows what slowed the frame and what to do. Nothing is changed, nothing is written until a stutter."), Ui.FsNote, "Dim", bold: false, wrap: true);
		_state = Ui.Text("", Ui.FsNote, "Accent", bold: false, wrap: true);
		_summary = Ui.Text("", Ui.FsBody, "Text", bold: false, wrap: true);
		_list = new StackPanel();
		var open = Ui.Btn(L.T("Open the folder"), () =>
		{
			try { string f = System.IO.Path.Combine(Store.Dir, "blackbox"); System.IO.Directory.CreateDirectory(f); Process.Start(new ProcessStartInfo("explorer.exe", "\"" + f + "\"") { UseShellExecute = true }); } catch (Exception) { }
		});
		open.HorizontalAlignment = HorizontalAlignment.Left;
		_shown = -1;
		Tick();
		return Ui.Scroll(Ui.Col(title, sub, new Border { Height = 10 }, _state, new Border { Height = 8 }, Ui.Card(Ui.Col(Ui.Head(L.T("Last game")), _summary)), Ui.Card(Ui.Col(Ui.Head(L.T("Stutters, newest first")), _list)), open));
	}

	public override void Tick()
	{
		if (_list == null) return;
		_state.Text = !H.BlackBoxOn ? L.T("Off (Settings, Black box).")
			: H.Box.Running ? L.F("Recording {0}. Play as usual.", H.Auto.GameExe)
			: L.T("Waiting for a game: turn Auto on and start the game.");
		var reports = H.Box.Reports;
		if (reports.Count == _shown) return;
		_shown = reports.Count;
		_list.Children.Clear();
		if (reports.Count == 0)
		{
			_summary.Text = L.T("No stutters recorded yet.");
			_list.Children.Add(Ui.Note(L.T("When a frame takes more than three times the usual and longer than two refreshes of the monitor, it appears here.")));
			return;
		}
		bool ru = L.Lang == "ru";
		var g = reports.GroupBy(r => r.Cause).ToDictionary(x => x.Key, x => x.Count());
		int n = reports.Count;
		string Part(BbCause c, string name) => g.TryGetValue(c, out int k) ? string.Format(CultureInfo.InvariantCulture, "{0} {1} ({2:0}%)", name, k, k * 100.0 / n) : null;
		_summary.Text = L.F("{0} stutters: ", n) + string.Join(", ", new[] {
			Part(BbCause.Game, L.T("game or graphics card")), Part(BbCause.Driver, L.T("driver")),
			Part(BbCause.Other, L.T("another program")), Part(BbCause.Cores, L.T("not enough free cores")), Part(BbCause.Loading, L.T("loading")) }.Where(x => x != null)) + ".";
		foreach (var r in reports.Take(30))
		{
			string badge = r.Cause switch { BbCause.Game => L.T("game or graphics card"), BbCause.Driver => L.T("driver"), BbCause.Other => L.T("another program"), BbCause.Loading => L.T("loading"), _ => L.T("not enough free cores") };
			string brush = r.Cause switch { BbCause.Game or BbCause.Loading => "Dim", BbCause.Cores => "Warn", _ => "Accent" };
			var head = new DockPanel();
			var when = Ui.Text(r.When.ToString("HH:mm:ss", CultureInfo.InvariantCulture) + "   " + string.Format(CultureInfo.InvariantCulture, "{0:0.0} ms", r.FrameMs), Ui.FsBody, "Strong", bold: true);
			var b = Ui.Text(badge, Ui.FsNote, brush, bold: true);
			b.HorizontalAlignment = HorizontalAlignment.Right;
			DockPanel.SetDock(b, Dock.Right);
			head.Children.Add(b); head.Children.Add(when);
			var text = Ui.Text(ru ? r.TextRu : r.TextEn, Ui.FsNote, "Text", bold: false, wrap: true);
			var adv = Ui.Text((ru ? "Что сделать: " : "What to do: ") + (ru ? r.AdviceRu : r.AdviceEn), Ui.FsNote, "Dim", bold: false, wrap: true);
			text.Margin = new Thickness(0, 4, 0, 2);
			_list.Children.Add(new Border { BorderBrush = Ui.B("Line"), BorderThickness = new Thickness(0, 0, 0, 1), Padding = new Thickness(0, 10, 0, 10), Child = Ui.Col(head, text, adv) });
		}
	}
}
