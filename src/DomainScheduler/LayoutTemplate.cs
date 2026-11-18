namespace DomainScheduler.App;

internal sealed class LayoutTemplate
{
	public string Key = "flat";

	public bool Stacked;

	public static LayoutTemplate For(Topo t)
	{
		if (t != null)
		{
			if (!t.Hybrid)
			{
				if (!t.SMT)
				{
					return new LayoutTemplate
					{
						Key = "flat"
					};
				}
				return new LayoutTemplate
				{
					Key = "smt",
					Stacked = true
				};
			}
			return new LayoutTemplate
			{
				Key = "hybrid",
				Stacked = true
			};
		}
		return new LayoutTemplate();
	}
}
