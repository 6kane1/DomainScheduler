using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Management;
using System.Threading.Tasks;
using Microsoft.Win32;

namespace DomainScheduler.App;

internal static class Hardware
{
	public sealed class Gpu
	{
		public string Name = "";

		public string Vendor = "";

		public string Version = "";

		public string Date = "";

		public bool Basic;
	}

	public sealed class Facts
	{
		public string Cpu = "";

		public string CpuVendor = "";

		public string Board = "";

		public string BoardVendor = "";

		public List<Gpu> Gpus = new List<Gpu>();

		public List<string> Audio = new List<string>();

		public List<string> Net = new List<string>();

		public List<string> Problems = new List<string>();

		public int Refresh;

		public int RamRated;

		public int RamNow;
	}

	public const string NvCleanstall = "https://www.techpowerup.com/download/techpowerup-nvcleanstall/";

	public static string ChipsetLink(string vendor)
	{
		if (!(vendor == "amd"))
		{
			if (vendor == "intel")
			{
				return "https://www.intel.com/content/www/us/en/support/detect.html";
			}
			return "";
		}
		return "https://www.amd.com/en/support/download/drivers.html";
	}

	public static string GpuLink(string vendor)
	{
		return vendor switch
		{
			"nvidia" => "https://www.nvidia.com/en-us/drivers/", 
			"amd" => "https://www.amd.com/en/support/download/drivers.html", 
			"intel" => "https://www.intel.com/content/www/us/en/download-center/home.html", 
			_ => "", 
		};
	}

	public static string BoardLink(string vendor)
	{
		return vendor switch
		{
			"asus" => "https://www.asus.com/support/download-center/", 
			"msi" => "https://www.msi.com/support/download", 
			"gigabyte" => "https://www.gigabyte.com/Support", 
			"asrock" => "https://www.asrock.com/support/index.asp", 
			"biostar" => "https://www.biostar.com.tw/app/en/support/download.php", 
			"lenovo" => "https://pcsupport.lenovo.com/", 
			"hp" => "https://support.hp.com/drivers", 
			"dell" => "https://www.dell.com/support/home/", 
			"acer" => "https://www.acer.com/support", 
			"huawei" => "https://consumer.huawei.com/en/support/", 
			"samsung" => "https://www.samsung.com/support/", 
			"razer" => "https://mysupport.razer.com/", 
			"microsoft" => "https://support.microsoft.com/surface", 
			_ => "", 
		};
	}

	private static string Vendor(string text, params (string Key, string Value)[] map)
	{
		for (int i = 0; i < map.Length; i++)
		{
			var (text2, result) = map[i];
			if (text2.Split('|').Any((string part) => text.Contains(part, StringComparison.OrdinalIgnoreCase)))
			{
				return result;
			}
		}
		return "";
	}

	private static IEnumerable<Dictionary<string, object>> Query(string cls, params string[] props)
	{
		List<Dictionary<string, object>> list = new List<Dictionary<string, object>>();
		try
		{
			using ManagementObjectSearcher managementObjectSearcher = new ManagementObjectSearcher("SELECT " + string.Join(",", props) + " FROM " + cls);
			using ManagementObjectCollection managementObjectCollection = managementObjectSearcher.Get();
			foreach (ManagementBaseObject o in managementObjectCollection)
			{
				try
				{
					list.Add(props.ToDictionary((string p) => p, (string p) => o[p]));
				}
				catch (Exception)
				{
				}
				finally
				{
					o.Dispose();
				}
			}
		}
		catch (Exception)
		{
		}
		return list;
	}

	private static string Str(object o)
	{
		return o?.ToString()?.Trim() ?? "";
	}

	private static int Int(object o)
	{
		if (o != null)
		{
			return (int)Math.Min(2147483647L, Convert.ToInt64(o));
		}
		return 0;
	}

	public static Facts Detect()
	{
		Facts facts = new Facts();
		Dictionary<string, object> dictionary = Query("Win32_Processor", "Name", "Manufacturer").FirstOrDefault();
		if (dictionary != null)
		{
			facts.Cpu = Str(dictionary["Name"]);
			facts.CpuVendor = Vendor(Str(dictionary["Manufacturer"]) + " " + facts.Cpu, ("AMD", "amd"), ("Intel", "intel"));
		}
		Dictionary<string, object> dictionary2 = Query("Win32_BaseBoard", "Manufacturer", "Product").FirstOrDefault();
		Dictionary<string, object> dictionary3 = Query("Win32_ComputerSystem", "Manufacturer", "Model").FirstOrDefault();
		string text = ((dictionary3 != null) ? Str(dictionary3["Manufacturer"]) : "");
		string text2 = ((dictionary2 != null) ? Str(dictionary2["Manufacturer"]) : "");
		bool flag = text.Length == 0 || text.Contains("System manufacturer", StringComparison.OrdinalIgnoreCase) || text.Contains("To be filled", StringComparison.OrdinalIgnoreCase) || text.Contains("Default string", StringComparison.OrdinalIgnoreCase);
		facts.Board = (flag ? (text2 + " " + ((dictionary2 != null) ? Str(dictionary2["Product"]) : "")).Trim() : (text + " " + Str(dictionary3["Model"])).Trim());
		facts.BoardVendor = Vendor(text + " " + text2, ("ASUS", "asus"), ("Micro-Star|MSI", "msi"), ("Gigabyte|GIGA-BYTE", "gigabyte"), ("ASRock", "asrock"), ("BIOSTAR", "biostar"), ("LENOVO", "lenovo"), ("HP|Hewlett", "hp"), ("Dell|Alienware", "dell"), ("Acer|Gateway", "acer"), ("HUAWEI|HONOR", "huawei"), ("SAMSUNG", "samsung"), ("Razer", "razer"), ("Microsoft", "microsoft"));
		foreach (Dictionary<string, object> item in Query("Win32_VideoController", "Name", "AdapterCompatibility", "PNPDeviceID", "DriverVersion", "DriverDate", "CurrentRefreshRate"))
		{
			string text3 = Str(item["Name"]);
			facts.Refresh = Math.Max(facts.Refresh, Int(item["CurrentRefreshRate"]));
			if (text3.Length != 0 && !text3.Contains("Remote", StringComparison.OrdinalIgnoreCase) && !text3.Contains("Virtual", StringComparison.OrdinalIgnoreCase))
			{
				string text4 = Str(item["DriverDate"]);
				facts.Gpus.Add(new Gpu
				{
					Name = text3,
					Version = Str(item["DriverVersion"]),
					Date = ((text4.Length >= 8) ? (text4.Substring(0, 4) + "-" + text4.Substring(4, 2) + "-" + text4.Substring(6, 2)) : ""),
					Basic = (text3.Contains("Basic Display", StringComparison.OrdinalIgnoreCase) || text3.Contains("Basic Render", StringComparison.OrdinalIgnoreCase)),
					Vendor = Vendor(text3 + " " + Str(item["AdapterCompatibility"]) + " " + Str(item["PNPDeviceID"]), ("NVIDIA|VEN_10DE", "nvidia"), ("AMD|Radeon|ATI|VEN_1002", "amd"), ("Intel|VEN_8086", "intel"))
				});
			}
		}
		foreach (Dictionary<string, object> item2 in Query("Win32_PhysicalMemory", "Speed", "ConfiguredClockSpeed"))
		{
			facts.RamRated = Math.Max(facts.RamRated, Int(item2["Speed"]));
			int num = Int(item2["ConfiguredClockSpeed"]);
			if (num > 0)
			{
				facts.RamNow = ((facts.RamNow == 0) ? num : Math.Min(facts.RamNow, num));
			}
		}
		facts.Audio = (from a in Query("Win32_SoundDevice", "Name")
			select Str(a["Name"]) into n
			where n.Length > 0 && !n.Contains("Virtual", StringComparison.OrdinalIgnoreCase)
			select n).ToList();
		facts.Net = (from a in Query("Win32_NetworkAdapter WHERE PhysicalAdapter = TRUE", "Name")
			select Str(a["Name"]) into n
			where n.Length > 0 && !n.Contains("Virtual", StringComparison.OrdinalIgnoreCase) && !n.Contains("Bluetooth", StringComparison.OrdinalIgnoreCase)
			select n).ToList();
		facts.Problems = (from p in Query("Win32_PnPEntity WHERE ConfigManagerErrorCode <> 0 AND ConfigManagerErrorCode <> 22", "Name", "DeviceID")
			select (Str(p["Name"]).Length <= 0) ? Str(p["DeviceID"]) : Str(p["Name"])).ToList();
		return facts;
	}

	public static void OpenLink(string url)
	{
		if (string.IsNullOrEmpty(url) || !url.StartsWith("https://", StringComparison.Ordinal))
		{
			return;
		}
		try
		{
			Process.Start(new ProcessStartInfo(url)
			{
				UseShellExecute = true
			});
		}
		catch (Exception ex)
		{
			AppLog.Add(L.F("Link: {0}", ex.Message), AppLog.Sev.Warn);
		}
	}

	public static (int Code, string Output) Run(string exe, string args, int timeoutMs = 600000)
	{
		try
		{
			using Process process = new Process();
			process.StartInfo = new ProcessStartInfo(exe, args)
			{
				UseShellExecute = false,
				CreateNoWindow = true,
				RedirectStandardOutput = true,
				RedirectStandardError = true
			};
			process.Start();
			Task<string> task = process.StandardOutput.ReadToEndAsync();
			Task<string> task2 = process.StandardError.ReadToEndAsync();
			if (!process.WaitForExit(timeoutMs))
			{
				return (Code: -1, Output: L.T("timed out"));
			}
			return (Code: process.ExitCode, Output: (task.Result + "\n" + task2.Result).Trim());
		}
		catch (Exception ex)
		{
			return (Code: -1, Output: ex.Message);
		}
	}
}
