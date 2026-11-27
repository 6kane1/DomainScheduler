//! A small backtracking regular-expression engine for the .NET patterns of `IrqAuto.cs` (the crate has no regex
//! dependency). It supports what those patterns use: literals and escapes (`\\ \. \( \) \{ \} \- \[ \]`), `.`,
//! classes `[...]` / `[^...]` with ranges, `\d \w \s \b`, `^ $`, groups `(...)` (capturing), alternation `|`,
//! the quantifiers `? * + {n} {n,} {n,m}` and their lazy forms. Options: IgnoreCase and Singleline.
//! .NET semantics kept: leftmost match, alternatives in order, greedy quantifiers back off, `$` also matches before a
//! final newline, `.` does not match `\n` without Singleline. `\d` is ASCII digits only and `\w` letters, digits and
//! `_` (the .NET classes are Unicode-wide; the device names, registry ids and event XML these patterns see are not).

#[derive(Clone, Debug)]
enum Item {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any,
    Class(bool, Vec<Item>),
    Start,
    End,
    WordB(bool),
    Digit(bool),
    Word(bool),
    Space(bool),
    Group(Box<Node>, Option<usize>),
    Alt(Vec<Node>),
    Concat(Vec<Node>),
    Repeat(Box<Node>, usize, Option<usize>, bool),
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Rx {
    root: Node,
    groups: usize,
    ic: bool,
    singleline: bool,
}

/// One match: the text of the whole match and of every capturing group (index 0 = the whole match; None = the
/// group did not take part).
#[derive(Clone, Debug)]
pub struct Match {
    pub groups: Vec<Option<String>>,
}

impl Match {
    fn new(caps: &[Option<(usize, usize)>], t: &[char]) -> Match {
        Match { groups: caps.iter().map(|c| c.map(|(a, b)| t[a..b].iter().collect())).collect() }
    }
    pub fn value(&self) -> String {
        self.group(0).unwrap_or_default()
    }
    pub fn group(&self, i: usize) -> Option<String> {
        self.groups.get(i).cloned().flatten()
    }
}

struct Parser<'a> {
    p: &'a [char],
    i: usize,
    groups: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.p.get(self.i).copied()
    }

    fn alt(&mut self) -> Node {
        let mut alts = vec![self.concat()];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.concat());
        }
        if alts.len() == 1 { alts.pop().unwrap_or(Node::Concat(vec![])) } else { Node::Alt(alts) }
    }

    fn concat(&mut self) -> Node {
        let mut v = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom();
            let atom = self.quant(atom);
            v.push(atom);
        }
        Node::Concat(v)
    }

    fn number(&mut self) -> Option<usize> {
        let s = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == s { None } else { self.p[s..self.i].iter().collect::<String>().parse().ok() }
    }

    fn quant(&mut self, atom: Node) -> Node {
        let (min, max) = match self.peek() {
            Some('?') => {
                self.i += 1;
                (0, Some(1))
            }
            Some('*') => {
                self.i += 1;
                (0, None)
            }
            Some('+') => {
                self.i += 1;
                (1, None)
            }
            Some('{') => {
                let save = self.i;
                self.i += 1;
                match self.number() {
                    Some(n) => {
                        let r = if self.peek() == Some('}') {
                            self.i += 1;
                            Some((n, Some(n)))
                        } else if self.peek() == Some(',') {
                            self.i += 1;
                            let m = self.number();
                            if self.peek() == Some('}') {
                                self.i += 1;
                                Some((n, m))
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        match r {
                            Some(x) => x,
                            None => {
                                self.i = save;
                                return atom;
                            }
                        }
                    }
                    None => {
                        self.i = save;
                        return atom;
                    }
                }
            }
            _ => return atom,
        };
        let lazy = if self.peek() == Some('?') {
            self.i += 1;
            true
        } else {
            false
        };
        Node::Repeat(Box::new(atom), min, max, lazy)
    }

    fn escape_class(c: char) -> Option<Item> {
        Some(match c {
            'd' => Item::Digit(true),
            'D' => Item::Digit(false),
            'w' => Item::Word(true),
            'W' => Item::Word(false),
            's' => Item::Space(true),
            'S' => Item::Space(false),
            _ => return None,
        })
    }

    fn escaped_char(c: char) -> char {
        match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            _ => c,
        }
    }

    fn atom(&mut self) -> Node {
        let c = self.p[self.i];
        self.i += 1;
        match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                self.groups += 1;
                let g = self.groups;
                let inner = self.alt();
                if self.peek() == Some(')') {
                    self.i += 1;
                }
                Node::Group(Box::new(inner), Some(g))
            }
            '[' => self.class(),
            '\\' => {
                let e = self.p.get(self.i).copied().unwrap_or('\\');
                self.i += 1;
                match e {
                    'b' => Node::WordB(true),
                    'B' => Node::WordB(false),
                    'd' => Node::Digit(true),
                    'D' => Node::Digit(false),
                    'w' => Node::Word(true),
                    'W' => Node::Word(false),
                    's' => Node::Space(true),
                    'S' => Node::Space(false),
                    _ => Node::Char(Self::escaped_char(e)),
                }
            }
            _ => Node::Char(c),
        }
    }

    fn class(&mut self) -> Node {
        let mut neg = false;
        if self.peek() == Some('^') {
            neg = true;
            self.i += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        while let Some(c) = self.peek() {
            if c == ']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            self.i += 1;
            let lo = if c == '\\' {
                let e = self.p.get(self.i).copied().unwrap_or('\\');
                self.i += 1;
                if let Some(it) = Self::escape_class(e) {
                    items.push(it);
                    continue;
                }
                Self::escaped_char(e)
            } else {
                c
            };
            // a range a-b (a '-' before ']' is a literal)
            if self.peek() == Some('-') && self.p.get(self.i + 1).is_some_and(|n| *n != ']') {
                self.i += 1;
                let mut hi = self.p[self.i];
                self.i += 1;
                if hi == '\\' {
                    hi = Self::escaped_char(self.p.get(self.i).copied().unwrap_or('\\'));
                    self.i += 1;
                }
                items.push(Item::Range(lo, hi));
            } else {
                items.push(Item::Range(lo, lo));
            }
        }
        Node::Class(neg, items)
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn unfold(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

type Caps = Vec<Option<(usize, usize)>>;

impl Rx {
    /// A pattern; `ic` = RegexOptions.IgnoreCase.
    pub fn new(pattern: &str, ic: bool) -> Rx {
        Self::with(pattern, ic, false)
    }

    /// `singleline` = RegexOptions.Singleline (`.` also matches `\n`).
    pub fn with(pattern: &str, ic: bool, singleline: bool) -> Rx {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser { p: &chars, i: 0, groups: 0 };
        let mut root = p.alt();
        // a stray ')' at the top level: the rest is ignored (no pattern of the program has one)
        while p.i < chars.len() {
            p.i += 1;
            let more = p.alt();
            root = Node::Concat(vec![root, more]);
        }
        Rx { root, groups: p.groups, ic, singleline }
    }

    /// `Regex.IsMatch(s, pattern, options)`.
    pub fn is_match(&self, s: &str) -> bool {
        let t: Vec<char> = s.chars().collect();
        self.find_at(&t, 0).is_some()
    }

    /// `Regex.Match`: the first match, or None.
    pub fn find(&self, s: &str) -> Option<Match> {
        let t: Vec<char> = s.chars().collect();
        let g = self.find_at(&t, 0)?;
        Some(Match::new(&g, &t))
    }

    /// `Regex.Matches`: every non-overlapping match, left to right.
    pub fn find_all(&self, s: &str) -> Vec<Match> {
        let t: Vec<char> = s.chars().collect();
        let mut out = Vec::new();
        let mut at = 0;
        while at <= t.len() {
            match self.find_at(&t, at) {
                Some(g) => {
                    let (a, b) = g[0].unwrap_or((at, at));
                    at = if b == a { b + 1 } else { b };
                    out.push(Match::new(&g, &t));
                }
                None => break,
            }
        }
        out
    }

    fn find_at(&self, t: &[char], from: usize) -> Option<Caps> {
        for start in from..=t.len() {
            let mut caps: Caps = vec![None; self.groups + 1];
            let mut end = None;
            if self.m(&self.root, t, start, &mut caps, &mut |p, _| {
                end = Some(p);
                true
            }) {
                // the captures of the successful path are in caps (kept on success)
                caps[0] = Some((start, end.unwrap_or(start)));
                return Some(caps);
            }
        }
        None
    }

    fn eq(&self, a: char, b: char) -> bool {
        a == b || (self.ic && fold(a) == fold(b))
    }

    fn item(&self, it: &Item, c: char) -> bool {
        match it {
            Item::Range(lo, hi) => {
                let inr = |x: char| *lo <= x && x <= *hi;
                inr(c) || (self.ic && (inr(fold(c)) || inr(unfold(c))))
            }
            Item::Digit(y) => c.is_ascii_digit() == *y,
            Item::Word(y) => is_word(c) == *y,
            Item::Space(y) => c.is_whitespace() == *y,
        }
    }

    fn one(&self, n: &Node, c: char) -> Option<bool> {
        Some(match n {
            Node::Char(x) => self.eq(*x, c),
            Node::Any => self.singleline || c != '\n',
            Node::Class(neg, items) => items.iter().any(|i| self.item(i, c)) != *neg,
            Node::Digit(y) => c.is_ascii_digit() == *y,
            Node::Word(y) => is_word(c) == *y,
            Node::Space(y) => c.is_whitespace() == *y,
            _ => return None,
        })
    }

    fn m(&self, n: &Node, t: &[char], pos: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        match n {
            Node::Start => pos == 0 && k(pos, caps),
            Node::End => (pos == t.len() || (pos + 1 == t.len() && t[pos] == '\n')) && k(pos, caps),
            Node::WordB(y) => {
                let a = pos > 0 && is_word(t[pos - 1]);
                let b = pos < t.len() && is_word(t[pos]);
                ((a != b) == *y) && k(pos, caps)
            }
            Node::Group(inner, g) => {
                let g = *g;
                self.m(inner, t, pos, caps, &mut |p, c| {
                    if let Some(i) = g {
                        let old = c[i];
                        c[i] = Some((pos, p));
                        if k(p, c) {
                            return true;
                        }
                        c[i] = old;
                        false
                    } else {
                        k(p, c)
                    }
                })
            }
            Node::Alt(alts) => {
                for a in alts {
                    if self.m(a, t, pos, caps, k) {
                        return true;
                    }
                }
                false
            }
            Node::Concat(v) => self.seq(v, 0, t, pos, caps, k),
            Node::Repeat(inner, min, max, lazy) => {
                // a single-char atom: iterative, no deep recursion
                if let Some(_) = self.one(inner, ' ') {
                    let mut n = 0;
                    let limit = max.unwrap_or(usize::MAX);
                    while n < limit && pos + n < t.len() && self.one(inner, t[pos + n]) == Some(true) {
                        n += 1;
                    }
                    if n < *min {
                        return false;
                    }
                    if *lazy {
                        for c in *min..=n {
                            if k(pos + c, caps) {
                                return true;
                            }
                        }
                    } else {
                        for c in (*min..=n).rev() {
                            if k(pos + c, caps) {
                                return true;
                            }
                        }
                    }
                    return false;
                }
                self.rep(inner, *min, *max, *lazy, 0, t, pos, caps, k)
            }
            _ => match self.one(n, ' ') {
                Some(_) => pos < t.len() && self.one(n, t[pos]) == Some(true) && k(pos + 1, caps),
                None => false,
            },
        }
    }

    fn seq(&self, v: &[Node], i: usize, t: &[char], pos: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        if i == v.len() {
            return k(pos, caps);
        }
        self.m(&v[i], t, pos, caps, &mut |p, c| self.seq(v, i + 1, t, p, c, k))
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &self,
        n: &Node,
        min: usize,
        max: Option<usize>,
        lazy: bool,
        count: usize,
        t: &[char],
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        let more = max.is_none_or(|m| count < m);
        if lazy {
            if count >= min && k(pos, caps) {
                return true;
            }
            return more
                && self.m(n, t, pos, caps, &mut |p, c| p != pos && self.rep(n, min, max, lazy, count + 1, t, p, c, k));
        }
        if more && self.m(n, t, pos, caps, &mut |p, c| (p != pos || count < min) && self.rep(n, min, max, lazy, count + 1, t, p, c, k)) {
            return true;
        }
        count >= min && k(pos, caps)
    }
}

#[cfg(test)]
mod tests {
    use super::Rx;

    #[test]
    fn basics() {
        assert!(Rx::new(r"^ACPI\\PNP0100|^PCI\\VEN_1022&DEV_14DB", true).is_match(r"PCI\VEN_1022&DEV_14DB&SUBSYS_0"));
        assert!(!Rx::new(r"^ACPI\\PNP0100", true).is_match(r"X\ACPI\PNP0100"));
        assert!(Rx::new(r"\bSATA\b", true).is_match("Standard SATA AHCI"));
        assert!(!Rx::new(r"\bSATA\b", true).is_match("XSATA"));
        assert!(Rx::new(r"^(stornvme|nvme\w*)$", true).is_match("NVMEx"));
        assert!(!Rx::new(r"^(stornvme|nvme\w*)$", true).is_match("xnvme"));
        assert!(Rx::new(r"^\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}\}\\\d{4}$", false).is_match(r"{4d36e972-e325}\0001"));
        assert!(!Rx::new(r"^\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}\}\\\d{4}$", false).is_match(r"{4d36e972-e325}\00011"));
        assert!(Rx::new(r"Intel\s*\(R\)\s*(UHD|HD|Iris( Xe)?)( Graphics)?\b", true).is_match("Intel(R) Iris Xe Graphics"));
        assert!(Rx::new("монитор", true).is_match("МОНИТОР"));
        assert!(Rx::new(r"^[A-Za-z0-9 _\-]{1,64}$", false).is_match("Device-Priority 1"));
        assert!(!Rx::new(r"^[A-Za-z0-9 _\-]{1,64}$", false).is_match("a|b"));
        let m = Rx::new("<Data Name=['\"]Number['\"]>(\\d+)</Data>", false).find("<Data Name='Number'>13</Data>").unwrap();
        assert_eq!(m.group(1).as_deref(), Some("13"));
        let all = Rx::with("<E[ >].*?</E>", false, true).find_all("<E>a\n</E><E x>b</E>");
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].value(), "<E x>b</E>");
        assert!(Rx::new(r"\b(COM\d+|Serial Port)", true).is_match("USB Serial Device (COM3)"));
        assert!(Rx::new("vb-?audio", true).is_match("VBAudio"));
    }
}
