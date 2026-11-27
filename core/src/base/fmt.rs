//! .NET composite formatting (`string.Format`) and number formatting, so `L.F` and the `ToString("0.0")` calls of the
//! C# code give the same text.
//!
//! - `{index[,alignment][:format]}`, `{{` and `}}`; a bad item or an index past the arguments is an error
//!   (`FormatException`).
//! - Numbers: standard formats `D`, `X`/`x`, `N`, `F`, `G`, `P`, `E` with an optional precision, and custom formats
//!   (`0`, `#`, `.`, `,`, `%`, literals, quotes, `\`, up to three sections `pos;neg;zero`).
//!   A double in a custom format is first rounded to 15 significant digits (.NET's `DoublePrecisionCustomFormat`),
//!   then rounded half away from zero, like .NET Core 3.0+; standard formats use the exact value.
//! - `bool` prints `True` / `False`; a string ignores the format; a `DateTime` takes date/time formats.
//! - The culture: [`Culture::current`] is the Windows user locale (decimal comma on a Russian Windows), read once;
//!   [`Culture::invariant`] is `CultureInfo.InvariantCulture`.

use super::time::DateTime;
use std::sync::OnceLock;

/// The parts of `CultureInfo` the program's texts use.
#[derive(Clone, Debug)]
pub struct Culture {
    pub name: String,
    pub decimal: String,
    pub group: String,
    pub negative: String,
    pub percent: String,
    pub short_time: String,
    pub long_time: String,
    pub short_date: String,
    pub am: String,
    pub pm: String,
    pub time_sep: String,
    pub date_sep: String,
    pub months: Vec<String>,
    pub months_abbr: Vec<String>,
    pub days: Vec<String>,
    pub days_abbr: Vec<String>,
}

static INVARIANT: OnceLock<Culture> = OnceLock::new();
static CURRENT: OnceLock<Culture> = OnceLock::new();

impl Culture {
    /// `CultureInfo.InvariantCulture`.
    pub fn invariant() -> Culture {
        INVARIANT
            .get_or_init(|| {
                let v = |s: &str| s.split(',').map(|x| x.to_string()).collect::<Vec<_>>();
                Culture {
                    name: String::new(),
                    decimal: ".".into(),
                    group: ",".into(),
                    negative: "-".into(),
                    percent: "%".into(),
                    short_time: "HH:mm".into(),
                    long_time: "HH:mm:ss".into(),
                    short_date: "MM/dd/yyyy".into(),
                    am: "AM".into(),
                    pm: "PM".into(),
                    time_sep: ":".into(),
                    date_sep: "/".into(),
                    months: v("January,February,March,April,May,June,July,August,September,October,November,December"),
                    months_abbr: v("Jan,Feb,Mar,Apr,May,Jun,Jul,Aug,Sep,Oct,Nov,Dec"),
                    days: v("Sunday,Monday,Tuesday,Wednesday,Thursday,Friday,Saturday"),
                    days_abbr: v("Sun,Mon,Tue,Wed,Thu,Fri,Sat"),
                }
            })
            .clone()
    }

    /// `CultureInfo.CurrentCulture` (the Windows user locale; invariant elsewhere). Read once per run.
    pub fn current() -> Culture {
        CURRENT.get_or_init(|| crate::native::sys::current_culture().unwrap_or_else(Culture::invariant)).clone()
    }

    pub fn month_name(&self, m: u32) -> String {
        self.months.get(m as usize - 1).cloned().unwrap_or_default()
    }
    pub fn month_abbr(&self, m: u32) -> String {
        self.months_abbr.get(m as usize - 1).cloned().unwrap_or_default()
    }
    pub fn day_name(&self, d: usize) -> String {
        self.days.get(d).cloned().unwrap_or_default()
    }
    pub fn day_abbr(&self, d: usize) -> String {
        self.days_abbr.get(d).cloned().unwrap_or_default()
    }
}

/// One argument of a composite format (a C# `object`).
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    S(String),
    I(i64),
    U(u64),
    F(f64),
    B(bool),
    T(DateTime),
}

macro_rules! arg_from {
    ($v:ident, $($t:ty),*) => { $(impl From<$t> for Arg { fn from(x: $t) -> Self { Arg::$v(x as _) } })* };
}
arg_from!(I, i8, i16, i32, i64, isize);
arg_from!(U, u8, u16, u32, u64, usize);
arg_from!(F, f64);
impl From<f32> for Arg {
    fn from(x: f32) -> Self {
        // A C# float formats with 7 digits; going through its shortest text keeps 0.1f as 0.1.
        Arg::F(x.to_string().parse().unwrap_or(x as f64))
    }
}
impl From<bool> for Arg {
    fn from(x: bool) -> Self {
        Arg::B(x)
    }
}
impl From<&str> for Arg {
    fn from(x: &str) -> Self {
        Arg::S(x.to_string())
    }
}
impl From<String> for Arg {
    fn from(x: String) -> Self {
        Arg::S(x)
    }
}
impl From<&String> for Arg {
    fn from(x: &String) -> Self {
        Arg::S(x.clone())
    }
}
impl From<char> for Arg {
    fn from(x: char) -> Self {
        Arg::S(x.to_string())
    }
}
impl From<DateTime> for Arg {
    fn from(x: DateTime) -> Self {
        Arg::T(x)
    }
}
impl From<&Arg> for Arg {
    fn from(x: &Arg) -> Self {
        x.clone()
    }
}
impl<T: Into<Arg> + Clone> From<&T> for Arg
where
    T: NotStr,
{
    fn from(x: &T) -> Self {
        x.clone().into()
    }
}
/// Marker for the `From<&T>` blanket (numbers, bool, DateTime).
pub trait NotStr {}
macro_rules! not_str { ($($t:ty),*) => { $(impl NotStr for $t {})* }; }
not_str!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, bool, char, DateTime);

impl Arg {
    /// `arg.ToString(format, culture)`.
    pub fn format(&self, f: &str, c: &Culture) -> String {
        match self {
            Arg::S(s) => s.clone(),
            Arg::B(b) => if *b { "True" } else { "False" }.to_string(),
            Arg::I(v) => fmt_int(*v as i128, f, c),
            Arg::U(v) => fmt_int(*v as i128, f, c),
            Arg::F(v) => fmt_double(*v, f, c),
            Arg::T(t) => t.format(f, c),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError;

/// `string.Format(culture, fmt, args)`.
pub fn format(c: &Culture, fmt: &str, args: &[Arg]) -> Result<String, FormatError> {
    let ch: Vec<char> = fmt.chars().collect();
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut i = 0;
    while i < ch.len() {
        match ch[i] {
            '{' if i + 1 < ch.len() && ch[i + 1] == '{' => {
                out.push('{');
                i += 2;
            }
            '}' if i + 1 < ch.len() && ch[i + 1] == '}' => {
                out.push('}');
                i += 2;
            }
            '}' => return Err(FormatError),
            '{' => {
                i += 1;
                let mut idx: usize = 0;
                let start = i;
                while i < ch.len() && ch[i].is_ascii_digit() {
                    idx = idx.checked_mul(10).and_then(|v| v.checked_add(ch[i] as usize - '0' as usize)).ok_or(FormatError)?;
                    i += 1;
                }
                if i == start {
                    return Err(FormatError);
                }
                while i < ch.len() && ch[i] == ' ' {
                    i += 1;
                }
                let mut width: i64 = 0;
                if i < ch.len() && ch[i] == ',' {
                    i += 1;
                    while i < ch.len() && ch[i] == ' ' {
                        i += 1;
                    }
                    let neg = i < ch.len() && ch[i] == '-';
                    if neg {
                        i += 1;
                    }
                    let ws = i;
                    while i < ch.len() && ch[i].is_ascii_digit() {
                        width = width * 10 + (ch[i] as i64 - '0' as i64);
                        if width > 1_000_000 {
                            return Err(FormatError);
                        }
                        i += 1;
                    }
                    if i == ws {
                        return Err(FormatError);
                    }
                    if neg {
                        width = -width;
                    }
                    while i < ch.len() && ch[i] == ' ' {
                        i += 1;
                    }
                }
                let mut spec = String::new();
                if i < ch.len() && ch[i] == ':' {
                    i += 1;
                    loop {
                        if i >= ch.len() {
                            return Err(FormatError);
                        }
                        if ch[i] == '}' {
                            if i + 1 < ch.len() && ch[i + 1] == '}' {
                                spec.push('}');
                                i += 2;
                                continue;
                            }
                            break;
                        }
                        if ch[i] == '{' {
                            if i + 1 < ch.len() && ch[i + 1] == '{' {
                                spec.push('{');
                                i += 2;
                                continue;
                            }
                            return Err(FormatError);
                        }
                        spec.push(ch[i]);
                        i += 1;
                    }
                }
                if i >= ch.len() || ch[i] != '}' {
                    return Err(FormatError);
                }
                i += 1;
                let a = args.get(idx).ok_or(FormatError)?;
                let s = a.format(&spec, c);
                let n = s.chars().count() as i64;
                if width > n {
                    out.extend(std::iter::repeat(' ').take((width - n) as usize));
                    out.push_str(&s);
                } else if -width > n {
                    out.push_str(&s);
                    out.extend(std::iter::repeat(' ').take((-width - n) as usize));
                } else {
                    out.push_str(&s);
                }
            }
            x => {
                out.push(x);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// `v.ToString(format, culture)` for a double.
pub fn num(v: f64, format: &str, c: &Culture) -> String {
    fmt_double(v, format, c)
}

/// `v.ToString(format, culture)` for an integer.
pub fn int(v: i64, format: &str, c: &Culture) -> String {
    fmt_int(v as i128, format, c)
}

/// `v.ToString(culture)` for a double (shortest round-trip text, .NET Core 3.0+).
pub fn double_default(v: f64, c: &Culture) -> String {
    fmt_double(v, "", c)
}

// ---- numbers ------------------------------------------------------------------------------------------------------

/// A decimal number: digits (no leading zeros except a lone 0) and the position of the decimal point
/// (value = 0.d1d2d3... * 10^point).
#[derive(Clone, Debug)]
struct Digits {
    neg: bool,
    digits: Vec<u8>,
    point: i32,
}

impl Digits {
    fn is_zero(&self) -> bool {
        self.digits.iter().all(|d| *d == 0)
    }

    fn from_int(v: i128) -> Digits {
        let s = v.unsigned_abs().to_string();
        let digits: Vec<u8> = s.bytes().map(|b| b - b'0').collect();
        let point = digits.len() as i32;
        let mut d = Digits { neg: v < 0, digits, point };
        d.trim();
        d
    }

    /// The exact decimal value of a finite double, `extra` digits after the point at least.
    fn from_f64_exact(v: f64, decimals: usize) -> Digits {
        let s = format!("{:.*}", decimals + 25, v.abs());
        Self::from_plain(&s, v.is_sign_negative() && v != 0.0)
    }

    /// The double rounded to `sig` significant digits.
    fn from_f64_sig(v: f64, sig: usize) -> Digits {
        if v == 0.0 {
            return Digits { neg: v.is_sign_negative(), digits: vec![0], point: 1 };
        }
        let s = format!("{:.*e}", sig - 1, v.abs());
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let digits: Vec<u8> = mant.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
        let mut d = Digits { neg: v < 0.0, digits, point: exp + 1 };
        d.trim();
        d
    }

    /// The shortest digits that read back as the same double (Rust's `{:e}`, same as .NET's "R").
    fn from_f64_shortest(v: f64) -> Digits {
        if v == 0.0 {
            return Digits { neg: v.is_sign_negative(), digits: vec![0], point: 1 };
        }
        let s = format!("{:e}", v.abs());
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let digits: Vec<u8> = mant.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
        let mut d = Digits { neg: v < 0.0, digits, point: exp + 1 };
        d.trim();
        d
    }

    fn from_plain(s: &str, neg: bool) -> Digits {
        let (ip, fp) = s.split_once('.').unwrap_or((s, ""));
        let mut digits: Vec<u8> = ip.bytes().map(|b| b - b'0').collect();
        let point = digits.len() as i32;
        digits.extend(fp.bytes().map(|b| b - b'0'));
        let mut d = Digits { neg, digits, point };
        d.trim();
        d
    }

    fn trim(&mut self) {
        while self.digits.len() > 1 && self.digits[0] == 0 {
            self.digits.remove(0);
            self.point -= 1;
        }
        while self.digits.len() > 1 && *self.digits.last().unwrap() == 0 {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.digits.push(0);
            self.point = 1;
        }
        if self.is_zero() {
            self.digits = vec![0];
            self.point = 1;
        }
    }

    /// Rounds half away from zero to `decimals` digits after the point.
    fn round(&mut self, decimals: i32) {
        let keep = self.point + decimals; // digits to keep
        if keep < 0 {
            self.digits = vec![0];
            self.point = 1;
            return;
        }
        let keep = keep as usize;
        if keep >= self.digits.len() {
            return;
        }
        let up = self.digits[keep] >= 5;
        self.digits.truncate(keep);
        if up {
            let mut k = keep;
            loop {
                if k == 0 {
                    self.digits.insert(0, 1);
                    self.point += 1;
                    break;
                }
                k -= 1;
                if self.digits[k] == 9 {
                    self.digits[k] = 0;
                } else {
                    self.digits[k] += 1;
                    break;
                }
            }
        }
        if self.digits.is_empty() {
            self.digits.push(0);
            self.point = 1;
        }
        self.trim();
    }

    /// Integer digits as text (at least `min` digits) and fraction digits (between min_frac and max_frac).
    fn parts(&self, min_int: usize, min_frac: usize, max_frac: usize) -> (String, String) {
        let mut int = String::new();
        if self.point > 0 {
            for k in 0..self.point as usize {
                int.push((b'0' + self.digits.get(k).copied().unwrap_or(0)) as char);
            }
        }
        let int = int.trim_start_matches('0').to_string();
        let mut int = int;
        while int.len() < min_int {
            int.insert(0, '0');
        }
        let mut frac = String::new();
        for k in 0..max_frac {
            let idx = self.point + k as i32;
            let d = if idx >= 0 { self.digits.get(idx as usize).copied().unwrap_or(0) } else { 0 };
            frac.push((b'0' + d) as char);
        }
        while frac.len() > min_frac && frac.ends_with('0') {
            frac.pop();
        }
        (int, frac)
    }
}

fn group(int: &str, sep: &str) -> String {
    let b: Vec<char> = int.chars().collect();
    let mut out = String::new();
    for (k, ch) in b.iter().enumerate() {
        if k > 0 && (b.len() - k) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(*ch);
    }
    out
}

/// Splits "N2" into ('N', Some(2)); None when the text is not a standard format.
fn standard(f: &str) -> Option<(char, Option<usize>)> {
    let mut it = f.chars();
    let c = it.next()?;
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let rest: String = it.collect();
    if rest.is_empty() {
        return Some((c, None));
    }
    if rest.len() <= 9 && rest.chars().all(|x| x.is_ascii_digit()) {
        return Some((c, rest.parse().ok()));
    }
    None
}

fn fmt_int(v: i128, f: &str, c: &Culture) -> String {
    let neg = |s: String| if v < 0 { format!("{}{}", c.negative, s) } else { s };
    if f.is_empty() {
        return neg(v.unsigned_abs().to_string());
    }
    if let Some((k, p)) = standard(f) {
        match k {
            'D' | 'd' => {
                let s = v.unsigned_abs().to_string();
                let w = p.unwrap_or(0);
                return neg(format!("{:0>w$}", s, w = w));
            }
            'X' | 'x' => {
                // Hex of the two's complement value of the original width is what C# prints; i64/u64 cover the program.
                let u = if v < 0 { (v as i64) as u64 } else { v as u64 };
                let s = if k == 'X' { format!("{:X}", u) } else { format!("{:x}", u) };
                return format!("{:0>w$}", s, w = p.unwrap_or(0));
            }
            'G' | 'g' if p.is_none() => return neg(v.unsigned_abs().to_string()),
            'N' | 'n' | 'F' | 'f' | 'P' | 'p' | 'E' | 'e' | 'G' | 'g' => {
                return fmt_standard(Digits::from_int(v), k, p, c, true);
            }
            _ => {}
        }
    }
    custom(Digits::from_int(v), f, c, false)
}

fn fmt_double(v: f64, f: &str, c: &Culture) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "∞".into() } else { format!("{}∞", c.negative) };
    }
    if f.is_empty() || f == "R" || f == "r" || f == "G" || f == "g" {
        return shortest(v, c);
    }
    if let Some((k, p)) = standard(f) {
        match k {
            'N' | 'n' | 'F' | 'f' | 'P' | 'p' | 'E' | 'e' | 'G' | 'g' => {
                let decimals = match k {
                    'N' | 'n' | 'F' | 'f' => p.unwrap_or(2),
                    'P' | 'p' => p.unwrap_or(2) + 2,
                    _ => 17,
                };
                let d = if k == 'G' || k == 'g' { Digits::from_f64_sig(v, p.unwrap_or(15).max(1)) } else { Digits::from_f64_exact(v, decimals) };
                return fmt_standard(d, k, p, c, false);
            }
            'R' | 'r' => return shortest(v, c),
            _ => {}
        }
    }
    custom(Digits::from_f64_sig(v, 15), f, c, true)
}

fn shortest(v: f64, c: &Culture) -> String {
    let d = Digits::from_f64_shortest(v);
    let exp = d.point - 1;
    let sign = if d.neg && !d.is_zero() { c.negative.as_str() } else { "" };
    if d.is_zero() {
        return if v.is_sign_negative() { format!("{}0", c.negative) } else { "0".into() };
    }
    if exp >= 15 || exp < -4 {
        let mant: String = d.digits.iter().map(|x| (b'0' + x) as char).collect();
        let m = if mant.len() > 1 { format!("{}{}{}", &mant[..1], c.decimal, &mant[1..]) } else { mant };
        return format!("{}{}E{}{:02}", sign, m, if exp < 0 { "-" } else { "+" }, exp.abs());
    }
    let frac_len = (d.digits.len() as i32 - d.point).max(0) as usize;
    let (int, frac) = d.parts(1, 0, frac_len);
    if frac.is_empty() { format!("{}{}", sign, int) } else { format!("{}{}{}{}", sign, int, c.decimal, frac) }
}

fn fmt_standard(mut d: Digits, k: char, p: Option<usize>, c: &Culture, _int: bool) -> String {
    let neg_of = |d: &Digits, s: String| if d.neg && !d.is_zero() { format!("{}{}", c.negative, s) } else { s };
    match k {
        'N' | 'n' | 'F' | 'f' => {
            let dec = p.unwrap_or(2);
            d.round(dec as i32);
            let (int, frac) = d.parts(1, dec, dec);
            let int = if k == 'N' || k == 'n' { group(&int, &c.group) } else { int };
            neg_of(&d, if dec > 0 { format!("{}{}{}", int, c.decimal, frac) } else { int })
        }
        'P' | 'p' => {
            let dec = p.unwrap_or(2);
            d.point += 2;
            d.round(dec as i32);
            let (int, frac) = d.parts(1, dec, dec);
            let s = if dec > 0 { format!("{}{}{}", group(&int, &c.group), c.decimal, frac) } else { group(&int, &c.group) };
            neg_of(&d, format!("{} {}", s, c.percent))
        }
        'E' | 'e' => {
            let prec = p.unwrap_or(6);
            let exp = d.point - 1;
            let mut m = d.clone();
            m.point = 1;
            m.round(prec as i32);
            let exp = exp + (m.point - 1);
            m.point = 1;
            let (int, frac) = m.parts(1, prec, prec);
            let body = if prec > 0 { format!("{}{}{}", int, c.decimal, frac) } else { int };
            neg_of(&d, format!("{}{}{}{:03}", body, if k == 'E' { 'E' } else { 'e' }, if exp < 0 { '-' } else { '+' }, exp.abs()))
        }
        _ => {
            // G with a precision
            let exp = d.point - 1;
            if exp >= -5 && exp < p.unwrap_or(15) as i32 {
                let frac_len = (d.digits.len() as i32 - d.point).max(0) as usize;
                let (int, frac) = d.parts(1, 0, frac_len);
                neg_of(&d, if frac.is_empty() { int } else { format!("{}{}{}", int, c.decimal, frac) })
            } else {
                let mant: String = d.digits.iter().map(|x| (b'0' + x) as char).collect();
                let m = if mant.len() > 1 { format!("{}{}{}", &mant[..1], c.decimal, &mant[1..]) } else { mant };
                neg_of(&d, format!("{}E{}{:02}", m, if exp < 0 { "-" } else { "+" }, exp.abs()))
            }
        }
    }
}

/// Splits a custom format into its sections (`;` outside quotes and escapes).
fn sections(f: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut q: Option<char> = None;
    let mut esc = false;
    for ch in f.chars() {
        if esc {
            out.last_mut().unwrap().push(ch);
            esc = false;
            continue;
        }
        match (q, ch) {
            (None, '\\') => {
                esc = true;
                out.last_mut().unwrap().push(ch);
            }
            (None, '\'') | (None, '"') => {
                q = Some(ch);
                out.last_mut().unwrap().push(ch);
            }
            (Some(x), y) if x == y => {
                q = None;
                out.last_mut().unwrap().push(ch);
            }
            (None, ';') => out.push(String::new()),
            _ => out.last_mut().unwrap().push(ch),
        }
    }
    out
}

/// One token of a custom numeric section.
enum Tok {
    Zero,
    Hash,
    Point,
    Comma,
    Percent,
    Permille,
    Lit(String),
}

fn tokens(s: &str) -> Vec<Tok> {
    let ch: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        match ch[i] {
            '0' => out.push(Tok::Zero),
            '#' => out.push(Tok::Hash),
            '.' => out.push(Tok::Point),
            ',' => out.push(Tok::Comma),
            '%' => out.push(Tok::Percent),
            '‰' => out.push(Tok::Permille),
            '\\' => {
                if i + 1 < ch.len() {
                    out.push(Tok::Lit(ch[i + 1].to_string()));
                    i += 1;
                }
            }
            q @ ('\'' | '"') => {
                let mut lit = String::new();
                i += 1;
                while i < ch.len() && ch[i] != q {
                    lit.push(ch[i]);
                    i += 1;
                }
                out.push(Tok::Lit(lit));
            }
            x => out.push(Tok::Lit(x.to_string())),
        }
        i += 1;
    }
    out
}

/// A custom numeric format ("0.0", "+0;-0;0", "#,##0", "0%").
fn custom(d: Digits, f: &str, c: &Culture, _double: bool) -> String {
    let secs = sections(f);
    let neg = d.neg && !d.is_zero();
    // Sections: one = every value; two = positive and zero ; negative; three = positive ; negative ; zero.
    // An empty negative or zero section uses the first one (the negative value then gets its minus sign).
    let (sec, minus): (&str, bool) = if secs.len() == 1 {
        (&secs[0], neg)
    } else if d.is_zero() {
        if secs.len() >= 3 && !secs[2].is_empty() { (&secs[2], false) } else { (&secs[0], false) }
    } else if neg {
        if !secs[1].is_empty() { (&secs[1], false) } else { (&secs[0], true) }
    } else {
        (&secs[0], false)
    };
    let (text, zero) = render_section(&d, sec, c);
    // A value that is not zero but rounds to zero in its section takes the zero section.
    if !d.is_zero() && zero && secs.len() >= 3 && !secs[2].is_empty() {
        return render_section(&Digits { neg: false, digits: vec![0], point: 1 }, &secs[2], c).0;
    }
    // .NET Core 3.0+: a negative value that rounds to zero in a one-section format still shows "-0".
    if minus { format!("{}{}", c.negative, text) } else { text }
}

/// Renders one section; returns the text and whether the rounded value is zero.
fn render_section(d: &Digits, sec: &str, c: &Culture) -> (String, bool) {
    let toks = tokens(sec);
    let first_ph = toks.iter().position(|t| matches!(t, Tok::Zero | Tok::Hash));
    let last_ph = toks.iter().rposition(|t| matches!(t, Tok::Zero | Tok::Hash));
    let point = toks.iter().position(|t| matches!(t, Tok::Point));
    let mut d = d.clone();
    d.neg = false;
    if toks.iter().any(|t| matches!(t, Tok::Percent)) {
        d.point += 2;
    }
    if toks.iter().any(|t| matches!(t, Tok::Permille)) {
        d.point += 3;
    }
    let (Some(fp), Some(lp)) = (first_ph, last_ph) else {
        // No digit placeholder: the literals only.
        let mut s = String::new();
        for t in &toks {
            match t {
                Tok::Lit(l) => s.push_str(l),
                Tok::Percent => s.push_str(&c.percent),
                Tok::Permille => s.push('‰'),
                _ => {}
            }
        }
        return (s, d.is_zero());
    };
    let int_end = point.unwrap_or(lp + 1).min(lp + 1);
    // Scaling commas: commas right before the point (or the end of the placeholders) divide by 1000 each.
    let mut k = if let Some(p) = point { p } else { lp + 1 };
    while k > 0 && matches!(toks[k - 1], Tok::Comma) {
        d.point -= 3;
        k -= 1;
    }
    let int_scale_end = k;
    let mut min_int = 0usize;
    let mut seen_zero = false;
    let mut grouping = false;
    for t in toks.iter().take(int_end).skip(fp) {
        match t {
            Tok::Zero => {
                seen_zero = true;
                min_int += 1;
            }
            Tok::Hash => {
                if seen_zero {
                    min_int += 1;
                }
            }
            _ => {}
        }
    }
    for (idx, t) in toks.iter().enumerate().take(int_scale_end).skip(fp) {
        if matches!(t, Tok::Comma) && idx < int_scale_end && toks[idx + 1..int_end].iter().any(|x| matches!(x, Tok::Zero | Tok::Hash)) {
            grouping = true;
        }
    }
    let (mut min_frac, mut max_frac) = (0usize, 0usize);
    if let Some(p) = point {
        for (j, t) in toks.iter().enumerate().skip(p + 1) {
            if j > lp {
                break;
            }
            match t {
                Tok::Zero => {
                    max_frac += 1;
                    min_frac = max_frac;
                }
                Tok::Hash => max_frac += 1,
                _ => {}
            }
        }
    }
    d.round(max_frac as i32);
    let zero = d.is_zero();
    let (mut int, frac) = d.parts(min_int, min_frac, max_frac);
    if grouping {
        int = group(&int, &c.group);
    }
    let mut out = String::new();
    for t in toks.iter().take(fp) {
        match t {
            Tok::Lit(l) => out.push_str(l),
            Tok::Percent => out.push_str(&c.percent),
            Tok::Permille => out.push('‰'),
            _ => {}
        }
    }
    out.push_str(&int);
    // Literals between the placeholders of the integer part go after the integer digits (simplified).
    for t in toks.iter().take(int_end).skip(fp) {
        if let Tok::Lit(l) = t {
            out.push_str(l);
        }
    }
    if !frac.is_empty() {
        out.push_str(&c.decimal);
        out.push_str(&frac);
    }
    if let Some(p) = point {
        for t in toks.iter().take(lp + 1).skip(p + 1) {
            if let Tok::Lit(l) = t {
                out.push_str(l);
            }
        }
    }
    for t in toks.iter().skip(lp + 1) {
        match t {
            Tok::Lit(l) => out.push_str(l),
            Tok::Percent => out.push_str(&c.percent),
            Tok::Permille => out.push('‰'),
            _ => {}
        }
    }
    (out, zero)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv() -> Culture {
        Culture::invariant()
    }

    fn ru() -> Culture {
        let mut c = Culture::invariant();
        c.decimal = ",".into();
        c.group = "\u{a0}".into();
        c
    }

    #[test]
    fn composite() {
        let c = inv();
        assert_eq!(format(&c, "a {0} b {1}", &["x".into(), 5.into()]).unwrap(), "a x b 5");
        assert_eq!(format(&c, "{{{0}}}", &[1.into()]).unwrap(), "{1}");
        assert_eq!(format(&c, "[{0,4}][{0,-4}]", &[7.into()]).unwrap(), "[   7][7   ]");
        assert!(format(&c, "{1}", &[1.into()]).is_err());
        assert!(format(&c, "{x}", &[1.into()]).is_err());
        assert!(format(&c, "}", &[]).is_err());
        assert_eq!(format(&c, "{0} {1}", &[true.into(), false.into()]).unwrap(), "True False");
    }

    #[test]
    fn numbers_like_dotnet() {
        let c = inv();
        assert_eq!(num(2.675, "0.00", &c), "2.68");     // 15 digits first, then half away from zero
        assert_eq!(num(0.125, "0.00", &c), "0.13");
        assert_eq!(num(1.25, "0.0", &c), "1.3");
        assert_eq!(num(-1.25, "0.0", &c), "-1.3");
        assert_eq!(num(3.0, "0.0", &c), "3.0");
        assert_eq!(num(99.6, "0", &c), "100");
        assert_eq!(num(-0.4, "0", &c), "-0");
        assert_eq!(num(5.0, "+0;-0;0", &c), "+5");
        assert_eq!(num(-5.0, "+0;-0;0", &c), "-5");
        assert_eq!(num(0.0, "+0;-0;0", &c), "0");
        assert_eq!(num(-0.4, "+0;-0;0", &c), "0");
        assert_eq!(int(150, "+0;-0;0", &c), "+150");
        assert_eq!(int(-150, "+0;-0;0", &c), "-150");
        assert_eq!(num(1234567.891, "N0", &c), "1,234,568");
        assert_eq!(num(1234.5, "N1", &ru()), "1\u{a0}234,5");
        assert_eq!(num(0.5, "0.0", &ru()), "0,5");
        assert_eq!(num(0.5, "", &ru()), "0,5");
        assert_eq!(num(1.0, "", &c), "1");
        assert_eq!(num(0.1 + 0.2, "", &c), "0.30000000000000004");
        assert_eq!(num(1e15, "", &c), "1E+15");
        assert_eq!(num(0.00001, "", &c), "1E-05");
        assert_eq!(num(0.0001, "", &c), "0.0001");
        assert_eq!(int(255, "x", &c), "ff");
        assert_eq!(int(255, "X4", &c), "00FF");
        assert_eq!(int(7, "D3", &c), "007");
        assert_eq!(num(0.256, "0%", &c), "26%");
        assert_eq!(num(12.0, "#,##0", &c), "12");
        assert_eq!(num(1234.0, "#,##0", &c), "1,234");
        assert_eq!(num(0.5, "F0", &c), "1");
        assert_eq!(num(2.5, "F0", &c), "3");
        assert_eq!(num(1.005, "F2", &c), "1.00");      // standard formats: the exact value (1.00499...)
        assert_eq!(int(5, "0.0", &c), "5.0");
        assert_eq!(num(0.05, "0.0", &c), "0.1");
        assert_eq!(num(-0.0, "", &c), "-0");
    }
}
