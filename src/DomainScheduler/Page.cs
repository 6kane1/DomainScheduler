using System.Windows;

namespace DomainScheduler.App;

internal abstract class Page
{
	protected readonly Hub H;

	protected readonly Shell S;

	public abstract string Title { get; }

	public virtual string Section => "Scheduler";

	public virtual bool ClosedInAuto => false;

	protected Page(Hub hub, Shell shell)
	{
		H = hub;
		S = shell;
	}

	public abstract FrameworkElement Build();

	public virtual void Tick()
	{
	}

	public virtual void Shown()
	{
	}
}
