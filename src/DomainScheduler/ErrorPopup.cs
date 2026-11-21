using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;
using System.Windows;
using System.Windows.Threading;

namespace DomainScheduler.App;

/// <summary>
/// Errors are shown in a Windows error window as soon as they happen (there is no on-screen journal).
/// While a game runs under Auto the window waits until the game is closed, so it never pulls a player out of a match;
/// the tray shows a short notice at once. Errors that come in while the window is open are added to the next one.
/// </summary>
internal sealed class ErrorPopup
{
	private readonly Dispatcher _ui;
	private readonly Func<bool> _gameRunning;
	private readonly Func<Window> _owner;
	private readonly Action<string> _trayNotice;
	private readonly List<AppLog.Entry> _pending = new List<AppLog.Entry>();
	private readonly DispatcherTimer _wait;
	private bool _showing;

	public ErrorPopup(Dispatcher ui, Func<bool> gameRunning, Func<Window> owner, Action<string> trayNotice)
	{
		_ui = ui;
		_gameRunning = gameRunning;
		_owner = owner;
		_trayNotice = trayNotice;
		_wait = new DispatcherTimer(DispatcherPriority.Background, ui) { Interval = TimeSpan.FromSeconds(3) };
		_wait.Tick += delegate { TryShow(); };
		AppLog.ErrorAdded += OnError;
	}

	private void OnError(AppLog.Entry e)
	{
		_ui.BeginInvoke(new Action(delegate
		{
			bool first = _pending.Count == 0;
			_pending.Add(e);
			if (first && Running())
			{
				try { _trayNotice(L.T("Domain OS Toolbox: an error happened. The details open when the game is closed.")); } catch (Exception) { }
			}
			TryShow();
		}));
	}

	private bool Running()
	{
		try { return _gameRunning(); } catch (Exception) { return false; }
	}

	private void TryShow()
	{
		if (_showing || _pending.Count == 0) { _wait.Stop(); return; }
		if (Running()) { _wait.Start(); return; }
		_wait.Stop();
		var items = new List<AppLog.Entry>(_pending);
		_pending.Clear();
		var sb = new StringBuilder();
		foreach (AppLog.Entry e in items.Count > 8 ? items.GetRange(items.Count - 8, 8) : items)
		{
			sb.Append(e.Utc.ToLocalTime().ToString("HH:mm:ss", CultureInfo.InvariantCulture)).Append("  ").AppendLine(e.Msg);
		}
		if (items.Count > 8) sb.AppendLine(L.F("…and {0} earlier", items.Count - 8));
		sb.AppendLine();
		sb.Append(L.F("Details are in the log file: {0}", AppLog.ShownPath));
		_showing = true;
		try
		{
			Window owner = null;
			try { Window w = _owner(); if (w != null && w.IsVisible) owner = w; } catch (Exception) { }
			string title = L.T("Domain OS Toolbox — error");
			if (owner != null) MessageBox.Show(owner, sb.ToString(), title, MessageBoxButton.OK, MessageBoxImage.Error);
			else MessageBox.Show(sb.ToString(), title, MessageBoxButton.OK, MessageBoxImage.Error, MessageBoxResult.OK, MessageBoxOptions.None);
		}
		catch (Exception) { }
		finally
		{
			_showing = false;
		}
		if (_pending.Count > 0) _ui.BeginInvoke(new Action(TryShow), DispatcherPriority.Background);
	}
}
