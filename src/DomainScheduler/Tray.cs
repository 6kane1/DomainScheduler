using System.Drawing;
using System.Windows.Forms;

namespace DomainScheduler.App;

/// <summary>The tray icon with only "Open" and "Exit".</summary>
sealed class Tray : IDisposable
{
    readonly NotifyIcon _icon;

    public Tray(Action open, Action exit)
    {
        var menu = new ContextMenuStrip();
        menu.Items.Add(L.T("Open"), null, (_, _) => open());
        menu.Items.Add(L.T("Exit"), null, (_, _) => exit());
        _icon = new NotifyIcon
        {
            Icon = Icon.ExtractAssociatedIcon(Environment.ProcessPath ?? "") ?? SystemIcons.Application,
            Text = "DomainScheduler",
            ContextMenuStrip = menu,
            Visible = true,
        };
        _icon.DoubleClick += (_, _) => open();
    }

    public void Dispose()
    {
        _icon.Visible = false;
        _icon.Dispose();
    }
}
