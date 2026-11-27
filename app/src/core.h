// C++ helpers over the core's C ABI used by the whole app: owned strings, translations (L), settings, the log.
// Owner: app shell.
#pragma once
#include <cstdint>
#include <initializer_list>
#include <string>
#include <string_view>

#include "json.h"
#include "shell_core.h"

namespace core {

/// Takes a string the core returned (may be NULL) and frees it.
std::string Take(char* s);
/// Takes and parses a JSON string the core returned.
json::Value TakeJson(char* s);

std::string Setting(const char* key, const char* def = "");
bool SettingBool(const char* key, bool def = false);
void SetSetting(const char* key, const std::string& value);
void SetSettingBool(const char* key, bool value);

enum class Sev { Info = 0, Warn = 1, Error = 2 };
void Log(const std::string& msg, Sev sev = Sev::Info);

} // namespace core

/// Translations exactly like L.cs: T looks the English text up (through the core's embedded ru.json),
/// F formats a translated text. Results are cached per language; the cache is cleared by L::Reload.
namespace L {

const std::string& T(const char* en);
inline const std::string& T(const std::string& en) { return T(en.c_str()); }
/// string.Format(T(en), args): "{0}", "{1}" ... are replaced by the arguments, which the caller formats itself
/// (a format part such as "{0:0.0}" is dropped: pass the number already formatted the C# way).
std::string F(const char* en, std::initializer_list<std::string_view> args);
/// L.Lang.
const std::string& Lang();
/// Called by the shell when the core's language changed: clears the cache. True when it changed.
bool Reload();

} // namespace L
