//! Case-insensitive strings like C#'s `StringComparer.OrdinalIgnoreCase` and `ToUpperInvariant` / `ToLowerInvariant`.
//!
//! .NET compares "ignore case" by the simple (one char to one char) upper-case mapping. Rust's `to_uppercase` is the
//! full mapping (`ß` -> `SS`), so every helper here maps char by char and keeps a char whose mapping is not one char.

use std::fmt;
use std::hash::{Hash, Hasher};

/// The simple upper-case mapping of one char (the char itself when the mapping is not exactly one char).
pub fn upper_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// The simple lower-case mapping of one char.
pub fn lower_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut u = c.to_lowercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// `s.ToUpperInvariant()`.
pub fn upper(s: &str) -> String {
    s.chars().map(upper_char).collect()
}

/// `s.ToLowerInvariant()`.
pub fn lower(s: &str) -> String {
    s.chars().map(lower_char).collect()
}

/// `string.Equals(a, b, StringComparison.OrdinalIgnoreCase)`.
pub fn eq(a: &str, b: &str) -> bool {
    a.chars().map(upper_char).eq(b.chars().map(upper_char))
}

/// `s.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)`.
pub fn starts_with(s: &str, prefix: &str) -> bool {
    let mut a = s.chars().map(upper_char);
    for p in prefix.chars().map(upper_char) {
        if a.next() != Some(p) {
            return false;
        }
    }
    true
}

/// `s.EndsWith(suffix, StringComparison.OrdinalIgnoreCase)`.
pub fn ends_with(s: &str, suffix: &str) -> bool {
    let a: Vec<char> = s.chars().map(upper_char).collect();
    let b: Vec<char> = suffix.chars().map(upper_char).collect();
    a.len() >= b.len() && a[a.len() - b.len()..] == b[..]
}

/// `s.Contains(part, StringComparison.OrdinalIgnoreCase)` (also `IndexOf(...) >= 0`).
pub fn contains(s: &str, part: &str) -> bool {
    find(s, part).is_some()
}

/// `s.IndexOf(part, StringComparison.OrdinalIgnoreCase)` as a byte offset into `s`.
pub fn find(s: &str, part: &str) -> Option<usize> {
    let p: Vec<char> = part.chars().map(upper_char).collect();
    if p.is_empty() {
        return Some(0);
    }
    let a: Vec<(usize, char)> = s.char_indices().map(|(i, c)| (i, upper_char(c))).collect();
    if a.len() < p.len() {
        return None;
    }
    'outer: for start in 0..=a.len() - p.len() {
        for (k, pc) in p.iter().enumerate() {
            if a[start + k].1 != *pc {
                continue 'outer;
            }
        }
        return Some(a[start].0);
    }
    None
}

/// A string key that hashes and compares case-insensitively but keeps the casing it was made with (the way a C#
/// dictionary with `StringComparer.OrdinalIgnoreCase` keeps the first key's casing).
#[derive(Clone, Default)]
pub struct CiString(pub String);

impl CiString {
    pub fn new(s: impl Into<String>) -> Self {
        CiString(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for CiString {
    fn eq(&self, o: &Self) -> bool {
        eq(&self.0, &o.0)
    }
}
impl Eq for CiString {}

impl Hash for CiString {
    fn hash<H: Hasher>(&self, h: &mut H) {
        for c in self.0.chars() {
            h.write_u32(upper_char(c) as u32);
        }
        h.write_u8(0xff);
    }
}

impl fmt::Debug for CiString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}
impl fmt::Display for CiString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl From<&str> for CiString {
    fn from(s: &str) -> Self {
        CiString(s.to_string())
    }
}
impl From<String> for CiString {
    fn from(s: String) -> Self {
        CiString(s)
    }
}
impl From<&String> for CiString {
    fn from(s: &String) -> Self {
        CiString(s.clone())
    }
}
/// A borrowed lookup key for [`CiMap`] / [`CiSet`] (no allocation): `map.get(&CiRef("cs2.exe"))`.
#[derive(Clone, Copy)]
pub struct CiRef<'a>(pub &'a str);

impl Hash for CiRef<'_> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        for c in self.0.chars() {
            h.write_u32(upper_char(c) as u32);
        }
        h.write_u8(0xff);
    }
}

impl indexmap::Equivalent<CiString> for CiRef<'_> {
    fn equivalent(&self, key: &CiString) -> bool {
        eq(self.0, &key.0)
    }
}

/// `Dictionary<string, V>(StringComparer.OrdinalIgnoreCase)`, insertion-ordered.
pub type CiMap<V> = indexmap::IndexMap<CiString, V>;
/// `HashSet<string>(StringComparer.OrdinalIgnoreCase)`, insertion-ordered.
pub type CiSet = indexmap::IndexSet<CiString>;

/// Lookup helpers so callers can write `map.ci_get("x")`.
pub trait CiMapExt<V> {
    fn ci_get(&self, key: &str) -> Option<&V>;
    fn ci_get_mut(&mut self, key: &str) -> Option<&mut V>;
    fn ci_contains(&self, key: &str) -> bool;
    fn ci_remove(&mut self, key: &str) -> Option<V>;
    /// `d[key] = value` (keeps the first key's casing, like C#).
    fn ci_insert(&mut self, key: &str, value: V);
}

impl<V> CiMapExt<V> for CiMap<V> {
    fn ci_get(&self, key: &str) -> Option<&V> {
        self.get(&CiRef(key))
    }
    fn ci_get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.get_mut(&CiRef(key))
    }
    fn ci_contains(&self, key: &str) -> bool {
        self.contains_key(&CiRef(key))
    }
    fn ci_remove(&mut self, key: &str) -> Option<V> {
        self.shift_remove(&CiRef(key))
    }
    fn ci_insert(&mut self, key: &str, value: V) {
        if let Some(v) = self.get_mut(&CiRef(key)) {
            *v = value;
        } else {
            self.insert(CiString::from(key), value);
        }
    }
}

/// Set helpers: `set.ci_contains("x")`, `set.ci_add("x")` (C# `HashSet.Add`: true when it was new).
pub trait CiSetExt {
    fn ci_contains(&self, key: &str) -> bool;
    fn ci_add(&mut self, key: &str) -> bool;
    fn ci_remove(&mut self, key: &str) -> bool;
}

impl CiSetExt for CiSet {
    fn ci_contains(&self, key: &str) -> bool {
        self.contains(&CiRef(key))
    }
    fn ci_add(&mut self, key: &str) -> bool {
        if self.contains(&CiRef(key)) {
            return false;
        }
        self.insert(CiString::from(key))
    }
    fn ci_remove(&mut self, key: &str) -> bool {
        self.shift_remove(&CiRef(key))
    }
}

/// A CiSet from a list of names.
pub fn set_of<'a>(items: impl IntoIterator<Item = &'a str>) -> CiSet {
    let mut s = CiSet::new();
    for i in items {
        s.ci_add(i);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_like_ordinal_ignore_case() {
        assert!(eq("CS2.exe", "cs2.EXE"));
        assert!(!eq("cs2.exe", "cs2.ex"));
        assert!(eq("Русский", "РУССКИЙ"));
        assert!(!eq("ß", "SS"));
        assert!(starts_with("COR_ENABLE", "cor_"));
        assert!(ends_with("game.EXE", ".exe"));
        assert_eq!(find("Hello World", "WORLD"), Some(6));
        let mut m: CiMap<i32> = CiMap::new();
        m.ci_insert("Cs2.exe", 1);
        m.ci_insert("CS2.EXE", 2);
        assert_eq!(m.len(), 1);
        assert_eq!(m.ci_get("cs2.exe"), Some(&2));
        assert_eq!(m.keys().next().unwrap().as_str(), "Cs2.exe");
    }
}
