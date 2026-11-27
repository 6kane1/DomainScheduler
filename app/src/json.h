// A tiny JSON reader for the strings the core returns (pages' data, the shell state). Header only, no exceptions:
// a malformed document gives a null value. Numbers are doubles (integers up to 2^53 are exact; masks travel as
// strings). Owner: app shell; used by every page.
//
//   json::Value v = json::Parse(text);
//   v["state"].Int();  v["status"].Str();  for (const auto& e : v["items"].Arr()) ...
#pragma once
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <map>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace json {

class Value {
public:
	enum class Kind { Null, Bool, Num, Str, Arr, Obj };

	Value() = default;
	static Value MakeBool(bool b) { Value v; v.k_ = Kind::Bool; v.b_ = b; return v; }
	static Value MakeNum(double d) { Value v; v.k_ = Kind::Num; v.n_ = d; return v; }
	static Value MakeStr(std::string s) { Value v; v.k_ = Kind::Str; v.s_ = std::move(s); return v; }
	static Value MakeArr() { Value v; v.k_ = Kind::Arr; v.a_ = std::make_shared<std::vector<Value>>(); return v; }
	static Value MakeObj() { Value v; v.k_ = Kind::Obj; v.o_ = std::make_shared<std::map<std::string, Value>>(); return v; }

	Kind kind() const { return k_; }
	bool IsNull() const { return k_ == Kind::Null; }
	bool IsObj() const { return k_ == Kind::Obj; }
	bool IsArr() const { return k_ == Kind::Arr; }
	bool IsStr() const { return k_ == Kind::Str; }
	bool IsNum() const { return k_ == Kind::Num; }

	bool Bool(bool def = false) const { return k_ == Kind::Bool ? b_ : (k_ == Kind::Num ? n_ != 0 : def); }
	double Num(double def = 0) const { return k_ == Kind::Num ? n_ : (k_ == Kind::Bool ? (b_ ? 1 : 0) : (k_ == Kind::Str ? Parse(s_, def) : def)); }
	int Int(int def = 0) const { return k_ == Kind::Num || k_ == Kind::Bool || k_ == Kind::Str ? (int)Num(def) : def; }
	int64_t I64(int64_t def = 0) const { return k_ == Kind::Num ? (int64_t)n_ : (k_ == Kind::Str ? (int64_t)std::strtoll(s_.c_str(), nullptr, 0) : def); }
	/// A 64-bit mask: a string ("0x1F" or decimal) or a number.
	uint64_t U64(uint64_t def = 0) const { return k_ == Kind::Str ? (uint64_t)std::strtoull(s_.c_str(), nullptr, 0) : (k_ == Kind::Num ? (uint64_t)n_ : def); }
	const std::string& Str() const { static const std::string empty; return k_ == Kind::Str ? s_ : empty; }
	std::string StrOr(const char* def) const { return k_ == Kind::Str ? s_ : std::string(def); }

	const std::vector<Value>& Arr() const { static const std::vector<Value> empty; return k_ == Kind::Arr ? *a_ : empty; }
	const std::map<std::string, Value>& Obj() const { static const std::map<std::string, Value> empty; return k_ == Kind::Obj ? *o_ : empty; }
	size_t size() const { return k_ == Kind::Arr ? a_->size() : (k_ == Kind::Obj ? o_->size() : 0); }
	bool Has(const std::string& key) const { return k_ == Kind::Obj && o_->count(key) > 0; }

	const Value& operator[](const std::string& key) const
	{
		static const Value none;
		if (k_ != Kind::Obj) return none;
		auto it = o_->find(key);
		return it == o_->end() ? none : it->second;
	}
	const Value& operator[](size_t i) const
	{
		static const Value none;
		return k_ == Kind::Arr && i < a_->size() ? (*a_)[i] : none;
	}

	std::vector<Value>& MutArr() { return *a_; }
	std::map<std::string, Value>& MutObj() { return *o_; }

private:
	static double Parse(const std::string& s, double def)
	{
		if (s.empty()) return def;
		char* end = nullptr;
		double d = std::strtod(s.c_str(), &end);
		return end == s.c_str() ? def : d;
	}
	Kind k_ = Kind::Null;
	bool b_ = false;
	double n_ = 0;
	std::string s_;
	std::shared_ptr<std::vector<Value>> a_;
	std::shared_ptr<std::map<std::string, Value>> o_;
};

namespace detail {
struct Reader {
	std::string_view t;
	size_t i = 0;
	bool bad = false;

	void Ws() { while (i < t.size() && (t[i] == ' ' || t[i] == '\t' || t[i] == '\n' || t[i] == '\r')) i++; }
	bool Eat(char c) { Ws(); if (i < t.size() && t[i] == c) { i++; return true; } return false; }

	static void Utf8(std::string& o, uint32_t cp)
	{
		if (cp < 0x80) o += (char)cp;
		else if (cp < 0x800) { o += (char)(0xC0 | (cp >> 6)); o += (char)(0x80 | (cp & 0x3F)); }
		else if (cp < 0x10000) { o += (char)(0xE0 | (cp >> 12)); o += (char)(0x80 | ((cp >> 6) & 0x3F)); o += (char)(0x80 | (cp & 0x3F)); }
		else { o += (char)(0xF0 | (cp >> 18)); o += (char)(0x80 | ((cp >> 12) & 0x3F)); o += (char)(0x80 | ((cp >> 6) & 0x3F)); o += (char)(0x80 | (cp & 0x3F)); }
	}
	uint32_t Hex4()
	{
		if (i + 4 > t.size()) { bad = true; return 0; }
		uint32_t v = 0;
		for (int k = 0; k < 4; k++)
		{
			char c = t[i++];
			v <<= 4;
			if (c >= '0' && c <= '9') v |= (uint32_t)(c - '0');
			else if (c >= 'a' && c <= 'f') v |= (uint32_t)(c - 'a' + 10);
			else if (c >= 'A' && c <= 'F') v |= (uint32_t)(c - 'A' + 10);
			else { bad = true; return 0; }
		}
		return v;
	}
	std::string String()
	{
		std::string o;
		if (!Eat('"')) { bad = true; return o; }
		while (i < t.size())
		{
			char c = t[i++];
			if (c == '"') return o;
			if (c != '\\') { o += c; continue; }
			if (i >= t.size()) break;
			char e = t[i++];
			switch (e)
			{
			case '"': o += '"'; break;
			case '\\': o += '\\'; break;
			case '/': o += '/'; break;
			case 'b': o += '\b'; break;
			case 'f': o += '\f'; break;
			case 'n': o += '\n'; break;
			case 'r': o += '\r'; break;
			case 't': o += '\t'; break;
			case 'u':
			{
				uint32_t cp = Hex4();
				if (cp >= 0xD800 && cp <= 0xDBFF && i + 6 <= t.size() && t[i] == '\\' && t[i + 1] == 'u')
				{
					i += 2;
					uint32_t lo = Hex4();
					cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
				}
				Utf8(o, cp);
				break;
			}
			default: bad = true; return o;
			}
		}
		bad = true;
		return o;
	}
	Value Any(int depth)
	{
		if (depth > 64) { bad = true; return {}; }
		Ws();
		if (i >= t.size()) { bad = true; return {}; }
		char c = t[i];
		if (c == '{')
		{
			i++;
			Value v = Value::MakeObj();
			if (Eat('}')) return v;
			do
			{
				Ws();
				std::string k = String();
				if (bad || !Eat(':')) { bad = true; return {}; }
				v.MutObj()[k] = Any(depth + 1);
				if (bad) return {};
			} while (Eat(','));
			if (!Eat('}')) bad = true;
			return v;
		}
		if (c == '[')
		{
			i++;
			Value v = Value::MakeArr();
			if (Eat(']')) return v;
			do
			{
				v.MutArr().push_back(Any(depth + 1));
				if (bad) return {};
			} while (Eat(','));
			if (!Eat(']')) bad = true;
			return v;
		}
		if (c == '"') return Value::MakeStr(String());
		if (t.substr(i, 4) == "true") { i += 4; return Value::MakeBool(true); }
		if (t.substr(i, 5) == "false") { i += 5; return Value::MakeBool(false); }
		if (t.substr(i, 4) == "null") { i += 4; return {}; }
		size_t s = i;
		while (i < t.size() && (t[i] == '-' || t[i] == '+' || t[i] == '.' || t[i] == 'e' || t[i] == 'E' || (t[i] >= '0' && t[i] <= '9'))) i++;
		if (s == i) { bad = true; return {}; }
		std::string num(t.substr(s, i - s));
		return Value::MakeNum(std::strtod(num.c_str(), nullptr));
	}
};
} // namespace detail

inline Value Parse(std::string_view text)
{
	detail::Reader r{text};
	Value v = r.Any(0);
	return r.bad ? Value() : v;
}

/// Escapes a string for a JSON document the app sends to the core (commands with arguments).
inline std::string Quote(std::string_view s)
{
	std::string o = "\"";
	for (char c : s)
	{
		switch (c)
		{
		case '"': o += "\\\""; break;
		case '\\': o += "\\\\"; break;
		case '\n': o += "\\n"; break;
		case '\r': o += "\\r"; break;
		case '\t': o += "\\t"; break;
		default:
			if ((unsigned char)c < 0x20) { char b[8]; snprintf(b, sizeof b, "\\u%04x", c); o += b; }
			else o += c;
		}
	}
	return o + "\"";
}

} // namespace json
