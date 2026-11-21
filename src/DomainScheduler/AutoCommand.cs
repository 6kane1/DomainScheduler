using System;
using System.Diagnostics;
using System.Threading;

namespace DomainScheduler;

/// <summary>
/// "--auto on|off|status" (1.16.0-test1.3), for Domain Bench: alternating measurements without a person at the PC.
/// The running Toolbox mirrors Auto into two manual-reset events (Local\DomainScheduler.Auto.IsOn / .IsOff) and listens
/// for two requests (Local\DomainScheduler.Auto.TurnOn / .TurnOff). A request does exactly what the Auto button does,
/// on the UI thread, so everything Auto changes is reverted by Auto itself. Session-local names: only programs of the
/// same signed-in session can reach them, and an elevated Toolbox's objects are not writable from a non-elevated one.
/// Exit codes of the command: 0 Auto on, 1 Auto off, 2 no answer in time, 3 Toolbox is not running, 4 unknown argument.
/// </summary>
internal sealed class AutoCommand
{
	const string IsOnName = "Local\\DomainScheduler.Auto.IsOn", IsOffName = "Local\\DomainScheduler.Auto.IsOff";
	const string OnName = "Local\\DomainScheduler.Auto.TurnOn", OffName = "Local\\DomainScheduler.Auto.TurnOff";
	readonly EventWaitHandle _isOn, _isOff, _turnOn, _turnOff;
	readonly Func<bool> _on;

	public AutoCommand(Func<bool> on, Action turnOn, Action turnOff)
	{
		_on = on;
		_isOn = new EventWaitHandle(false, EventResetMode.ManualReset, IsOnName);
		_isOff = new EventWaitHandle(true, EventResetMode.ManualReset, IsOffName);
		_turnOn = new EventWaitHandle(false, EventResetMode.AutoReset, OnName);
		_turnOff = new EventWaitHandle(false, EventResetMode.AutoReset, OffName);
		Mirror();
		var t = new Thread(() =>
		{
			var hs = new WaitHandle[] { _turnOn, _turnOff };
			while (true)
			{
				int i;
				try { i = WaitHandle.WaitAny(hs); } catch (Exception) { return; }
				try
				{
					if (i == 0) { AppLog.Add("Auto on (--auto on)", AppLog.Sev.Info); turnOn(); }
					else if (i == 1) { AppLog.Add("Auto off (--auto off)", AppLog.Sev.Info); turnOff(); }
				}
				catch (Exception ex) { AppLog.Add("--auto: " + ex.Message, AppLog.Sev.Warn); }
				Mirror();
			}
		}) { IsBackground = true, Name = "auto-command" };
		t.Start();
	}

	public void Mirror()
	{
		try
		{
			if (_on()) { _isOff.Reset(); _isOn.Set(); }
			else { _isOn.Reset(); _isOff.Set(); }
		}
		catch (Exception) { }
	}

	/// <summary>The command side: ask the running Toolbox and wait up to 20 s for the state to follow.</summary>
	public static int Send(string what)
	{
		what = (what ?? "status").Trim().ToLowerInvariant();
		if (what is not ("on" or "off" or "status")) return 4;
		EventWaitHandle isOn, isOff;
		try { isOn = EventWaitHandle.OpenExisting(IsOnName); isOff = EventWaitHandle.OpenExisting(IsOffName); }
		catch (Exception) { return 3; }
		using (isOn)
		using (isOff)
		{
			if (what == "status") return isOn.WaitOne(0) ? 0 : 1;
			bool wantOn = what == "on";
			if (wantOn ? isOn.WaitOne(0) : isOff.WaitOne(0)) return wantOn ? 0 : 1;
			try { using var req = EventWaitHandle.OpenExisting(wantOn ? OnName : OffName); req.Set(); }
			catch (Exception) { return 3; }
			var sw = Stopwatch.StartNew();
			while (sw.ElapsedMilliseconds < 20000)
			{
				if (wantOn ? isOn.WaitOne(200) : isOff.WaitOne(200)) return wantOn ? 0 : 1;
			}
			return 2;
		}
	}
}
