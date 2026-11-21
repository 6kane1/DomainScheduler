using System.Runtime.InteropServices;

// Every DllImport of this assembly (kernel32, powrprof, setupapi, nvapi64…) is loaded from System32 only: never from
// the program's folder, the current folder or PATH, where another program could put a file with the same name.
[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
