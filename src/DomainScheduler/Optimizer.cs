using System.Collections.Concurrent;
using System.Windows.Threading;

namespace DomainScheduler.App;

/// <summary>
/// The background loop (section 11): one worker thread draining a BlockingCollection of jobs,
/// a 1 s DispatcherTimer that queues the tick (dropped while the worker is busy), results
/// posted to the UI with the Dispatcher. Every tick Auto runs; every N ticks the preset layer
/// re-applies (N from ReapplyMs, from AutoPolicy while Auto is on); every 60 ticks a prune.
/// </summary>
sealed class Optimizer : IDisposable
{
    readonly BlockingCollection<Action> _q = new();
    readonly Thread _worker;
    readonly Dispatcher _ui;
    readonly Hub _h;
    readonly DispatcherTimer _timer;
    int _busy, _ticks;
    public event Action Evaluate;

    public Optimizer(Dispatcher ui, Hub hub)
    {
        _ui = ui;
        _h = hub;
        _worker = new Thread(Run) { IsBackground = true, Name = "optimizer" };
        _timer = new DispatcherTimer(DispatcherPriority.Background, ui) { Interval = TimeSpan.FromSeconds(1) };
        _timer.Tick += (_, _) => Queue(Tick, skipIfBusy: true, reports: true);
    }

    public void Start()
    {
        _worker.Start();
        _timer.Start();
        Queue(Tick, reports: true);
    }

    void Tick()
    {
        int tick = _ticks++;
        _h.Auto.Tick();
        bool auto = _h.Auto.On;
        int n = auto ? Math.Clamp(AutoPolicy.ReapplyMs / 1000, 1, 1000) : _h.ReapplyTicks;
        _h.Soft.Tick(pid => _h.Engine.ProcessesNow().ContainsKey(pid));
        if (tick % n == 0 && !_h.Paused)
        {
            _h.Sys.Pass(_h.Auto.GamePid);
            if (!auto) _h.Auto.StandaloneBackground(_h.BgOn, _h.GameMaskValue);      // 22.1
        }
        if (tick % 60 == 59 && !auto) _h.Engine.Prune();
    }

    void Run()
    {
        foreach (var job in _q.GetConsumingEnumerable())
        {
            try { job(); }
            catch (Exception e) { AppLog.Add(L.F("Worker error: {0}", e.Message), AppLog.Sev.Error); }
            finally { Interlocked.Exchange(ref _busy, 0); }
        }
    }

    /// <summary>Queues a job; skipIfBusy drops it while another job runs; reports refreshes the UI after it.</summary>
    public void Queue(Action job, bool skipIfBusy = false, bool reports = false)
    {
        if (_q.IsAddingCompleted) return;
        if (skipIfBusy && Volatile.Read(ref _busy) != 0) return;
        Interlocked.Exchange(ref _busy, 1);
        _q.Add(() =>
        {
            job();
            if (reports) _ui.BeginInvoke(() => Evaluate?.Invoke());
        });
    }

    /// <summary>Runs work on the worker and hands its result to the UI thread.</summary>
    public void Run<T>(Func<T> work, Action<T> done)
    {
        Queue(() =>
        {
            T result = default;
            Exception error = null;
            try { result = work(); }
            catch (Exception e) { error = e; }
            _ui.BeginInvoke(() =>
            {
                if (error != null) AppLog.Add(L.F("Error: {0}", error.Message), AppLog.Sev.Error);
                done(result);
                Evaluate?.Invoke();
            });
        });
    }

    /// <summary>Runs a job on the worker and waits for it (exit, session end).</summary>
    public void RunNow(Action job, int timeoutMs = 15000)
    {
        if (_q.IsAddingCompleted || !_worker.IsAlive)
        {
            job();
            return;
        }
        using var done = new ManualResetEventSlim();
        _q.Add(() =>
        {
            try { job(); }
            finally { done.Set(); }
        });
        done.Wait(timeoutMs);
    }

    public void Dispose()
    {
        _timer.Stop();
        _q.CompleteAdding();
        if (_worker.IsAlive) _worker.Join(5000);
    }
}
