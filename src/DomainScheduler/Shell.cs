using System.ComponentModel;
using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Media;
using System.Windows.Shapes;

namespace DomainScheduler.App;

/// <summary>The one window: navigation on the left, the page, the log panel below.</summary>
sealed class Shell : Window
{
    readonly Hub _h;
    readonly List<Page> _pages = new();
    readonly StackPanel _nav = new();
    readonly ContentControl _host = new() { Focusable = false };
    readonly Ellipse _dot = new() { Width = 12, Height = 12, VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(0, 0, 8, 0) };
    readonly TextBlock _status = Ui.Text("", wrap: false);
    readonly TextBlock _log = new() { TextWrapping = TextWrapping.Wrap, FontSize = Ui.FsSmall };
    readonly ScrollViewer _logScroll;
    readonly Border _overlay;
    readonly TextBlock _overlayText = Ui.Text("", Ui.FsBody);
    readonly Grid _body = new();
    int _current;
    long _logVersion = -1;
    AutoState _dotState = (AutoState)(-1);
    bool _closedShown;

    public Shell(Hub hub)
    {
        _h = hub;
        Title = "DomainScheduler";
        Width = 1060;
        Height = 720;
        MinWidth = 900;
        MinHeight = 560;
        Background = Ui.B("Bg");
        Foreground = Ui.B("Text");
        FontSize = Ui.FsBody;
        FontFamily = Ui.Font();
        UseLayoutRounding = true;
        SnapsToDevicePixels = true;
        WindowStartupLocation = WindowStartupLocation.Manual;

        _pages.Add(new QuickPage(hub, this));
        _pages.Add(new GameThreadsPage(hub, this));
        _pages.Add(new ProcessesPage(hub, this));
        _pages.Add(new ThreadsPage(hub, this));
        _pages.Add(new MasksPage(hub, this));
        _pages.Add(new PresetsPage(hub, this));
        _pages.Add(new DevicesPage(hub, this));
        _pages.Add(new CleanerPage(hub, this));
        _pages.Add(new SettingsPage(hub, this));
        _pages.Add(new AboutPage(hub, this));

        _logScroll = new ScrollViewer { Content = _log, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, Focusable = false, Padding = new Thickness(10, 6, 10, 6) };
        _overlay = new Border { Background = Ui.B("Overlay"), Visibility = Visibility.Collapsed, Child = _overlayText };
        _overlayText.HorizontalAlignment = HorizontalAlignment.Center;

        var header = new DockPanel { Margin = new Thickness(16, 10, 16, 10) };
        header.Children.Add(_dot);
        header.Children.Add(_status);

        var navBox = new Border { Width = 176, Background = Ui.B("Panel"), BorderBrush = Ui.B("Line"), BorderThickness = new Thickness(0, 0, 1, 0), Padding = new Thickness(8, 10, 8, 10), Child = _nav };
        var logBox = new Border { Height = 132, Background = Ui.B("Panel"), BorderBrush = Ui.B("Line"), BorderThickness = new Thickness(0, 1, 0, 0), Child = _logScroll };
        var headBox = new Border { BorderBrush = Ui.B("Line"), BorderThickness = new Thickness(0, 0, 0, 1), Child = header };

        _body.Children.Add(new Border { Padding = new Thickness(16, 14, 16, 4), Child = _host });
        _body.Children.Add(_overlay);

        var root = new DockPanel();
        DockPanel.SetDock(navBox, Dock.Left);
        DockPanel.SetDock(logBox, Dock.Bottom);
        DockPanel.SetDock(headBox, Dock.Top);
        root.Children.Add(navBox);
        root.Children.Add(logBox);
        root.Children.Add(headBox);
        root.Children.Add(_body);
        Content = root;

        RestoreGeometry();
        L.Changed += OnLanguage;
        BuildNav();
        Show(0);
        Refresh();
    }

    public double DpiScaleX => VisualTreeHelper.GetDpi(this).DpiScaleX;
    public bool AutoOn => _h.Auto.On;

    void OnLanguage() => Dispatcher.BeginInvoke(() =>
    {
        FontFamily = Ui.Font();
        BuildNav();
        Show(_current);
        _logVersion = -1;
        Refresh();
    });

    void BuildNav()
    {
        _nav.Children.Clear();
        for (int i = 0; i < _pages.Count; i++)
        {
            int index = i;
            bool closed = _pages[i].ClosedInAuto && AutoOn;
            var b = new Button
            {
                Content = _pages[i].Title, Height = 32, Margin = new Thickness(0, 0, 0, 4), HorizontalContentAlignment = HorizontalAlignment.Left,
                FontSize = Ui.FsBody, BorderBrush = Ui.B(i == _current ? "Dim" : "None"), Background = Ui.B(i == _current ? "Sel" : "None"),
                Visibility = closed ? Visibility.Collapsed : Visibility.Visible,
            };
            b.Click += (_, _) => Show(index);
            _nav.Children.Add(b);
        }
    }

    /// <summary>Shows a page; the page is built anew so it always reflects the current state.</summary>
    public void Show(int index)
    {
        if (index < 0 || index >= _pages.Count) index = 0;
        if (_pages[index].ClosedInAuto && AutoOn) index = 0;
        _current = index;
        try
        {
            _host.Content = _pages[index].Build();
            _pages[index].Shown();
        }
        catch (Exception e)
        {
            AppLog.Add(L.F("Page {0}: {1}", _pages[index].Title, e.Message), AppLog.Sev.Error);
            _host.Content = Ui.Text(e.Message, wrap: true, brush: "Error");
        }
        BuildNav();
    }

    public void ShowPage<T>() where T : Page => Show(_pages.FindIndex(p => p is T));

    public T Get<T>() where T : Page => _pages.OfType<T>().First();

    /// <summary>The state machine's state as a colour: the only place that paints the dot.</summary>
    void SetDot(AutoState s)
    {
        if (s == _dotState) return;
        _dotState = s;
        _dot.Fill = Ui.B(s switch
        {
            AutoState.Armed => "Warn",
            AutoState.Active => "On",
            AutoState.Reverting => "Dim",
            AutoState.Faulted => "Error",
            _ => "Faint",
        });
    }

    /// <summary>After every tick and every action: dot, status line, log, the current page.</summary>
    public void Refresh()
    {
        SetDot(_h.Auto.State);
        _status.Text = _h.Auto.Status;
        bool closed = AutoOn;
        if (closed != _closedShown)
        {
            _closedShown = closed;
            if (_pages[_current].ClosedInAuto && closed) Show(0);
            else BuildNav();
        }
        DrawLog();
        try { _pages[_current].Tick(); }
        catch (Exception e) { AppLog.Add(L.F("Page {0}: {1}", _pages[_current].Title, e.Message), AppLog.Sev.Error); }
    }

    void DrawLog()
    {
        long v = AppLog.Version;
        if (v == _logVersion) return;
        _logVersion = v;
        bool atEnd = _logScroll.VerticalOffset >= _logScroll.ScrollableHeight - 4;
        _log.Inlines.Clear();
        foreach (var e in AppLog.Tail(200))
        {
            string brush = e.Sev switch { AppLog.Sev.Error => "Error", AppLog.Sev.Warn => "Warn", _ => "Dim" };
            _log.Inlines.Add(new Run(e.Utc.ToLocalTime().ToString("HH:mm:ss", CultureInfo.InvariantCulture) + "  " + e.Msg + "\n") { Foreground = Ui.B(brush) });
        }
        if (atEnd) _logScroll.ScrollToEnd();
    }

    /// <summary>A long operation: the overlay is shown, the work runs on the worker, done runs here.</summary>
    public void Busy<T>(string text, Func<T> work, Action<T> done)
    {
        _overlayText.Text = text;
        _overlay.Visibility = Visibility.Visible;
        _h.Opt.Run(work, r =>
        {
            _overlay.Visibility = Visibility.Collapsed;
            try { done(r); }
            catch (Exception e) { AppLog.Add(L.F("Error: {0}", e.Message), AppLog.Sev.Error); }
        });
    }

    /// <summary>The same for work that must not wait for the engine's worker (disk, WMI).</summary>
    public void BusyTask<T>(string text, Func<T> work, Action<T> done)
    {
        _overlayText.Text = text;
        _overlay.Visibility = Visibility.Visible;
        Task.Run(() =>
        {
            T r = default;
            try { r = work(); }
            catch (Exception e) { AppLog.Add(L.F("Error: {0}", e.Message), AppLog.Sev.Error); }
            Dispatcher.BeginInvoke(() =>
            {
                _overlay.Visibility = Visibility.Collapsed;
                try { done(r); }
                catch (Exception e) { AppLog.Add(L.F("Error: {0}", e.Message), AppLog.Sev.Error); }
                Refresh();
            });
        });
    }

    /// <summary>Every user action ends here: a log line and a refresh of the dot.</summary>
    public void Did(string message, AppLog.Sev sev = AppLog.Sev.Info)
    {
        AppLog.Add(message, sev);
        Refresh();
    }

    /// <summary>Closing the window leaves the program in the tray; the changes stay until Exit.</summary>
    protected override void OnClosing(CancelEventArgs e)
    {
        e.Cancel = true;
        SaveGeometry();
        Hide();
    }

    void RestoreGeometry()
    {
        bool okL = double.TryParse(Settings.Get("WindowLeft"), NumberStyles.Float, CultureInfo.InvariantCulture, out double left);
        bool okT = double.TryParse(Settings.Get("WindowTop"), NumberStyles.Float, CultureInfo.InvariantCulture, out double top);
        if (okL && okT && left >= SystemParameters.VirtualScreenLeft && top >= SystemParameters.VirtualScreenTop &&
            left < SystemParameters.VirtualScreenLeft + SystemParameters.VirtualScreenWidth - 100 && top < SystemParameters.VirtualScreenTop + SystemParameters.VirtualScreenHeight - 100)
        {
            Left = left;
            Top = top;
        }
        else WindowStartupLocation = WindowStartupLocation.CenterScreen;
    }

    public void SaveGeometry()
    {
        try
        {
            if (WindowState != WindowState.Normal || double.IsNaN(Left)) return;
            Settings.Set("WindowLeft", Left.ToString("0", CultureInfo.InvariantCulture));
            Settings.Set("WindowTop", Top.ToString("0", CultureInfo.InvariantCulture));
        }
        catch (Exception) { }
    }
}
