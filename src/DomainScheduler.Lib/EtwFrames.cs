using System;
using System.Collections.Generic;
using System.Threading;
using Microsoft.Diagnostics.Tracing;
using Microsoft.Diagnostics.Tracing.Session;

namespace DomainScheduler;

public sealed class EtwFrames : IFrameSource, IDisposable
{
	private static readonly Guid Dxgi = new Guid("CA11C036-0102-4A2D-A6AD-F03CFED5D3C9");

	private const string SessionName = "DomainScheduler-Frames";

	private const int PresentStart = 42;

	// A minute of frames at up to ~660 FPS for the home chart (1.14.0; was 20000).
	private const int Keep = 40000;

	private readonly object _gate = new object();

	private readonly Queue<DateTime> _frames = new Queue<DateTime>();

	private TraceEventSession _session;

	private Thread _thread;

	private Thread _flusher;

	private volatile uint _pid;

	public bool Available { get; private set; }

	public string Name => "ETW DXGI Present";

	public string Error { get; private set; } = "";


	public void Start()
	{
		try
		{
			if (!TraceEventSession.IsElevated().GetValueOrDefault())
			{
				Error = L.T("no administrator rights");
			}
			else
			{
				Available = true;
			}
		}
		catch (Exception ex)
		{
			Error = ex.Message;
			Available = false;
		}
	}

	private void Open(uint pid)
	{
		Close();
		try
		{
			_session = new TraceEventSession("DomainScheduler-Frames")
			{
				StopOnDispose = true
			};
			TraceEventProviderOptions options = new TraceEventProviderOptions
			{
				ProcessIDFilter = new List<int> { (int)pid }
			};
			_session.EnableProvider(Dxgi, TraceEventLevel.Informational, ulong.MaxValue, options);
			_session.Source.Dynamic.All += OnEvent;
			TraceEventSession session = _session;
			_thread = new Thread((ThreadStart)delegate
			{
				try
				{
					session.Source.Process();
				}
				catch (Exception ex2)
				{
					Error = ex2.Message;
				}
			})
			{
				IsBackground = true,
				Name = "etw-frames",
				Priority = ThreadPriority.BelowNormal
			};
			_thread.Start();
			// Real-time ETW hands buffers over about once a second or when they fill; a flush twice a second keeps
			// the frames close to the clock for the tuner and the watchdog (as Domain FrameTime does).
			_flusher = new Thread((ThreadStart)delegate
			{
				while (_session == session)
				{
					Thread.Sleep(500);
					try { session.Flush(); } catch (Exception) { }
				}
			})
			{
				IsBackground = true,
				Name = "etw-frames-flush",
				Priority = ThreadPriority.BelowNormal
			};
			_flusher.Start();
		}
		catch (Exception ex)
		{
			Error = ex.Message;
			Available = false;
			Close();
		}
	}

	private void Close()
	{
		try
		{
			_session?.Dispose();
		}
		catch (Exception)
		{
		}
		_session = null;
		_thread = null;
	}

	private void OnEvent(TraceEvent e)
	{
		if (_pid == 0 || e.ProcessID != _pid || e.ID != (TraceEventID)42 || e.ProviderGuid != Dxgi)
		{
			return;
		}
		lock (_gate)
		{
			_frames.Enqueue(e.TimeStamp.ToUniversalTime());
			while (_frames.Count > Keep)
			{
				_frames.Dequeue();
			}
		}
	}

	public void Watch(uint pid)
	{
		lock (_gate)
		{
			if (pid != _pid || (pid != 0 && _session == null))
			{
				_pid = pid;
				_frames.Clear();
				if (pid == 0)
				{
					Close();
				}
				else if (Available)
				{
					Open(pid);
				}
			}
		}
	}

	public List<double> Intervals(DateTime from, DateTime to)
	{
		List<double> list = new List<double>();
		lock (_gate)
		{
			DateTime dateTime = default(DateTime);
			foreach (DateTime frame in _frames)
			{
				if (dateTime != default(DateTime) && frame > from && frame <= to)
				{
					list.Add((frame - dateTime).TotalMilliseconds);
				}
				dateTime = frame;
			}
			return list;
		}
	}

	/// <summary>Present times (UTC, ascending) after `from`, copied into `into` (reused by the caller: no garbage per update).</summary>
	public int Times(DateTime from, List<DateTime> into)
	{
		into.Clear();
		lock (_gate)
		{
			foreach (DateTime frame in _frames)
			{
				if (frame > from)
				{
					into.Add(frame);
				}
			}
		}
		return into.Count;
	}

	public void Dispose()
	{
		lock (_gate)
		{
			Close();
		}
		Available = false;
	}
}
