//! A small seeded random generator for the ported fuzz tests (C# `new Random(seed)`; the sequence differs from .NET's,
//! the properties checked are the same). `DS_FUZZ_ITER=100000` runs longer, as in the C# tests.

pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Self {
        Rand(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn step(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `Next(max)`: 0..max-1 (0 when max <= 0).
    pub fn next(&mut self, max: i32) -> i32 {
        if max <= 0 {
            return 0;
        }
        (self.step() % max as u64) as i32
    }

    /// `Next(min, max)`: min..max-1.
    pub fn range(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + (self.step() % (max as i64 - min as i64) as u64) as i32
    }

    /// `Next()`: 0..i32::MAX-1.
    pub fn next_i32(&mut self) -> i32 {
        (self.step() % i32::MAX as u64) as i32
    }

    pub fn next_u64(&mut self) -> u64 {
        self.step()
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.step() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `NextBytes`.
    pub fn fill(&mut self, b: &mut [u8]) {
        for x in b.iter_mut() {
            *x = self.step() as u8;
        }
    }

    /// A random buffer of 0..max-1 bytes.
    pub fn bytes(&mut self, max: i32) -> Vec<u8> {
        let mut b = vec![0u8; self.next(max) as usize];
        self.fill(&mut b);
        b
    }

    /// A random text of 0..max-1 chars from the C# fuzz alphabet.
    pub fn text(&mut self, max: i32) -> String {
        const ALPHABET: &str = "{}[]\":,0123456789abcdefABCDEF-+.eE \n\t\\/xyzDOSTАБВ\u{0}\u{ffff}😀";
        let a: Vec<char> = ALPHABET.chars().collect();
        let len = self.next(max);
        (0..len).map(|_| a[self.next(a.len() as i32) as usize]).collect()
    }

    /// Flips some bytes of a good input and sometimes cuts its end (FuzzTests.Mutate).
    pub fn mutate(&mut self, good: &[u8]) -> Vec<u8> {
        let mut b = good.to_vec();
        let flips = self.range(1, 12);
        for _ in 0..flips {
            if b.is_empty() {
                break;
            }
            let at = self.next(b.len() as i32) as usize;
            b[at] = match self.next(4) {
                0 => 0,
                1 => 0xFF,
                2 => b[at] ^ (1 << self.next(8)),
                _ => self.next(256) as u8,
            };
        }
        if self.next(5) == 0 {
            let cut = self.range(1, 64) as usize;
            b.truncate(b.len().saturating_sub(cut));
        }
        b
    }
}

/// The iteration count of the fuzz tests (DS_FUZZ_ITER, default 3000).
pub fn iterations() -> usize {
    std::env::var("DS_FUZZ_ITER").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(3000)
}
