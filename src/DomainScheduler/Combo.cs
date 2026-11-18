using System;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

internal static class Combo
{
	public static ComboBox New(double width, params Opt[] opts)
	{
		ComboBox comboBox = new ComboBox
		{
			Width = width,
			Height = 32.0,
			FontSize = Ui.FsBody,
			Margin = new Thickness(0.0, 0.0, 6.0, 0.0)
		};
		foreach (Opt opt in opts)
		{
			comboBox.Items.Add(new ComboBoxItem
			{
				Content = opt.Label,
				Tag = opt.Key
			});
		}
		if (comboBox.Items.Count > 0)
		{
			comboBox.SelectedIndex = 0;
		}
		return comboBox;
	}

	public static bool Select(ComboBox cb, string key)
	{
		for (int i = 0; i < cb.Items.Count; i++)
		{
			if (cb.Items[i] is ComboBoxItem comboBoxItem && string.Equals(comboBoxItem.Tag as string, key, StringComparison.OrdinalIgnoreCase))
			{
				cb.SelectedIndex = i;
				return true;
			}
		}
		return false;
	}

	public static string Key(ComboBox cb)
	{
		return ((cb.SelectedItem as ComboBoxItem)?.Tag as string) ?? "";
	}

	public static void OnChange(ComboBox cb, Action<string> changed)
	{
		cb.SelectionChanged += delegate
		{
			changed(Key(cb));
		};
	}
}
