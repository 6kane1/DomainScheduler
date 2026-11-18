using System;
using System.Drawing;
using System.Windows.Forms;

namespace DomainScheduler.App;

internal sealed class Tray : IDisposable
{
	/// <summary>
	/// The same icon as the window, the taskbar and the shortcuts (app.ico), taken at the tray's own size (16 px at
	/// 100 %, 20/24 px at higher scaling) from the size drawn for it, instead of shrinking the 32 px exe icon.
	/// </summary>
	private static Icon TrayIcon()
	{
		try
		{
			var info = System.Windows.Application.GetResourceStream(new Uri("pack://application:,,,/app.ico"));
			if (info != null)
			{
				using var s = info.Stream;
				return new Icon(s, SystemInformation.SmallIconSize);
			}
		}
		catch (Exception) { }
		return Icon.ExtractAssociatedIcon(Environment.ProcessPath ?? "") ?? SystemIcons.Application;
	}

	private readonly NotifyIcon _icon;

	public Tray(Action open, Action exit)
	{
		ContextMenuStrip contextMenuStrip = new ContextMenuStrip
		{
			Items = 
			{
				{
					L.T("Open"),
					(Image?)null,
					(EventHandler?)delegate
					{
						open();
					}
				},
				{
					L.T("Exit"),
					(Image?)null,
					(EventHandler?)delegate
					{
						exit();
					}
				}
			}
		};
		_icon = new NotifyIcon
		{
			Icon = TrayIcon(),
			Text = "Domain OS Toolbox",
			ContextMenuStrip = contextMenuStrip,
			Visible = true
		};
		_icon.DoubleClick += delegate
		{
			open();
		};
	}

	/// <summary>A short notice from the tray icon (Windows shows it as a notification).</summary>
	public void Notice(string text)
	{
		_icon.ShowBalloonTip(5000, "Domain OS Toolbox", text, ToolTipIcon.Error);
	}

	/// <summary>A plain information notice (updates).</summary>
	public void Info(string text)
	{
		_icon.ShowBalloonTip(5000, "Domain OS Toolbox", text, ToolTipIcon.Info);
	}

	public void Dispose()
	{
		_icon.Visible = false;
		_icon.Dispose();
	}
}
