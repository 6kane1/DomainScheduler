using System;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

// DllImports of the window (user32, dwmapi) are loaded from System32 only.
[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.System32)]

namespace DomainScheduler.App;

/// <summary>
/// The entry point (1.15.0 Test 1.3). Before WPF loads any native library, the program makes sure the .NET host
/// unpacked them into the admin-only state folder (<see cref="NativeHome"/>), starting itself again if needed.
/// </summary>
public static class Program
{
	[STAThread]
	public static int Main(string[] args)
	{
		int? done = NativeHome.RelaunchIfNeeded(args);
		if (done.HasValue)
		{
			return done.Value;
		}
		return RunApp();
	}

	/// <summary>Separate and never inlined: compiling Main must not touch the WPF types.</summary>
	[MethodImpl(MethodImplOptions.NoInlining)]
	private static int RunApp()
	{
		App app = new App();
		app.InitializeComponent();
		return app.Run();
	}
}
