using System;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;

namespace DomainScheduler.App;

/// <summary>
/// Updates and testing (1.16.0 Test 1.9): the update card (was on About), the daily check (was in Settings) and the
/// tester key with the update channel (was in Settings), all in one place.
/// </summary>
internal sealed class UpdatePage : Page
{
	public override string Title => L.T("Updates and testing");

	public override string Section => "Settings";

	public UpdatePage(Hub h, Shell s)
		: base(h, s)
	{
	}

	public override FrameworkElement Build()
	{
		CheckRow upd = new CheckRow(L.T("Check for updates"), L.T("Once a day. Only a small file is read from the Domain OS site; nothing about this PC is sent. Nothing installs until you press Update."), Settings.GetBool("CheckUpdates", true));
		upd.Changed += delegate(bool v)
		{
			Settings.SetBool("CheckUpdates", v);
			H.Configure();
			S.Did(v ? L.F("{0}: on", L.T("Check for updates")) : L.F("{0}: off", L.T("Check for updates")));
		};
		return Ui.Scroll(Ui.Col(new UpdateCard(always: true).Root, Ui.Card(Ui.Col(upd)), Ui.Card(TesterCard())));
	}

	/// <summary>
	/// Tester key (1.15.0 Test 1.1, moved here from Settings in 1.16.0 Test 1.9): the key from the closed testers channel turns the test channel on. It is checked
	/// against the published test builds before it is saved; "Remove" goes back to the stable channel.
	/// </summary>
	private UIElement TesterCard()
	{
		StackPanel col = Ui.Col(Ui.Head(L.T("Tester")), Ui.Note(L.T("Testers get a key in the closed Discord channel. With the key Toolbox also receives the test builds; without it, only stable releases.")));
		TextBlock state = Ui.Text("", Ui.FsNote, "Dim", bold: false, wrap: true);
		string typed = "";
		Button channel = null;
		void Show()
		{
			bool key = TesterKey.Current() != null;
			bool on = AppVersion.Channel == "test";
			state.Text = on ? L.T("Test channel on: this PC receives the test builds.") : (key ? L.T("Key saved, stable releases only: test builds are not offered. A test build already installed stays until a newer stable release comes out.") : L.T("No key: stable channel."));
			state.Foreground = Ui.B(on ? "Accent" : "Dim");
			if (channel != null)
			{
				channel.Visibility = key ? Visibility.Visible : Visibility.Collapsed;
				channel.Content = on ? L.T("Stable releases only") : L.T("Receive test builds");
			}
		}
		// 1.16.0: the tester chooses the channel without removing the key.
		channel = Ui.Btn("", delegate
		{
			bool on = AppVersion.Channel == "test";
			Settings.Set("TestBuilds", on ? "false" : "true");
			S.Did(on ? L.T("Update channel: stable releases only") : L.T("Update channel: test builds and stable releases"));
			Show();
			UpdateService.Instance?.CheckNow();
		}, 190.0);
		TextBox box = Ui.Input("", 360.0, delegate(string v) { typed = v; });
		Button save = null;
		save = Ui.Btn(L.T("Save key"), delegate
		{
			byte[] key = TesterKey.Parse(typed);
			if (key == null)
			{
				state.Text = L.T("This is not a tester key. Copy it whole, starting with DOST-.");
				state.Foreground = Ui.B("Warn");
				return;
			}
			save.IsEnabled = false;
			state.Text = L.T("Checking the key…");
			state.Foreground = Ui.B("Dim");
			Task.Run(async delegate
			{
				string err = await Updater.TryKeyAsync(key);
				S.Dispatcher.BeginInvoke((Action)delegate
				{
					save.IsEnabled = true;
					if (err != null)
					{
						state.Text = L.T(err);
						state.Foreground = Ui.B("Warn");
						return;
					}
					Settings.Set("TesterKey", TesterKey.Format(key));
					box.Text = "";
					S.Did(L.T("Tester key saved: test channel on"));
					Show();
					UpdateService.Instance?.CheckNow();
				});
			});
		});
		Button remove = Ui.Btn(L.T("Remove key"), delegate
		{
			Settings.Reset("TesterKey");
			S.Did(L.T("Tester key removed: stable channel"));
			Show();
			UpdateService.Instance?.CheckNow();
		});
		Show();
		col.Children.Add(Ui.Row(box, save, remove));
		col.Children.Add(state);
		channel.Margin = new Thickness(0.0, 8.0, 0.0, 0.0);
		channel.HorizontalAlignment = HorizontalAlignment.Left;
		col.Children.Add(channel);
		return col;
	}

}
