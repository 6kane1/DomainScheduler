using System.IO;
using System.Globalization;

namespace DomainScheduler.App;

/// <summary>What every page shares: the engine, Auto, the preset layer, the loaded presets.</summary>
sealed class Hub
{
    public Engine Engine;
    public AutoController Auto;
    public SysLayer Sys;
    public Optimizer Opt;
    public EtwFrames Frames;
    public WinGameProbe Probe;
    public readonly SoftClose Soft = new();
    public readonly EtwIrq Irq = new();
    /// <summary>22.1: the background layer on its own (the switch BgOn); the mask watched programs get.</summary>
    public volatile bool BgOn = true;
    public ulong GameMaskValue;
    bool _irqFailed;
    public Dictionary<string, Preset> Presets = new(StringComparer.OrdinalIgnoreCase);
    public Dictionary<string, string> Background = new(StringComparer.OrdinalIgnoreCase);
    public List<string> SelfTest = new();
    /// <summary>The preset layer rests after "Revert" until "Reapply" or the next start.</summary>
    public volatile bool Paused;
    public int ReapplyTicks = 10;
    HashSet<string> _watch = new(StringComparer.OrdinalIgnoreCase);

    public ICollection<string> Watch => _watch;

    /// <summary>Reads the presets from the disk and reconciles them with the built-in ones.</summary>
    public void LoadPresets()
    {
        var notes = Store.Load(out var presets, out var bg);
        foreach (var n in notes) AppLog.Add(n, AppLog.Sev.Warn);
        foreach (var n in Store.MarkBuiltin(presets, ThreadIni.LoadBundled().Presets.Values))
            if (!n.Contains(".exe: built-in preset added", StringComparison.Ordinal) || File.Exists(Store.PresetsPath)) AppLog.Add(n);
        Presets = presets;
        Background = bg;
    }

    public void SavePresets(Dictionary<string, Preset> presets)
    {
        Store.Save(presets, Background);
        Presets = presets;
        Configure();
    }

    public static GameMask.Edits MaskEdits()
    {
        ulong.TryParse(Settings.Get("MaskAdd"), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out ulong add);
        ulong.TryParse(Settings.Get("MaskDrop"), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out ulong drop);
        return new GameMask.Edits { Add = add, Drop = drop, Sig = Settings.Get("MaskSig") };
    }

    public static string MaskSpecText()
    {
        string s = Settings.Get("MaskName");
        if (Settings.GetBool("MaskNoSmt")) s += "+nosmt";
        if (Settings.GetBool("MaskNoCore0")) s += "+nocore0";
        return s;
    }

    /// <summary>The mask of watched processes without their own rule: spec, manual edits, IRQ refinement.</summary>
    public static GameMaskInfo MaskInfo(Topo t) => GameMask.Build(t, MaskSpecText(), MaskEdits(), Settings.Get("IrqMode"));

    /// <summary>Hands the current settings to the layers (after any change of presets, watchlist or masks).</summary>
    public void Configure()
    {
        _watch = new HashSet<string>(Settings.Watch(), StringComparer.OrdinalIgnoreCase);
        GameMask.Mode = Settings.Get("IrqMode");
        // 22.9: live DPC/ISR joins the registry pins only when the switch and its flag are on.
        bool etw = Settings.GetBool("EtwIrq") && Flags.Open(Flags.EtwIrq);
        if (etw && !Irq.Available && !_irqFailed) _irqFailed = !Irq.Start();     // unavailable: one warning, the registry pins alone
        else if (!etw)
        {
            if (Irq.Available) Irq.Dispose();
            _irqFailed = false;
        }
        GameMask.NoisyHook = etw && Irq.Available ? Irq.Noisy : null;

        string rule = "auto";
        GameMaskValue = 0;
        try
        {
            var t = TopoBuilder.Live();
            if (t != null)
            {
                var info = MaskInfo(t);
                rule = GameMask.ManualRule(info);
                GameMaskValue = info.Final;
            }
        }
        catch (Exception e) { AppLog.Add(L.F("Mask: {0}", e.Message), AppLog.Sev.Warn); }
        Sys.Configure(Presets, _watch, Settings.LoadAffRules(), rule, Settings.GetBool("SysPresets"), Settings.GetBool("Warmup"));

        // Section 22: the roadmap switches go to the layers; a closed flag keeps its function off.
        BgOn = Settings.GetBool("BgOn");
        Auto.ThreadSets = Settings.GetBool("ThreadSetsOn");
        Auto.TimerRes = Settings.GetBool("TimerResOn") && Flags.Open(Flags.TimerRes);
        Auto.Heat = exe => Presets.TryGetValue(exe, out var p) && !p.Builtin && p.Enabled ? (p.HotFirst, p.HotOthers) : ("none", "none");
        Soft.On = Settings.GetBool("SoftCloseOn") && Flags.Open(Flags.SoftClose);
        Soft.List = new HashSet<string>(Settings.WatchList(Settings.Get("ClosableList")), StringComparer.OrdinalIgnoreCase);
        Auto.Soft = Soft;
        ReapplyTicks = Math.Clamp((int.TryParse(Settings.Get("ReapplyMs"), out int ms) ? ms : ThreadIni.DefaultSleep) / 1000, 1, 1000);
    }

    public void SetWatched(string exe, bool on)
    {
        var list = Settings.Watch();
        list.RemoveAll(x => x.Equals(exe, StringComparison.OrdinalIgnoreCase));
        if (on) list.Add(exe);
        Settings.SetWatch(list);
        Configure();
        AppLog.Add(on ? L.F("{0} added to the watchlist", exe) : L.F("{0} removed from the watchlist", exe));
    }

    /// <summary>Why this process may not be changed (null = it may).</summary>
    public string Eligibility(string exe) => Guard.Eligibility(exe, _watch, Engine.ProcessesNow().Values.Select(v => v.Name));
}
