// The C++ side of the core's page calls (core_bridge.h). Owner: ffi.
#include "core_bridge.h"

#include <cinttypes>
#include <cstdio>

namespace bridge {

namespace {

std::string TakeStr(char* s)
{
	if (s == nullptr) return {};
	std::string r(s);
	dos_free_str(s);
	return r;
}

json::Value TakeVal(char* s)
{
	std::string t = TakeStr(s);
	return t.empty() ? json::Value() : json::Parse(t);
}

} // namespace

void Args::Key(const char* key)
{
	if (!body_.empty()) body_ += ',';
	body_ += json::Quote(key ? key : "");
	body_ += ':';
}

Args& Args::Add(const char* key, std::string_view v)
{
	Key(key);
	body_ += json::Quote(v);
	return *this;
}

Args& Args::Add(const char* key, bool v)
{
	Key(key);
	body_ += v ? "true" : "false";
	return *this;
}

Args& Args::Add(const char* key, int64_t v)
{
	Key(key);
	body_ += std::to_string(v);
	return *this;
}

Args& Args::Add(const char* key, double v)
{
	Key(key);
	char b[64];
	std::snprintf(b, sizeof b, "%.17g", v);
	body_ += b;
	return *this;
}

Args& Args::Mask(const char* key, uint64_t v)
{
	Key(key);
	char b[32];
	std::snprintf(b, sizeof b, "\"0x%" PRIX64 "\"", v);
	body_ += b;
	return *this;
}

Args& Args::Null(const char* key)
{
	Key(key);
	body_ += "null";
	return *this;
}

json::Value Get(const char* name, const Args& a) { return TakeVal(dos_get(name, a.Json().c_str())); }

json::Value Do(const char* name, const Args& a) { return TakeVal(dos_do(name, a.Json().c_str())); }

uint64_t Start(const char* name, const Args& a) { return dos_start(name, a.Json().c_str()); }

std::string Fmt(const char* en, std::initializer_list<FmtArg> args)
{
	std::string j = "[";
	bool first = true;
	for (const FmtArg& x : args)
	{
		if (!first) j += ',';
		first = false;
		switch (x.kind)
		{
		case FmtArg::Str: j += json::Quote(x.s); break;
		case FmtArg::Int: j += std::to_string(x.i); break;
		case FmtArg::Bool: j += x.i ? "true" : "false"; break;
		case FmtArg::Dbl:
		{
			// A double that happens to be whole stays a double for .NET ("{0:0.0}" of 5.0 is "5.0", of 5 too).
			char b[64];
			std::snprintf(b, sizeof b, "%.17g", x.d);
			std::string s = b;
			if (s.find_first_of(".eEn") == std::string::npos) s += ".0";
			j += s;
			break;
		}
		}
	}
	j += "]";
	return TakeStr(dos_fmt(en, j.c_str()));
}

std::string Num(double v, const char* format) { return TakeStr(dos_num(v, format)); }

} // namespace bridge

#ifdef DOS_CORE_STUB
// NO_CORE=1 (the app alone, app/src/core_stub.cpp for the shell's calls): the page calls answer "unknown" and start
// nothing, so the pages build and show their empty states.
#include <cstdlib>
#include <cstring>

namespace {
char* StubDup(const char* s)
{
	size_t n = std::strlen(s) + 1;
	char* p = (char*)std::malloc(n);
	if (p) std::memcpy(p, s, n);
	return p;
}
} // namespace

extern "C" {
char* dos_get(const char*, const char*) { return StubDup("{\"error\":\"stub core\"}"); }
char* dos_do(const char*, const char*) { return StubDup("{\"error\":\"stub core\"}"); }
uint64_t dos_start(const char*, const char*) { return 0; }
char* dos_fmt(const char* en, const char*) { return StubDup(en ? en : ""); }
char* dos_num(double v, const char*)
{
	char b[64];
	std::snprintf(b, sizeof b, "%g", v);
	return StubDup(b);
}
int dos_quit_requested(void) { return 0; }
}
#endif
