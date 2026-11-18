using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;

namespace DomainScheduler.App;

internal sealed class Hub
{
	public Engine Engine;

	public AutoController Auto;

	public SysLayer Sys;

	public Optimizer Opt;

	public EtwFrames Frames;

	public volatile bool UiVisible;

	public WinGameProbe Probe;

	public readonly SoftClose Soft = new SoftClose();

	public readonly EtwIrq Irq = new EtwIrq();

	public volatile bool BgOn = true;

	public ulong GameMaskValue;

	private bool _irqFailed;

	private long[] _cpuIdle;

	private long[] _cpuTotal;

	private readonly Process _self = Process.GetCurrentProcess();

	private TimeSpan _selfCpu;

	private DateTime _selfAt;

	private double _uiMsSum;

	private double _tickMsSum;

	private int _uiCount;

	private int _tickCount;

	private int _uiSkipped;

	private double _chartMsSum;

	private int _chartCount;

	private static readonly string[] PartNames = new string[4] { "cpu load", "Auto", "soft close", "presets/background" };

	private readonly double[] _partSum = new double[4];

	private double[] _partAvg = new double[4];


	public volatile double[] CpuLoad = Array.Empty<double>();

	public Dictionary<string, Preset> Presets = new Dictionary<string, Preset>(StringComparer.OrdinalIgnoreCase);

	public Dictionary<string, string> Background = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);

	public List<string> SelfTest = new List<string>();

	public volatile bool Paused;

	public int ReapplyTicks = 10;

	private HashSet<string> _watch = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

	public double SelfCpu { get; private set; }

	public double UiMs { get; private set; }

	public double TickMs { get; private set; }

	public int UiPerMinute { get; private set; }

	public int UiSkippedPerMinute { get; private set; }

	/// <summary>Updates of the home screen's live frame-time chart in the last minute and their average time.</summary>
	public int ChartPerMinute { get; private set; }

	public double ChartMs { get; private set; }

	public ICollection<string> Watch => _watch;

	public void NoteUi(double ms)
	{
		lock (_self)
		{
			_uiMsSum += ms;
			_uiCount++;
		}
	}

	public void NoteChart(double ms)
	{
		lock (_self)
		{
			_chartMsSum += ms;
			_chartCount++;
		}
	}

	public void NoteUiSkipped()
	{
		lock (_self)
		{
			_uiSkipped++;
		}
	}

	public void NoteTick(double ms)
	{
		lock (_self)
		{
			_tickMsSum += ms;
			_tickCount++;
		}
	}

	public void NotePart(int i, double ms)
	{
		lock (_self)
		{
			_partSum[i] += ms;
		}
	}

	public void MeasureSelf()
	{
		DateTime utcNow = DateTime.UtcNow;
		if (_selfAt != default(DateTime) && (utcNow - _selfAt).TotalSeconds < 60.0)
		{
			return;
		}
		try
		{
			_self.Refresh();
			TimeSpan totalProcessorTime = _self.TotalProcessorTime;
			lock (_self)
			{
				if (_selfAt != default(DateTime))
				{
					SelfCpu = (totalProcessorTime - _selfCpu).TotalSeconds / Math.Max(1.0, (utcNow - _selfAt).TotalSeconds);
					UiMs = ((_uiCount > 0) ? (_uiMsSum / (double)_uiCount) : 0.0);
					TickMs = ((_tickCount > 0) ? (_tickMsSum / (double)_tickCount) : 0.0);
					UiPerMinute = _uiCount;
					_partAvg = _partSum.Select((double x) => (_tickCount <= 0) ? 0.0 : (x / (double)_tickCount)).ToArray();
					UiSkippedPerMinute = _uiSkipped;
					ChartPerMinute = _chartCount;
					ChartMs = ((_chartCount > 0) ? (_chartMsSum / (double)_chartCount) : 0.0);
				}
				_selfCpu = totalProcessorTime;
				_selfAt = utcNow;
				_uiMsSum = (_tickMsSum = 0.0);
				Array.Clear(_partSum);
				_uiCount = (_tickCount = (_uiSkipped = 0));
				_chartMsSum = 0.0;
				_chartCount = 0;
			}
		}
		catch (Exception)
		{
		}
	}

	/// <summary>
	/// Writes the voltage boost and both clock offsets and reads each back: the lines say what is in force.
	/// guard: remember when the values were set; if Windows crashes after that (blue screen or sudden power-off),
	/// the profile is not applied at the next start.
	/// </summary>
	public List<string> GpuOcApply(int coreKHz, int memKHz, int voltPct, bool guard = true)
	{
		List<string> list = new List<string>();
		if (guard)
		{
			Settings.Set("GpuOcAppliedUtc", DateTime.UtcNow.ToString("o", System.Globalization.CultureInfo.InvariantCulture));
		}
		int? num = NvOc.SetVoltage(voltPct);
		list.Add(num.HasValue ? L.F("Voltage boost: {0}%", num.Value) : L.F("Voltage boost: {0}", NvOc.Error));
		int? num2 = NvOc.Set(0, coreKHz);
		list.Add(num2.HasValue ? L.F("Core clock offset: {0:+0;-0;0} MHz", num2.Value / 1000) : L.F("Core clock: {0}", NvOc.Error));
		int? num3 = NvOc.Set(4, memKHz);
		list.Add(num3.HasValue ? L.F("Memory clock offset: {0:+0;-0;0} MHz", num3.Value / 1000) : L.F("Memory clock: {0}", NvOc.Error));
		return list;
	}

	/// <summary>The saved profiles; the "apply at start" values of 1.12 and older become a profile the first time.</summary>
	public static GpuProfiles GpuProfilesLoad()
	{
		bool exists = File.Exists(GpuProfiles.FilePath);
		GpuProfiles g = GpuProfiles.Load();
		if (GpuOcLogic.Migrate(g, exists, Settings.GetBool("GpuOcAtStart"), Settings.GetInt("GpuOcCoreKHz", 0), Settings.GetInt("GpuOcMemKHz", 0), Settings.GetInt("GpuOcVoltPct", 0), L.T("Saved values")))
		{
			try
			{
				g.Save();
				Settings.SetBool("GpuOcAtStart", value: false);
			}
			catch (Exception)
			{
			}
		}
		return g;
	}

	public void GpuOcStartup()
	{
		GpuProfiles g = GpuProfilesLoad();
		// Skipped only when Windows went down unexpectedly after the last apply (blue screen, freeze with a power-off):
		// a normal restart or shutdown keeps the profile.
		bool pending = DateTime.TryParse(Settings.Get("GpuOcAppliedUtc"), System.Globalization.CultureInfo.InvariantCulture, System.Globalization.DateTimeStyles.RoundtripKind, out DateTime appliedUtc)
			&& BootGuard.UnexpectedShutdownSince(appliedUtc);
		(GpuOcLogic.Start What, GpuProfile Profile) d = GpuOcLogic.DecideStart(Flags.Open("gpu-oc"), g, pending);
		if (d.What == GpuOcLogic.Start.SkipCrash)
		{
			AppLog.Add(L.F("GPU profile \"{0}\" was NOT re-applied: Windows crashed after it was set (a blue screen or a sudden power-off). Lower the values, apply them by hand and mark the profile for the start again.", d.Profile.Name), AppLog.Sev.Warn);
			g.AtStart = "";
			g.Skipped = d.Profile.Name;
			g.SkippedUtc = DateTime.UtcNow;
			Settings.Set("GpuOcAppliedUtc", "");
			try
			{
				g.Save();
			}
			catch (Exception)
			{
			}
		}
		else if (d.What == GpuOcLogic.Start.Apply)
		{
			AppLog.Add(L.F("GPU profile \"{0}\" applied at start", d.Profile.Name));
			foreach (string item in GpuOcApply(d.Profile.CoreMHz * 1000, d.Profile.MemMHz * 1000, d.Profile.VoltPct))
			{
				AppLog.Add(item);
			}
		}
	}

	public string SelfLine()
	{
		if (UiPerMinute + UiSkippedPerMinute != 0 || TickMs != 0.0)
		{
			return L.F("Own load (last minute): {0:0.00}% of one core; background tick {1:0.0} ms; window drawn {2} times, {3:0.0} ms each; skipped while hidden {4}; GPU not used (software drawing)", SelfCpu * 100.0, TickMs, UiPerMinute, UiMs, UiSkippedPerMinute) + ((ChartPerMinute > 0) ? ("; " + L.F("home chart updated {0} times, {1:0.00} ms each", ChartPerMinute, ChartMs)) : "") + ((TickMs > 0.0) ? ("; tick parts: " + string.Join(", ", PartNames.Select((string n, int i) => L.F("{0} {1:0.0} ms", n, _partAvg[i])))) : "");
		}
		return L.T("Own load: measured after the first minute");
	}

	public void SampleCpu()
	{
		if (!Engine.Native.CpuTimes(out var idle, out var total))
		{
			return;
		}
		if (_cpuIdle != null && _cpuIdle.Length == idle.Length)
		{
			double[] array = new double[idle.Length];
			for (int i = 0; i < idle.Length; i++)
			{
				long num = total[i] - _cpuTotal[i];
				long num2 = idle[i] - _cpuIdle[i];
				array[i] = ((num > 0) ? Math.Clamp(1.0 - (double)num2 / (double)num, 0.0, 1.0) : 0.0);
			}
			CpuLoad = array;
		}
		_cpuIdle = idle;
		_cpuTotal = total;
	}

	public void LoadPresets()
	{
		Dictionary<string, Preset> presets;
		Dictionary<string, string> bg;
		foreach (string item in Store.Load(out presets, out bg))
		{
			AppLog.Add(item, AppLog.Sev.Warn);
		}
		foreach (string item2 in Store.MarkBuiltin(presets, ThreadIni.LoadBundled().Presets.Values))
		{
			if (!item2.Contains(".exe: built-in preset added", StringComparison.Ordinal) || File.Exists(Store.PresetsPath))
			{
				AppLog.Add(item2);
			}
		}
		Presets = presets;
		Background = bg;
	}

	public static GameMask.Edits MaskEdits()
	{
		ulong.TryParse(Settings.Get("MaskAdd"), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out var result);
		ulong.TryParse(Settings.Get("MaskDrop"), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out var result2);
		return new GameMask.Edits
		{
			Add = result,
			Drop = result2,
			Sig = Settings.Get("MaskSig")
		};
	}

	public static string MaskSpecText()
	{
		string text = Settings.Get("MaskName");
		if (Settings.GetBool("MaskNoSmt"))
		{
			text += "+nosmt";
		}
		if (Settings.GetBool("MaskNoCore0"))
		{
			text += "+nocore0";
		}
		return text;
	}

	public static GameMaskInfo MaskInfo(Topo t)
	{
		return GameMask.Build(t, MaskSpecText(), MaskEdits(), Settings.Get("IrqMode"));
	}

	public void Configure()
	{
		_watch = new HashSet<string>(Settings.Watch(), StringComparer.OrdinalIgnoreCase);
		if (Engine != null) Engine.TimerGuard = Settings.GetBool("TimerGuard", fallback: true);
		GameMask.Mode = Settings.Get("IrqMode");
		// Live DPC/ISR runs only while a game is handled (LiveTick); the old per-processor hook is not used.
		GameMask.NoisyHook = null;
		LiveIrqOn = Settings.GetBool("LiveIrq", fallback: true);
		LiveIrqMask = Settings.GetBool("LiveIrqMask", fallback: true);
		string maskName = "auto";
		GameMaskValue = 0uL;
		try
		{
			Topo topo = TopoBuilder.Live();
			if (topo != null)
			{
				GameMaskInfo gameMaskInfo = MaskInfo(topo);
				maskName = GameMask.ManualRule(gameMaskInfo);
				GameMaskValue = gameMaskInfo.Final;
			}
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Mask: {0}", ex.Message), AppLog.Sev.Warn);
		}
		Sys.Configure(Presets, _watch, Settings.LoadAffRules(), maskName, Settings.GetBool("SysPresets"));
		BgOn = Settings.GetBool("BgOn");
		Auto.TimerRes = false;
		Auto.Heat = (string exe) => (!Presets.TryGetValue(exe, out var value) || value.Builtin || !value.Enabled) ? (First: "none", Others: "none") : (First: value.HotFirst, Others: value.HotOthers);
		Soft.On = false;
		Soft.List = new HashSet<string>(Settings.WatchList(Settings.Get("ClosableList")), StringComparer.OrdinalIgnoreCase);
		Auto.Soft = Soft;
		ReapplyTicks = Math.Clamp((int.TryParse(Settings.Get("ReapplyMs"), out var result) ? result : 10000) / 1000, 1, 1000);
	}

	// ---- the automatic A/B tuner ---------------------------------------------------------------------------------

	public readonly AbTuner Tuner = new AbTuner();
	TunerVariant _tunerOriginal;

	/// <summary>Starts the tuner while Auto handles a game; returns a message for the log.</summary>
	public string TunerStart()
	{
		if (Tuner.Running) return L.T("Auto-tune is already running");
		if (!Auto.On || Auto.GamePid == 0) return L.T("Auto-tune: turn Auto on and start the game first, then press the button again");
		if (Frames == null || !Frames.Available) return L.T("Auto-tune needs the frame-time measurement, which is not available here");
		bool live = LiveIrqOn;
		var variants = new List<TunerVariant>
		{
			new TunerVariant(L.T("Roles"), false, false),
			new TunerVariant(L.T("Dynamic Top-5"), true, false),
		};
		if (live)
		{
			variants.Add(new TunerVariant(L.T("Roles + interrupt cores avoided"), false, true));
			variants.Add(new TunerVariant(L.T("Dynamic Top-5 + interrupt cores avoided"), true, true));
		}
		bool curLive = live && LiveIrqMask;
		int baseline = variants.FindIndex(v => v.Top5 == Auto.Top5Mode && v.LiveMask == curLive);
		if (baseline < 0) baseline = 0;
		_tunerOriginal = variants[baseline];
		TunerPending = null;
		TunerOffer = "";
		TunerNoGain = false;
		var first = Tuner.Start(variants, baseline, DateTime.UtcNow);
		ApplyVariant(first);
		return L.F("Auto-tune started: {0} combinations, about {1} minutes. Keep playing as usual; the game must stay in front.", variants.Count, (int)Math.Ceiling(Tuner.Total.TotalMinutes));
	}

	public void TunerStop(string why)
	{
		if (!Tuner.Running) return;
		Tuner.Cancel();
		if (_tunerOriginal != null) ApplyVariant(_tunerOriginal);
		AppLog.Add(L.F("Auto-tune stopped ({0}): the previous settings are back", why), AppLog.Sev.Warn);
	}

	/// <summary>A combination the last auto-tune found better; null when none. Applied only by TunerApplyPending.</summary>
	public volatile TunerVariant TunerPending;
	/// <summary>The text of the offer (what changes and by how much).</summary>
	public volatile string TunerOffer = "";
	/// <summary>The last auto-tune ended with no better combination (shown once on the home screen).</summary>
	public volatile bool TunerNoGain;

	string TunerOfferText(int winner)
	{
		var (wl, wh) = Tuner.Stats(winner);
		var (bl, bh) = Tuner.Stats(Tuner.Baseline);
		return L.F("Auto-tune found a better combination: {0}. 1% low {1:0} → {2:0} FPS, hitches {3} → {4}.", Tuner.Variants[winner].Name, bl, wl, bh, wh);
	}

	/// <summary>On the worker: applies and saves the combination the auto-tune found.</summary>
	public void TunerApplyPending()
	{
		var v = TunerPending;
		if (v == null) return;
		if (!Tuner.Running) ApplyVariant(v);
		Settings.SetBool("Top5On", v.Top5);
		if (LiveIrqOn) Settings.SetBool("LiveIrqMask", v.LiveMask);
		TunerPending = null;
		TunerOffer = "";
		AppLog.Add(L.F("Auto-tune result applied: {0}", v.Name));
	}

	public void TunerDismiss()
	{
		if (TunerPending != null) AppLog.Add(L.T("Auto-tune result not applied: the current settings stay"));
		TunerPending = null;
		TunerOffer = "";
		TunerNoGain = false;
	}

	void ApplyVariant(TunerVariant v)
	{
		if (Auto.Top5Mode != v.Top5) Auto.SetTop5(v.Top5);
		LiveIrqMask = v.LiveMask;
		GameMask.BusyHook = LiveIrqOn && LiveIrqMask && Irq.Available ? new Func<Topo, ulong?>(Irq.BusyCoresIfReady) : null;
	}

	/// <summary>Once a second on the worker.</summary>
	public void TunerTick()
	{
		if (!Tuner.Running) return;
		if (!Auto.On || Auto.GamePid == 0)
		{
			TunerStop(L.T("the game ended or Auto was turned off"));
			return;
		}
		var step = Tuner.Tick(DateTime.UtcNow, (from, to) => Frames.Intervals(from, to), out var next);
		if (step == AbTuner.Step.Switch)
		{
			ApplyVariant(next);
			AppLog.Add(L.F("Auto-tune {0}/{1}: {2}", Tuner.StepIndex + 1, Tuner.StepCount, next.Name));
		}
		else if (step == AbTuner.Step.Finished)
		{
			// The current settings come back; a better combination is only offered (TunerPending), the user applies it.
			ApplyVariant(_tunerOriginal);
			TunerPending = Tuner.Winner >= 0 && Tuner.Winner != Tuner.Baseline ? Tuner.Variants[Tuner.Winner] : null;
			TunerOffer = TunerPending == null ? "" : TunerOfferText(Tuner.Winner);
			TunerNoGain = TunerPending == null;
			AppLog.Add(L.T("Auto-tune results (1% low per round):") + "\n" + Tuner.Table());
			AppLog.Add(Tuner.Verdict, Tuner.Winner >= 0 ? AppLog.Sev.Info : AppLog.Sev.Warn);
			// The result also goes to its own file at once: the log is a ring buffer and is written only on exit.
			try
			{
				string file = System.IO.Path.Combine(Store.Dir, "autotune-" + DateTime.Now.ToString("yyyyMMdd-HHmm", System.Globalization.CultureInfo.InvariantCulture) + ".txt");
				System.IO.Directory.CreateDirectory(Store.Dir);
				System.IO.File.WriteAllText(file, L.T("Auto-tune results (1% low per round):") + Environment.NewLine + Tuner.Table() + Environment.NewLine + Environment.NewLine + Tuner.Verdict + Environment.NewLine, new System.Text.UTF8Encoding(true));
				AppLog.Add(L.F("Auto-tune results saved: {0}", file));
			}
			catch (Exception ex) { AppLog.Add(L.F("Auto-tune results could not be saved: {0}", ex.Message), AppLog.Sev.Warn); }
		}
	}

	/// <summary>Live DPC/ISR trace while a game runs (latency watchdog); LiveIrqMask: Auto keeps the game off busy interrupt cores.</summary>
	public bool LiveIrqOn = true, LiveIrqMask = true;

	int _liveTicks;

	/// <summary>Once a second on the worker: starts the live trace when a game is handled, stops it when none is.</summary>
	public void LiveTick()
	{
		bool game = Auto.GamePid != 0 || Auto.BackgroundStandalone;
		if (!LiveIrqOn || !game)
		{
			GameMask.BusyHook = null;
			if (Irq.Available)
			{
				ReportLong();
				Irq.Pause();
			}
			_irqFailed = false;
			_liveTicks = 0;
			return;
		}
		if (!Irq.Available && !_irqFailed) _irqFailed = !Irq.Start();
		if (!Irq.Available) { GameMask.BusyHook = null; return; }
		GameMask.BusyHook = LiveIrqMask ? new Func<Topo, ulong?>(Irq.BusyCoresIfReady) : null;
		if (++_liveTicks % 60 == 0) ReportLong();
	}

	/// <summary>The latency watchdog: long DPCs/interrupts of the last minute, per driver, into the log.</summary>
	void ReportLong()
	{
		foreach (var (driver, lng, veryLong, maxMs) in Irq.TakeLong().Take(5))
		{
			if (veryLong > 0)
				AppLog.Add(L.F("Driver latency: {0} — {1} time(s) 1 ms or longer, longest {2:0.00} ms (last minute)", driver, veryLong, maxMs), AppLog.Sev.Warn);
			else if (lng >= 5)
				AppLog.Add(L.F("Driver latency: {0} — {1} time(s) 0.5–1 ms, longest {2:0.00} ms (last minute)", driver, lng, maxMs));
		}
	}

	public void SetWatched(string exe, bool on)
	{
		List<string> list = Settings.Watch();
		list.RemoveAll((string x) => x.Equals(exe, StringComparison.OrdinalIgnoreCase));
		if (on)
		{
			list.Add(exe);
		}
		Settings.SetWatch(list);
		Configure();
		AppLog.Add(on ? L.F("{0} added to the watchlist", exe) : L.F("{0} removed from the watchlist", exe));
	}

	public string Eligibility(string exe)
	{
		return Guard.Eligibility(exe, _watch, Engine.ProcessesNow().Values.Select(((string Name, uint Parent) v) => v.Name));
	}
}
