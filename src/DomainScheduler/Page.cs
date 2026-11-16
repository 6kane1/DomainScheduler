using System.Windows;

namespace DomainScheduler.App;

/// <summary>A page of the window: built in code, rebuilt when the language changes.</summary>
abstract class Page
{
    protected readonly Hub H;
    protected readonly Shell S;

    protected Page(Hub hub, Shell shell)
    {
        H = hub;
        S = shell;
    }

    public abstract string Title { get; }
    /// <summary>Pages that change things by hand are closed while Auto is on.</summary>
    public virtual bool ClosedInAuto => false;
    public abstract FrameworkElement Build();
    /// <summary>Every second while the page is shown.</summary>
    public virtual void Tick() { }
    /// <summary>The page became visible.</summary>
    public virtual void Shown() { }
}
