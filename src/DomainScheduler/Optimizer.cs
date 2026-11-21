using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Threading;
using System.Windows.Threading;

namespace DomainScheduler.App;

internal sealed class Optimizer : IDisposable
{
	private readonly BlockingCollection<Action> _q = new BlockingCollection<Action>();

	private readonly Thread _worker;

	private readonly Dispatcher _ui;

	private readonly Hub _h;

	private readonly DispatcherTimer _timer;

	private int _busy;

	private int _ticks;

	public event Action Evaluate;

	public Optimizer(Dispatcher ui, Hub hub)
	{
		_ui = ui;
		_h = hub;
		_worker = new Thread(Run)
		{
			IsBackground = true,
			Name = "optimizer",
			Priority = ThreadPriority.BelowNormal
		};
		_timer = new DispatcherTimer(DispatcherPriority.Background, ui)
		{
			Interval = TimeSpan.FromSeconds(1.0)
		};
		_timer.Tick += delegate
		{
			Queue(Tick, skipIfBusy: true, reports: true);
		};
	}

	public void Start()
	{
		_worker.Start();
		_timer.Start();
		Queue(Tick, skipIfBusy: false, reports: true);
	}

	private void Tick()
	{
		int num = _ticks++;
		bool flag = !_h.UiVisible && _h.Auto.State == AutoState.Idle && _h.Auto.GamePid == 0 && !_h.Auto.BackgroundStandalone;
		long t0;
		if (!flag || num % 5 == 0)
		{
			long startingTimestamp = (t0 = Stopwatch.GetTimestamp());
			if (_h.UiVisible)
			{
				_h.SampleCpu();
			}
			Part(0);
			_h.LiveTick();
			_h.Auto.Tick();
			_h.TunerTick();
			Part(1);
			bool on = _h.Auto.On;
			int num2 = (on ? Math.Clamp(5, 1, 1000) : (flag ? Math.Max(30, _h.ReapplyTicks) : _h.ReapplyTicks));
			if (flag)
			{
				num2 = (num2 + 4) / 5 * 5;
			}
			_h.Soft.Tick((uint pid) => _h.Engine.ProcessesNow().ContainsKey(pid));
			Part(2);
			if (num % num2 == 0 && !_h.Paused)
			{
				_h.Sys.Pass(_h.Auto.GamePid);
			}
			if (!on && num % (flag ? 10 : _h.ReapplyTicks) == 0 && !_h.Paused)
			{
				_h.Auto.StandaloneBackground(_h.BgOn, _h.GameMaskValue);
			}
			Part(3);
			if (num % 60 == 55 && !on)
			{
				_h.Engine.Prune();
			}
			_h.NoteTick(Stopwatch.GetElapsedTime(startingTimestamp).TotalMilliseconds);
			_h.MeasureSelf();
		}
		void Part(int i)
		{
			long timestamp = Stopwatch.GetTimestamp();
			_h.NotePart(i, (double)(timestamp - t0) * 1000.0 / (double)Stopwatch.Frequency);
			t0 = timestamp;
		}
	}

	private void Run()
	{
		foreach (Action item in _q.GetConsumingEnumerable())
		{
			try
			{
				item();
			}
			catch (Exception ex)
			{
				AppLog.Add(L.F("Worker error: {0}", ex.Message), AppLog.Sev.Error);
			}
			finally
			{
				Interlocked.Exchange(ref _busy, 0);
			}
		}
	}

	public void Queue(Action job, bool skipIfBusy = false, bool reports = false)
	{
		if (_q.IsAddingCompleted || (skipIfBusy && Volatile.Read(ref _busy) != 0))
		{
			return;
		}
		Interlocked.Exchange(ref _busy, 1);
		_q.Add(delegate
		{
			job();
			if (reports)
			{
				_ui.BeginInvoke((Action)delegate
				{
					this.Evaluate?.Invoke();
				});
			}
		});
	}

	public void Run<T>(Func<T> work, Action<T> done)
	{
		Queue(delegate
		{
			T result = default(T);
			Exception error = null;
			try
			{
				result = work();
			}
			catch (Exception ex)
			{
				error = ex;
			}
			_ui.BeginInvoke((Action)delegate
			{
				if (error != null)
				{
					AppLog.Add(L.F("Error: {0}", AppLog.Describe(error)), AppLog.Sev.Error);
				}
				done(result);
				this.Evaluate?.Invoke();
			});
		});
	}

	/// <summary>Runs the job on the worker and waits for it; false when it did not finish within the time (it may still run).</summary>
	public bool RunNow(Action job, int timeoutMs = 15000)
	{
		if (_q.IsAddingCompleted || !_worker.IsAlive)
		{
			job();
			return true;
		}
		// Not disposed here: a job that is still running after the timeout sets it later.
		ManualResetEventSlim done = new ManualResetEventSlim();
		_q.Add(delegate
		{
			try
			{
				job();
			}
			finally
			{
				done.Set();
			}
		});
		return done.Wait(timeoutMs);
	}

	public void Dispose()
	{
		_timer.Stop();
		_q.CompleteAdding();
		if (_worker.IsAlive)
		{
			_worker.Join(5000);
		}
	}
}
