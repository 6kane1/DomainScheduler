using System;
using System.Collections.Concurrent;
using System.Drawing;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace DomainScheduler.App;

internal static class IconCache
{
	private static readonly ConcurrentDictionary<string, ImageSource> Cache = new ConcurrentDictionary<string, ImageSource>(StringComparer.OrdinalIgnoreCase);

	public static void Load(string path, int pixels, System.Windows.Controls.Image target)
	{
		if (string.IsNullOrEmpty(path))
		{
			return;
		}
		if (Cache.TryGetValue(path, out var value))
		{
			target.Source = value;
			return;
		}
		Task.Run(delegate
		{
			ImageSource src = null;
			try
			{
				using Icon icon = Icon.ExtractAssociatedIcon(path);
				if (icon != null)
				{
					BitmapSource bitmapSource = Imaging.CreateBitmapSourceFromHIcon(icon.Handle, Int32Rect.Empty, BitmapSizeOptions.FromWidthAndHeight(pixels, pixels));
					bitmapSource.Freeze();
					src = bitmapSource;
				}
			}
			catch (Exception)
			{
			}
			Cache[path] = src;
			if (src != null)
			{
				target.Dispatcher.BeginInvoke((Func<ImageSource>)(() => target.Source = src));
			}
		});
	}
}
