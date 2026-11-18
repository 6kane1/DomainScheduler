using System.Windows.Controls.Primitives;

namespace DomainScheduler.App;

internal sealed class FactGrid : UniformGrid
{
	public FactGrid(int columns = 2)
	{
		base.Columns = columns;
	}

	public FactGrid Add(string k, string v)
	{
		base.Children.Add(Ui.Pair(k, v));
		return this;
	}
}
