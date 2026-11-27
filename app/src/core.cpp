#include "core.h"

#include <unordered_map>

namespace core {

std::string Take(char* s)
{
	if (s == nullptr) return {};
	std::string r(s);
	dos_free_str(s);
	return r;
}

json::Value TakeJson(char* s)
{
	std::string t = Take(s);
	return t.empty() ? json::Value() : json::Parse(t);
}

// Settings.Get(name, fallback): the stored value (or the built-in default the core knows), else the fallback.
std::string Setting(const char* key, const char* def)
{
	std::string v = Take(dos_setting_get(key));
	return v.empty() ? std::string(def) : v;
}

// Settings.GetBool: an empty or missing value is the fallback, else "true" (any case) is true, anything else false.
bool SettingBool(const char* key, bool def)
{
	std::string v = Setting(key, "");
	if (v.empty()) return def;
	for (char& c : v) c = (char)((c >= 'A' && c <= 'Z') ? c + 32 : c);
	return v == "true";
}

void SetSetting(const char* key, const std::string& value) { dos_setting_set(key, value.c_str()); }

// Settings.SetBool writes "true" / "false".
void SetSettingBool(const char* key, bool value) { dos_setting_set(key, value ? "true" : "false"); }

void Log(const std::string& msg, Sev sev) { dos_log_add(msg.c_str(), (int)sev); }

} // namespace core

namespace L {

namespace {
std::unordered_map<std::string, std::string> g_cache;
std::string g_lang;
bool g_langRead = false;
}

const std::string& T(const char* en)
{
	if (en == nullptr) en = "";
	auto it = g_cache.find(en);
	if (it != g_cache.end()) return it->second;
	std::string t = core::Take(dos_tr(en));
	if (t.empty()) t = en;
	return g_cache.emplace(en, std::move(t)).first->second;
}

std::string F(const char* en, std::initializer_list<std::string_view> args)
{
	const std::string& fmt = T(en);
	std::string out;
	out.reserve(fmt.size() + 16);
	for (size_t i = 0; i < fmt.size(); i++)
	{
		char c = fmt[i];
		if (c == '{' && i + 1 < fmt.size() && fmt[i + 1] == '{') { out += '{'; i++; continue; }
		if (c == '}' && i + 1 < fmt.size() && fmt[i + 1] == '}') { out += '}'; i++; continue; }
		if (c == '{')
		{
			size_t end = fmt.find('}', i);
			if (end != std::string::npos)
			{
				size_t n = 0;
				size_t k = i + 1;
				bool digits = false;
				while (k < end && fmt[k] >= '0' && fmt[k] <= '9') { n = n * 10 + (size_t)(fmt[k] - '0'); k++; digits = true; }
				if (digits && (k == end || fmt[k] == ':' || fmt[k] == ','))
				{
					if (n < args.size()) out += *(args.begin() + n);
					i = end;
					continue;
				}
			}
		}
		out += c;
	}
	return out;
}

const std::string& Lang()
{
	if (!g_langRead)
	{
		g_lang = core::Take(dos_lang());
		if (g_lang.empty()) g_lang = "en";
		g_langRead = true;
	}
	return g_lang;
}

bool Reload()
{
	std::string now = core::Take(dos_lang());
	if (now.empty()) now = "en";
	bool changed = !g_langRead || now != g_lang;
	g_lang = now;
	g_langRead = true;
	if (changed) g_cache.clear();
	return changed;
}

} // namespace L
