//! The version of this build, the update channel, the tester key, version order, the update feeds, downloads and the
//! program-file swap (port of `Updater.cs`: `AppVersion`, `TesterKey`, `UpdateVersion`, `UpdateFile`, `UpdateFeed`,
//! `UpdateCheck`, `Updater`). The same URLs, keys, signature check (ECDSA P-256 over SHA-256, base64 DER), sealed test
//! feed (AES-256-GCM, "DOSE1" | nonce 12 | tag 16 | ciphertext) and `.signed` format.
//!
//! The C# async methods are blocking functions here (the app calls them from a worker thread); no caller passes a
//! cancellation token, so there is none.

use super::http::{resolve, Client, HttpError};
use super::util::{hex_lower, hex_upper, random_bytes};
use crate::base::ci;
use crate::base::hooks::Hook;
use crate::base::l::L;
use crate::base::settings::Settings;
use crate::base::state_dir::store_dir;
use crate::base::time::{DateTime, TimeSpan, TICKS_PER_MINUTE};
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
#[cfg(test)]
use std::sync::Arc;

/// `Convert.FromBase64String`: white space anywhere is ignored, the padding must be right, unused trailing bits are
/// ignored.
pub fn from_base64(s: &str) -> Option<Vec<u8>> {
    const CFG: GeneralPurposeConfig =
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true).with_decode_padding_mode(DecodePaddingMode::RequireCanonical);
    const ENGINE: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::STANDARD, CFG);
    let clean: String = s.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')).collect();
    ENGINE.decode(clean.as_bytes()).ok()
}

/// `Convert.ToBase64String`.
pub fn to_base64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// `string.IsNullOrWhiteSpace`.
fn blank(s: Option<&str>) -> bool {
    s.map(|x| x.trim().is_empty()).unwrap_or(true)
}

// ---- AppVersion -------------------------------------------------------------------------------------------------

static VERSION_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// The version of this build and the update channel (1.15.0). Stable: only the stable releases. Test: the test builds
/// and the stable releases, whichever is newer. The test channel is on while a valid tester key is saved (Settings ->
/// Tester); the test builds are encrypted with that key, so without it there is nothing to read.
pub struct AppVersion;

impl AppVersion {
    /// "1.15.0" or "1.15.0-test1.1" (the informational version without the "+commit" tail).
    pub fn full() -> String {
        if let Some(v) = VERSION_OVERRIDE.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return v.clone();
        }
        crate::base::version::full().to_string()
    }

    /// "stable" or "test": test while a tester key is saved, unless the tester chose "stable releases only" (1.16.0:
    /// Settings "TestBuilds" = false keeps the key but asks for stable releases only).
    pub fn channel() -> &'static str {
        Self::channel_for(TesterKey::current().is_some(), Settings::get_bool_or("TestBuilds", true))
    }

    pub fn channel_for(has_key: bool, wants_test: bool) -> &'static str {
        if has_key && wants_test { "test" } else { "stable" }
    }

    pub fn is_test() -> bool {
        Self::channel() == "test"
    }

    /// What the window shows: "1.15.0" or "1.15.0 Test 1.1".
    pub fn display() -> String {
        Self::describe(Some(&Self::full()))
    }

    pub fn describe(v: Option<&str>) -> String {
        match UpdateVersion::parse(v) {
            None => v.unwrap_or("").to_string(),
            Some(p) => match &p.test {
                None => p.core_text(),
                Some(t) => format!("{} Test {}", p.core_text(), t),
            },
        }
    }

    /// For tests (None: the built version again).
    pub fn override_full(full: Option<&str>) {
        *VERSION_OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = full.map(|s| s.to_string());
    }
}

// ---- TesterKey --------------------------------------------------------------------------------------------------

/// Why a sealed file did not open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SealError {
    /// `InvalidDataException("Not a sealed Domain OS file.")`.
    NotSealed,
    /// `CryptographicException`: a wrong key or a changed file.
    Crypto,
}

impl SealError {
    pub fn message(&self) -> String {
        match self {
            SealError::NotSealed => "Not a sealed Domain OS file.".into(),
            SealError::Crypto => "The computed authentication tag did not match the input authentication tag.".into(),
        }
    }
}

/// The tester key (1.15.0 Test 1.1): 32 random bytes shown as "DOST-XXXX-XXXX-…" (base32). Test builds and the test
/// feed are sealed with AES-256-GCM under it; the key is handed out only in the closed testers channel. A wrong or
/// replaced key opens nothing (GCM refuses it), so there is no way to read a test build without the current key.
pub struct TesterKey;

impl TesterKey {
    const ABC: &'static str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    const MAGIC: &'static [u8; 5] = b"DOSE1";

    /// The saved key (Settings "TesterKey"), None when none or unreadable.
    pub fn current() -> Option<Vec<u8>> {
        Self::parse(Some(&Settings::get("TesterKey")))
    }

    pub fn parse(text: Option<&str>) -> Option<Vec<u8>> {
        if blank(text) {
            return None;
        }
        let mut t: String = text.unwrap_or("").trim().chars().map(ci::upper_char).collect();
        if t.starts_with("DOST") {
            t = t[4..].to_string();
        }
        let t: String = t.chars().filter(|c| *c != '-' && *c != ' ').collect();
        let (mut bits, mut acc) = (0i32, 0i32);
        let mut out = Vec::new();
        for c in t.chars() {
            let v = Self::ABC.find(c)? as i32;
            acc = (acc << 5) | v;
            bits += 5;
            if bits >= 8 {
                out.push((acc >> (bits - 8)) as u8);
                bits -= 8;
                acc &= (1 << bits) - 1;
            }
        }
        if out.len() == 32 { Some(out) } else { None }
    }

    pub fn format(key: &[u8]) -> String {
        let abc = Self::ABC.as_bytes();
        let mut sb = String::new();
        let (mut bits, mut acc) = (0i32, 0i32);
        for &b in key {
            acc = (acc << 8) | b as i32;
            bits += 8;
            while bits >= 5 {
                sb.push(abc[((acc >> (bits - 5)) & 31) as usize] as char);
                bits -= 5;
            }
            acc &= (1 << bits) - 1;
        }
        if bits > 0 {
            sb.push(abc[((acc << (5 - bits)) & 31) as usize] as char);
        }
        let groups: Vec<&str> = (0..sb.len().div_ceil(4)).map(|i| &sb[i * 4..(i * 4 + 4).min(sb.len())]).collect();
        format!("DOST-{}", groups.join("-"))
    }

    /// "DOSE1" | nonce 12 | tag 16 | ciphertext. Err for a wrong key or a changed file.
    #[allow(deprecated)]
    pub fn open(sealed: &[u8], key: &[u8]) -> Result<Vec<u8>, SealError> {
        use aes_gcm::aead::AeadInPlace;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce, Tag};
        if sealed.len() < 33 || &sealed[..5] != Self::MAGIC {
            return Err(SealError::NotSealed);
        }
        let nonce = &sealed[5..17];
        let tag = &sealed[17..33];
        let mut plain = sealed[33..].to_vec();
        let gcm = Aes256Gcm::new_from_slice(key).map_err(|_| SealError::Crypto)?;
        gcm.decrypt_in_place_detached(Nonce::from_slice(nonce), b"", &mut plain, Tag::from_slice(tag)).map_err(|_| SealError::Crypto)?;
        Ok(plain)
    }

    #[allow(deprecated)]
    pub fn seal(plain: &[u8], key: &[u8]) -> Result<Vec<u8>, SealError> {
        use aes_gcm::aead::AeadInPlace;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        let nonce = random_bytes(12);
        let gcm = Aes256Gcm::new_from_slice(key).map_err(|_| SealError::Crypto)?;
        let mut ct = plain.to_vec();
        let tag = gcm.encrypt_in_place_detached(Nonce::from_slice(&nonce), b"", &mut ct).map_err(|_| SealError::Crypto)?;
        let mut out = Self::MAGIC.to_vec();
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&tag);
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

// ---- UpdateVersion ----------------------------------------------------------------------------------------------

/// A parsed version: the core (major, minor, build) and the test part ("1.1"; None for a stable release).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V {
    pub core: (i32, i32, i32),
    pub test: Option<String>,
    pub test_parts: Option<Vec<i32>>,
}

impl V {
    /// `Core.ToString(3)`.
    pub fn core_text(&self) -> String {
        format!("{}.{}.{}", self.core.0, self.core.1, self.core.2)
    }
}

/// `Version.TryParse`: 2 to 4 parts, each `int.TryParse(NumberStyles.Integer)` and not negative; (major, minor, build)
/// with build -1 when there are only two parts.
fn version_try_parse(s: &str) -> Option<(i32, i32, i32)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 2 || parts.len() > 4 {
        return None;
    }
    let mut v = Vec::new();
    for p in &parts {
        let x = crate::base::settings::parse_int_integer_style(p)?;
        if x < 0 {
            return None;
        }
        v.push(x);
    }
    Some((v[0], v[1], if v.len() >= 3 { v[2] } else { -1 }))
}

/// "1.15.0", "1.15.0-test1.0", "1.15.0-test1.1": a stable release is newer than every test build of the same number.
pub struct UpdateVersion;

impl UpdateVersion {
    pub fn parse(s: Option<&str>) -> Option<V> {
        if blank(s) {
            return None;
        }
        let mut s = s.unwrap_or("").trim().trim_start_matches(['v', 'V']);
        if let Some(plus) = s.find('+') {
            s = &s[..plus];
        }
        let mut core = s;
        let mut test: Option<String> = None;
        if let Some(dash) = s.find('-') {
            core = &s[..dash];
            let tail = &s[dash + 1..];
            if !ci::starts_with(tail, "test") {
                return None;
            }
            let mut t = tail[4..].trim_start_matches(['.', '-']).to_string();
            if t.is_empty() {
                t = "0".into();
            }
            test = Some(t);
        }
        let (ma, mi, mut bu) = version_try_parse(core)?;
        if bu < 0 {
            bu = 0;
        }
        let mut parts = None;
        if let Some(t) = &test {
            let mut p = Vec::new();
            for bit in t.split('.') {
                p.push(crate::base::settings::parse_int_integer_style(bit)?);
            }
            parts = Some(p);
        }
        Some(V { core: (ma, mi, bu), test, test_parts: parts })
    }

    /// <0 when a is older than b, 0 the same, >0 newer. Unreadable versions count as the oldest.
    pub fn compare(a: Option<&str>, b: Option<&str>) -> i32 {
        let x = Self::parse(a);
        let y = Self::parse(b);
        let (x, y) = match (x, y) {
            (Some(x), Some(y)) => (x, y),
            (x, y) => return (if x.is_none() { 0 } else { 1 }) - (if y.is_none() { 0 } else { 1 }),
        };
        let c = x.core.cmp(&y.core);
        if c != std::cmp::Ordering::Equal {
            return c as i32;
        }
        if x.test.is_none() || y.test.is_none() {
            return (if x.test.is_none() { 1 } else { 0 }) - (if y.test.is_none() { 1 } else { 0 });
        }
        let p = x.test_parts.unwrap_or_default();
        let q = y.test_parts.unwrap_or_default();
        for i in 0..p.len().max(q.len()) {
            let u = p.get(i).copied().unwrap_or(0);
            let w = q.get(i).copied().unwrap_or(0);
            if u != w {
                return u.cmp(&w) as i32;
            }
        }
        0
    }
}

// ---- feeds -------------------------------------------------------------------------------------------------------

/// One file in an update feed: Toolbox or the playbook. Sealed: the file is encrypted with the tester key
/// (sha256/size are of the encrypted file). Token (1.16.0 Test 1.8): test builds live in a private repository that only
/// the owner and the testers can see; the sealed test feed carries a read-only GitHub token for that one repository,
/// and the download asks the GitHub API with it. Only a sealed entry may carry a token, and only for an
/// api.github.com URL.
#[derive(Clone, PartialEq, Eq)]
pub struct UpdateFile {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: i64,
    pub notes_en: String,
    pub notes_ru: String,
    pub sealed_: bool,
    pub token: Option<String>,
}

impl UpdateFile {
    pub fn new(version: &str, url: &str, sha256: &str, size: i64, notes_en: &str, notes_ru: &str) -> Self {
        UpdateFile {
            version: version.into(),
            url: url.into(),
            sha256: sha256.into(),
            size,
            notes_en: notes_en.into(),
            notes_ru: notes_ru.into(),
            sealed_: false,
            token: None,
        }
    }
}

/// Never prints the token (logs, errors, debugger).
impl std::fmt::Display for UpdateFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.version, self.url)
    }
}

impl std::fmt::Debug for UpdateFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.version, self.url)
    }
}

/// update/stable-native.json or update/test-native.bin on the site (`Updater::STABLE_FEED`, `Updater::TEST_FEED`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateFeed {
    pub toolbox: Option<UpdateFile>,
    pub playbook: Option<UpdateFile>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateCheck {
    pub toolbox: Option<UpdateFile>,
    pub playbook: Option<UpdateFile>,
    pub error: Option<String>,
}

impl UpdateCheck {
    pub fn any(&self) -> bool {
        self.toolbox.is_some() || self.playbook.is_some()
    }

    fn failed(e: String) -> Self {
        UpdateCheck { toolbox: None, playbook: None, error: Some(e) }
    }
}

/// One trusted update key: id (see `Updater::key_id`), a short name for the log, the public key (base64 SPKI DER).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    pub id: String,
    pub name: String,
    pub public_key: String,
}

impl TrustedKey {
    pub fn new(id: &str, name: &str, public_key: &str) -> Self {
        TrustedKey { id: id.into(), name: name.into(), public_key: public_key.into() }
    }
}

/// Why a file operation of the updater failed (the C# exception type matters to the callers and tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdError {
    /// `InvalidDataException`.
    InvalidData(String),
    /// `FileNotFoundException`.
    NotFound(String),
    /// `IOException` and the rest.
    Io(String),
}

impl UpdError {
    pub fn message(&self) -> &str {
        match self {
            UpdError::InvalidData(m) | UpdError::NotFound(m) | UpdError::Io(m) => m,
        }
    }
}

impl std::fmt::Display for UpdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

fn io(e: std::io::Error) -> UpdError {
    UpdError::Io(e.to_string())
}

/// Test hook for the pauses between retries (milliseconds); None: `Thread.Sleep`.
pub static RETRY_SLEEP: Hook<dyn Fn(i32) + Send + Sync> = Hook::new();

/// The update check (1.15.0). It reads one or two small JSON files from the Domain OS site and nothing else is sent:
/// no ID, no hardware, only a plain User-Agent with the version. Feeds are at most 1 MB and must carry a valid
/// signature; a version must look like "1.15.0" or "1.15.0-test1.3" before it is used anywhere.
/// Downloads (1.15.0 Test 1.3) go into memory and are checked there: size and SHA-256 of exactly the bytes that are
/// then installed (a sealed test build is opened in memory too). Toolbox's new program file is written once, straight
/// from those bytes, as the program file (nothing is read back from a folder another account could write to). A
/// playbook is written once into a new folder with a random name inside the admin-only state folder (create-new: an
/// existing file is never reused). Installing Toolbox is done by the app (it has to exit); the playbook is never
/// installed here: its file is downloaded, checked and shown, AME Wizard installs it.
pub struct Updater;

impl Updater {
    pub const FEED_BASE: &'static str = "https://6kane1.github.io/DomainOS/update/";
    /// The native Toolbox's own feeds (1.17.0, owner's decision 2026-11-27). The C# Toolbox 1.16.x reads stable.json and
    /// test.bin; the native exe is never listed there, so a C# client is never offered it: the jump to the native
    /// Toolbox is made only by installing the playbook (2.3.2+). release/hub/domainos.py and tools/publish-test-build.py
    /// write these names.
    pub const STABLE_FEED: &'static str = "stable-native.json";
    pub const TEST_FEED: &'static str = "test-native.bin";
    /// The only place a feed token is ever sent to (a release asset of the private test repository).
    pub const GITHUB_API: &'static str = "https://api.github.com/repos/";

    /// The public half of the main Domain OS update key (ECDSA P-256). Every feed is signed with a private half, which
    /// exists only with the author (the main key on the author's PC, the backup key offline); a feed without a valid
    /// signature is refused. The feed lists each file's SHA-256, so the signature covers the downloads too: someone who
    /// takes over the site or the GitHub account still cannot make Toolbox install anything.
    pub const PUBLIC_KEY: &'static str =
        "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAENxIGlvWT392aI8r24XWshofv30+lyxs2sEMlNVslbKJh1UNZzJEu5Jx1h0EFzmnaNv1fGMBTHHU5mp/rz8gcWg==";

    /// The public half of the backup update key (ECDSA P-256, made 2026-11-19). Its private half is kept offline and is
    /// used only when the main key leaks or is lost: the next feed is signed with it, and the Toolbox in that feed adds
    /// a new backup key and moves the old main key to `REVOKED_KEY_IDS` (rotation steps: README, "Update keys").
    pub const BACKUP_PUBLIC_KEY: &'static str =
        "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEqHD2RMemyHHX5CUG0TWCJfJEIkYzQClvrLb8rmC/BIO/oPJKJsxCoP5Dd4HwUAfdaj9jDeQIQcHPwUN1Qh84lw==";

    /// Ids of keys that must never be trusted again (a leaked key goes here in the next Toolbox). Checked by the key's
    /// own fingerprint, so a revoked key is refused even if it is still (or again) listed in `trusted_keys()`.
    pub const REVOKED_KEY_IDS: &'static [&'static str] = &[];

    /// A feed or its signature larger than this is refused.
    pub const MAX_FEED_BYTES: i64 = 1 << 20;
    /// A download larger than this is refused (Toolbox is far smaller).
    pub const MAX_DOWNLOAD_BYTES: i64 = 512 << 20;

    /// Every key a feed may be signed with: a feed is valid when its signature checks with ANY of them. The .sig file
    /// stays what it always was (base64 DER ECDSA over SHA-256, no key id in it), so feeds signed for older Toolbox
    /// versions, which know only the main key, keep working, and the new code just tries each key in turn.
    pub fn trusted_keys() -> Vec<TrustedKey> {
        vec![TrustedKey::new("824da223f01c0905", "main", Self::PUBLIC_KEY), TrustedKey::new("a807a74083942dde", "backup", Self::BACKUP_PUBLIC_KEY)]
    }

    pub fn revoked_key_ids() -> Vec<String> {
        Self::REVOKED_KEY_IDS.iter().map(|s| s.to_string()).collect()
    }

    /// Key id: the first 16 hex digits (lower case) of SHA-256 over the public key's SPKI DER bytes; None for bad base64.
    pub fn key_id(public_key_base64: Option<&str>) -> Option<String> {
        let der = from_base64(public_key_base64.unwrap_or(""))?;
        Some(hex_upper(&Sha256::digest(&der))[..16].to_lowercase())
    }

    /// True when sig is a valid signature of exactly these bytes by one of the built-in trusted, not revoked keys.
    pub fn verify(data: Option<&[u8]>, sig: Option<&str>) -> bool {
        Self::verify_any(data, sig, &Self::trusted_keys(), &Self::revoked_key_ids()).is_some()
    }

    /// The trusted key that signed these bytes, or None when none did. A key whose fingerprint is in revoked is skipped
    /// (the id listed next to a key is not trusted: the fingerprint is computed from the key itself).
    pub fn verify_any(data: Option<&[u8]>, sig: Option<&str>, keys: &[TrustedKey], revoked: &[String]) -> Option<TrustedKey> {
        let data = data?;
        if blank(sig) {
            return None;
        }
        for k in keys {
            let Some(id) = Self::key_id(Some(&k.public_key)) else { continue };
            if revoked.iter().any(|r| ci::eq(r, &id)) {
                continue;
            }
            if Self::verify_with(data, sig, &k.public_key) {
                return Some(k.clone());
            }
        }
        None
    }

    /// True when sig is a valid signature of exactly these bytes with this one public key (DER ECDSA, SHA-256).
    pub fn verify_with(data: &[u8], sig: Option<&str>, public_key: &str) -> bool {
        use p256::ecdsa::signature::Verifier;
        use p256::ecdsa::{Signature, VerifyingKey};
        use p256::pkcs8::DecodePublicKey;
        let Some(der) = from_base64(public_key) else { return false };
        let Ok(vk) = VerifyingKey::from_public_key_der(&der) else { return false };
        let Some(raw) = from_base64(sig.unwrap_or("").trim()) else { return false };
        let Ok(s) = Signature::from_der(&raw) else { return false };
        vk.verify(data, &s).is_ok()
    }

    /// Downloads: a folder in the admin-only state folder.
    pub fn dir() -> Result<PathBuf, String> {
        Ok(store_dir()?.join("Updates"))
    }

    /// `^\d+\.\d+\.\d+(-test\d+(\.\d+)*)?$` ("1.15.0" or "1.15.0-test1.3": the only version texts used, in names, in
    /// the log, in comparisons). .NET's `$` also matches before one final "\n", so that is accepted as well.
    pub fn version_rx(s: &str) -> bool {
        let s = s.strip_suffix('\n').unwrap_or(s);
        fn digits(b: &[u8], i: &mut usize) -> bool {
            let st = *i;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
            *i > st
        }
        let b = s.as_bytes();
        let mut i = 0;
        for k in 0..3 {
            if !digits(b, &mut i) {
                return false;
            }
            if k < 2 {
                if b.get(i) != Some(&b'.') {
                    return false;
                }
                i += 1;
            }
        }
        if i == b.len() {
            return true;
        }
        if !s[i..].starts_with("-test") {
            return false;
        }
        i += 5;
        if !digits(b, &mut i) {
            return false;
        }
        while i < b.len() {
            if b[i] != b'.' {
                return false;
            }
            i += 1;
            if !digits(b, &mut i) {
                return false;
            }
        }
        true
    }

    /// The playbook version applied last on this PC (history.log written by the playbook), None when none.
    pub fn installed_playbook(history_file: Option<&Path>) -> Option<String> {
        let path = match history_file {
            Some(p) => p.to_path_buf(),
            None => crate::native::sys::common_app_data().join("Domain OS").join("history.log"),
        };
        if !path.is_file() {
            return None;
        }
        let text = super::util::read_all_text(&path).ok()?;
        let mut last: Option<String> = None;
        for line in text.split("\r\n").flat_map(|l| l.split('\n')).flat_map(|l| l.split('\r')) {
            let t: Vec<&str> = line.split(' ').filter(|x| !x.is_empty()).collect();
            if t.len() >= 3 && t[2] == "applied" {
                last = t.get(3).map(|s| s.to_string());
            } else if t.len() >= 3 && ci::starts_with(t[2], "undo") {
                last = None;
            }
        }
        last
    }

    /// Err: the text is not JSON (`JsonException`; the caller treats it as "no feed").
    pub fn parse_feed(json: &str) -> Result<UpdateFeed, String> {
        let root: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        // A feed that is not a JSON object (a number, a list ...) offers nothing; found by the fuzz tests (1.16.0).
        let Some(root) = root.as_object() else { return Ok(UpdateFeed::default()) };
        Ok(UpdateFeed { toolbox: Self::feed_file(root, "toolbox"), playbook: Self::feed_file(root, "playbook") })
    }

    fn feed_file(root: &serde_json::Map<String, serde_json::Value>, name: &str) -> Option<UpdateFile> {
        use serde_json::Value;
        let e = root.get(name)?.as_object()?;
        let s = |k: &str| -> Option<String> {
            match e.get(k) {
                Some(Value::String(v)) => Some(v.clone()),
                _ => None,
            }
        };
        // TryGetInt64: a fractional or huge size ("1.5", "1e30") is a bad entry, not an error (fuzz tests, 1.16.0).
        let size = match e.get("size") {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            _ => 0,
        };
        let sealed_file = matches!(e.get("sealed"), Some(Value::Bool(true)));
        let token = s("token");
        let version = s("version");
        let url = s("url");
        let sha = s("sha256").map(|x| ci::lower(&x));
        // A token goes only with a sealed (test) entry and only to the GitHub API; a token in the open stable feed, a
        // token for another host or a token with odd characters makes the whole entry bad.
        let token_ok = match &token {
            None => true,
            Some(t) => {
                let n = t.encode_utf16().count();
                sealed_file
                    && n > 10
                    && n < 512
                    && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && url.as_deref().map(|u| ci::starts_with(u, Self::GITHUB_API)).unwrap_or(false)
            }
        };
        let ok = token_ok
            && version.as_deref().map(|v| Self::version_rx(v) && UpdateVersion::parse(Some(v)).is_some()).unwrap_or(false)
            && url.as_deref().map(|u| ci::starts_with(u, "https://")).unwrap_or(false)
            && sha.as_deref().map(|h| h.encode_utf16().count() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).unwrap_or(false)
            && size > 0
            && size <= Self::MAX_DOWNLOAD_BYTES;
        if !ok {
            return None;
        }
        Some(UpdateFile {
            version: version.unwrap_or_default(),
            url: url.unwrap_or_default(),
            sha256: sha.unwrap_or_default(),
            size,
            notes_en: s("notes_en").unwrap_or_default(),
            notes_ru: s("notes_ru").unwrap_or_default(),
            sealed_: sealed_file,
            token,
        })
    }

    /// What to offer. A stable build looks at the stable feed only; a test build at both, taking the newest Toolbox.
    /// Only versions newer than what runs (Toolbox) or what was applied (playbook) are offered.
    pub fn choose(current: &str, channel: &str, playbook: Option<&str>, stable: Option<&UpdateFeed>, test: Option<&UpdateFeed>) -> UpdateCheck {
        let feeds: Vec<Option<&UpdateFeed>> = if channel == "test" { vec![stable, test] } else { vec![stable] };
        let mut tb: Option<&UpdateFile> = None;
        let mut pb: Option<&UpdateFile> = None;
        for f in feeds.into_iter().flatten() {
            if let Some(t) = &f.toolbox {
                if UpdateVersion::compare(Some(&t.version), Some(current)) > 0
                    && tb.map(|x| UpdateVersion::compare(Some(&t.version), Some(&x.version)) > 0).unwrap_or(true)
                {
                    tb = Some(t);
                }
            }
            if let (Some(p), Some(applied)) = (&f.playbook, playbook) {
                if UpdateVersion::compare(Some(&p.version), Some(applied)) > 0
                    && pb.map(|x| UpdateVersion::compare(Some(&p.version), Some(&x.version)) > 0).unwrap_or(true)
                {
                    pb = Some(p);
                }
            }
        }
        UpdateCheck { toolbox: tb.cloned(), playbook: pb.cloned(), error: None }
    }

    fn user_agent() -> String {
        format!("DomainOS-Toolbox/{}", AppVersion::full().replace('+', "."))
    }

    fn client(timeout: TimeSpan) -> Client {
        // max_buffer caps every buffered read (feeds, signatures); downloads are streamed and capped by their size.
        Client { timeout, max_buffer: Self::MAX_FEED_BYTES as u64, follow_redirects: true, user_agent: Self::user_agent() }
    }

    /// `CheckAsync`: never fails; a problem is in `error`.
    pub fn check() -> UpdateCheck {
        let http = Self::client(TimeSpan::from_secs_i(20));
        let get = |name: &str| -> Result<Option<UpdateFeed>, String> {
            match Self::fetch(&http, name)? {
                None => Ok(None),
                Some(body) => Self::parse_feed(&String::from_utf8_lossy(&body)).map(Some),
            }
        };
        let stable = match get(Self::STABLE_FEED) {
            Ok(s) => s,
            Err(e) => return UpdateCheck::failed(e),
        };
        let mut test: Option<UpdateFeed> = None;
        if let Some(key) = TesterKey::current() {
            if AppVersion::channel() == "test" {
                let bx = match Self::fetch(&http, Self::TEST_FEED) {
                    Ok(b) => b,
                    Err(e) => return UpdateCheck::failed(e),
                };
                if let Some(bx) = bx {
                    match TesterKey::open(&bx, &key) {
                        Ok(plain) => match Self::parse_feed(&String::from_utf8_lossy(&plain)) {
                            Ok(f) => test = Some(f),
                            Err(e) => return UpdateCheck::failed(e),
                        },
                        Err(SealError::Crypto) => return UpdateCheck::failed(Self::key_refused()),
                        Err(e) => return UpdateCheck::failed(e.message()),
                    }
                }
            }
        }
        Self::choose(&AppVersion::full(), AppVersion::channel(), Self::installed_playbook(None).as_deref(), stable.as_ref(), test.as_ref())
    }

    pub fn key_refused() -> String {
        L::n("The tester key does not open the test builds (it may have been replaced). Take the current key from the testers channel.").to_string()
    }

    /// Checks a key against the published test feed: None when it opens it, otherwise why not.
    pub fn try_key(key: &[u8]) -> Option<String> {
        let http = Self::client(TimeSpan::from_secs_i(20));
        let bx = match Self::fetch(&http, Self::TEST_FEED) {
            Ok(Some(b)) => b,
            Ok(None) => return Some(L::n("There are no test builds right now.").to_string()),
            Err(e) => return Some(e),
        };
        match TesterKey::open(&bx, key) {
            Ok(plain) => match Self::parse_feed(&String::from_utf8_lossy(&plain)) {
                Ok(_) => None,
                Err(e) => Some(e),
            },
            Err(SealError::Crypto) => Some(Self::key_refused()),
            Err(e) => Some(e.message()),
        }
    }

    /// 1.16.1 Test 4: the feed and its signature in ONE file, "<name>.signed" = the base64 signature, a newline, then
    /// the feed bytes. The site's cache keeps every file up to 10 minutes on its own and ignores the query string, so
    /// right after a publish it could hand out the new feed with the old .sig ("no valid signature"). One file cannot
    /// mix versions. None when the site has no such file (older publishers): the separate pair is used then.
    pub fn split_signed(combined: Option<&[u8]>) -> (Option<Vec<u8>>, Option<String>) {
        let Some(c) = combined else { return (None, None) };
        let Some(nl) = c.iter().position(|b| *b == b'\n') else { return (None, None) };
        if nl == 0 || nl > 512 {
            return (None, None);
        }
        // Encoding.ASCII: a byte above 127 becomes '?'.
        let sig: String = c[..nl].iter().map(|b| if *b < 128 { *b as char } else { '?' }).collect();
        (Some(c[nl + 1..].to_vec()), Some(sig.trim().to_string()))
    }

    /// A feed file with a valid signature; None when it is not there (404). Err when the signature is missing or wrong,
    /// or the site could not be read.
    fn fetch(http: &Client, name: &str) -> Result<Option<Vec<u8>>, String> {
        let q = format!("?t={}", DateTime::utc_now().ticks / TICKS_PER_MINUTE);
        match http.get_bytes(&format!("{}{}.signed{}", Self::FEED_BASE, name, q)) {
            Ok(combined) => {
                let (feed, s1) = Self::split_signed(Some(&combined));
                if let Some(feed) = feed {
                    if Self::verify(Some(&feed), s1.as_deref()) {
                        return Ok(Some(feed));
                    }
                }
            }
            Err(HttpError::Request { .. }) => {}
            Err(e) => return Err(e.message()),
        }
        Self::fetch_pair(http, name, &q)
    }

    fn fetch_pair(http: &Client, name: &str, q: &str) -> Result<Option<Vec<u8>>, String> {
        let body = match http.get_bytes(&format!("{}{}{}", Self::FEED_BASE, name, q)) {
            Ok(b) => b,
            Err(e) if e.status() == Some(404) => return Ok(None),
            Err(e) => return Err(e.message()),
        };
        let sig = match http.get_string(&format!("{}{}.sig{}", Self::FEED_BASE, name, q)) {
            Ok(s) => Some(s),
            Err(e) if e.status() == Some(404) => None,
            Err(e) => return Err(e.message()),
        };
        if !Self::verify(Some(&body), sig.as_deref()) {
            return Err(format!("The update file {} has no valid Domain OS signature: it was ignored.", name));
        }
        Ok(Some(body))
    }

    pub fn sha256_file(path: &Path) -> std::io::Result<String> {
        let mut f = std::fs::File::open(path)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(hex_lower(&h.finalize()))
    }

    pub fn sha256(data: &[u8]) -> String {
        hex_lower(&Sha256::digest(data))
    }

    /// Downloads into memory and checks size and SHA-256 of those bytes; a sealed file is opened with the tester key,
    /// also in memory. Returns the checked (and opened) bytes: exactly what the caller writes. Err on any mismatch.
    /// Progress is reported only when the whole percent changes.
    pub fn download_bytes(f: &UpdateFile, progress: Option<&dyn Fn(f64)>) -> Result<Vec<u8>, String> {
        if f.size <= 0 || f.size > Self::MAX_DOWNLOAD_BYTES {
            return Err("The update entry has no valid size.".into());
        }
        let http = Self::client(TimeSpan::from_minutes(30.0));
        let mut buf = vec![0u8; f.size as usize];
        let url = if f.token.is_some() { Self::resolve_private_asset(f)? } else { f.url.clone() };
        {
            let mut resp = http.send(&url, &[]).map_err(|e| e.message())?;
            if !resp.success() {
                return Err(HttpError::status_error(resp.status, &resp.reason).message());
            }
            if let Some(cl) = resp.content_length {
                if cl as i64 != f.size {
                    return Err("The file is not the size the update says.".into());
                }
            }
            Self::read_all(&mut resp.body, &mut buf, progress)?;
        }
        Self::check_bytes(buf, f)
    }

    /// A release asset of the private test repository: the GitHub API, asked with the token, answers with a redirect to
    /// a short-lived download address. Redirects are followed by hand so the token goes to api.github.com only and never
    /// to the download host; the file itself is then fetched without it.
    fn resolve_private_asset(f: &UpdateFile) -> Result<String, String> {
        if !ci::starts_with(&f.url, Self::GITHUB_API) {
            return Err("A token may only be sent to the GitHub API.".into());
        }
        let api = Client { timeout: TimeSpan::from_minutes(2.0), max_buffer: Self::MAX_FEED_BYTES as u64, follow_redirects: false, user_agent: Self::user_agent() };
        let auth = format!("Bearer {}", f.token.as_deref().unwrap_or(""));
        let resp = api
            .send(&f.url, &[("Authorization", &auth), ("Accept", "application/octet-stream"), ("X-GitHub-Api-Version", "2022-11-28")])
            .map_err(|e| e.message())?;
        if (300..400).contains(&resp.status) {
            if let Some(loc) = resp.location.as_deref().filter(|l| !l.is_empty()) {
                let to = resolve(&f.url, loc);
                if !ci::starts_with(&to, "https://") {
                    return Err("The test download did not point to an https address.".into());
                }
                return Ok(to);
            }
        }
        if matches!(resp.status, 404 | 401 | 403) {
            return Err(
                "The test build could not be fetched: the testers' access to it was changed. Get the newest test build from the testers channel.".into()
            );
        }
        if !resp.success() {
            return Err(HttpError::status_error(resp.status, &resp.reason).message());
        }
        Err("The GitHub API did not give a download address.".into())
    }

    /// Reads exactly buf.len() bytes; more data than that is refused.
    pub fn read_all(src: &mut dyn Read, buf: &mut [u8], progress: Option<&dyn Fn(f64)>) -> Result<(), String> {
        let (mut got, mut shown) = (0usize, -1i64);
        while got < buf.len() {
            let n = match src.read(&mut buf[got..]) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            };
            if n == 0 {
                return Err(format!("The download ended early ({} of {} bytes): nothing was changed.", got, buf.len()));
            }
            got += n;
            let pct = (got as i64 * 100) / buf.len() as i64;
            if pct != shown {
                shown = pct;
                if let Some(p) = progress {
                    p(pct as f64 / 100.0);
                }
            }
        }
        let mut one = [0u8; 1];
        loop {
            match src.read(&mut one) {
                Ok(0) => return Ok(()),
                Ok(_) => return Err("The file is larger than the update says.".into()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// Size and SHA-256 of these exact bytes, then (sealed) the tester key opens them in memory.
    pub fn check_bytes(data: Vec<u8>, f: &UpdateFile) -> Result<Vec<u8>, String> {
        let sum = Self::sha256(&data);
        if data.len() as i64 != f.size || !ci::eq(&sum, &f.sha256) {
            return Err(format!("The downloaded file does not match (size {}, SHA-256 {}…): nothing was changed.", data.len(), &sum[..12]));
        }
        if !f.sealed_ {
            return Ok(data);
        }
        let key = TesterKey::current().ok_or_else(Self::key_refused)?;
        match TesterKey::open(&data, &key) {
            Ok(p) => Ok(p),
            Err(SealError::Crypto) => Err(Self::key_refused()),
            Err(e) => Err(e.message()),
        }
    }

    /// A plain file name (no folder, no ".." and nothing but letters, digits, dot, dash and underscore).
    pub fn safe_file_name(name: &str) -> bool {
        !name.is_empty()
            && name.encode_utf16().count() <= 100
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
            && !name.starts_with('.')
            && !name.contains("..")
    }

    /// Writes checked bytes once into a new folder with a random name under dir (the downloads folder): the folder and
    /// the file are created new (an existing one is never reused), the file is opened without sharing while it is
    /// written. Older download folders are removed first. Returns the file.
    pub fn write_new(data: &[u8], file_name: &str, dir: Option<&Path>) -> Result<PathBuf, UpdError> {
        if !Self::safe_file_name(file_name) {
            return Err(UpdError::InvalidData(format!("Not a plain file name: {}", file_name)));
        }
        let dir = match dir {
            Some(d) => d.to_path_buf(),
            None => Self::dir().map_err(UpdError::Io)?,
        };
        std::fs::create_dir_all(&dir).map_err(io)?;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        let sub = dir.join(hex_lower(&random_bytes(8)));
        if sub.exists() {
            return Err(UpdError::Io(format!("The download folder already exists: {}", sub.display())));
        }
        std::fs::create_dir(&sub).map_err(io)?;
        let path = sub.join(file_name);
        let mut fs = create_new_unshared(&path).map_err(io)?;
        fs.write_all(data).map_err(io)?;
        fs.sync_all().map_err(io)?;
        Ok(path)
    }

    /// A file operation tried up to five times (an antivirus scan can hold a fresh exe for a moment).
    fn retry(mut a: impl FnMut() -> std::io::Result<()>) -> Result<(), UpdError> {
        let mut wait = 200;
        let mut i = 0;
        loop {
            match a() {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if i >= 4 {
                        return Err(io(e));
                    }
                    match RETRY_SLEEP.get() {
                        Some(h) => h(wait),
                        None => std::thread::sleep(std::time::Duration::from_millis(wait as u64)),
                    }
                }
            }
            i += 1;
            wait *= 2;
        }
    }

    /// Installs the new program file: the running exe is renamed to "<exe>.prev" (Windows allows renaming a running
    /// program) and the checked bytes are written as the exe, created new and unshared while written. On any failure the
    /// old file is put back.
    pub fn swap(exe: &Path, content: &[u8]) -> Result<(), UpdError> {
        if content.is_empty() {
            return Err(UpdError::InvalidData("No program file to install.".into()));
        }
        let prev = with_suffix(exe, ".prev");
        if prev.exists() {
            Self::retry(|| std::fs::remove_file(&prev))?;
        }
        Self::retry(|| move_file(exe, &prev))?;
        let r = (|| -> std::io::Result<()> {
            let mut fs = create_new_unshared(exe)?;
            fs.write_all(content)?;
            fs.sync_all()
        })();
        if let Err(e) = r {
            if exe.exists() {
                let _ = std::fs::remove_file(exe);
            }
            Self::retry(|| move_file(&prev, exe))?;
            return Err(io(e));
        }
        Ok(())
    }

    /// Puts the previous program file back (after a failed start of the new one). The program file is never left
    /// missing: each move is retried, and if the previous file cannot take its place the new one goes back.
    pub fn swap_back(exe: &Path) -> Result<(), UpdError> {
        Self::exchange(exe, &with_suffix(exe, ".prev"), &with_suffix(exe, ".bad"))
    }

    /// "Return to the previous version": the previous file becomes the program, the current one "<exe>.next".
    pub fn swap_to_previous(exe: &Path) -> Result<(), UpdError> {
        Self::exchange(exe, &with_suffix(exe, ".prev"), &with_suffix(exe, ".next"))
    }

    /// The previous version did not start after a return to it: the newer one comes back, the previous is kept as
    /// ".prev".
    pub fn undo_swap_to_previous(exe: &Path) -> Result<(), UpdError> {
        Self::exchange(exe, &with_suffix(exe, ".next"), &with_suffix(exe, ".prev"))
    }

    /// The exe is moved to park, the file in from takes its place; on failure the exe comes back from park.
    fn exchange(exe: &Path, from: &Path, park: &Path) -> Result<(), UpdError> {
        if !from.is_file() {
            return Err(UpdError::NotFound("There is no other version to switch to.".into()));
        }
        if park.is_file() {
            Self::retry(|| std::fs::remove_file(park))?;
        }
        if exe.is_file() {
            Self::retry(|| move_file(exe, park))?;
        }
        if let Err(e) = Self::retry(|| move_file(from, exe)) {
            if !exe.is_file() && park.is_file() {
                Self::retry(|| move_file(park, exe))?;
            }
            return Err(e);
        }
        Ok(())
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// `File.Move(a, b)` (no overwrite: an existing target is an error).
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("Cannot create a file when that file already exists: '{}'", to.display())));
    }
    if !from.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("Could not find file '{}'.", from.display())));
    }
    std::fs::rename(from, to)
}

/// `new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None)`.
fn create_new_unshared(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    o.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::MemorySettingsBackend;
    use crate::base::testrand::{iterations, Rand};
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};
    use p256::pkcs8::EncodePublicKey;

    const SIGSAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/sigsample");

    fn sample(name: &str) -> Vec<u8> {
        std::fs::read(Path::new(SIGSAMPLE).join(name)).unwrap()
    }

    fn sign(k: &SigningKey, d: &[u8]) -> String {
        let s: Signature = k.sign(d);
        to_base64(s.to_der().as_bytes())
    }

    fn new_key() -> SigningKey {
        SigningKey::from_slice(&random_bytes(32)).unwrap()
    }

    fn pub_of(k: &SigningKey) -> String {
        to_base64(k.verifying_key().to_public_key_der().unwrap().as_bytes())
    }

    fn t(name: &str, k: &SigningKey) -> TrustedKey {
        TrustedKey::new(&Updater::key_id(Some(&pub_of(k))).unwrap(), name, &pub_of(k))
    }

    fn sha() -> String {
        "a".repeat(64)
    }

    fn f(v: &str) -> UpdateFile {
        UpdateFile::new(v, &format!("https://x/{}", v), &sha(), 10, "", "")
    }

    fn sign_of(x: i32) -> i32 {
        x.signum()
    }

    /// UpdaterTests.VersionOrder (11 cases)
    #[test]
    fn version_order() {
        let cases: [(&str, &str, i32); 11] = [
            ("1.15.0", "1.14.1", 1),
            ("1.15.0", "1.15.0-test1.0", 1),
            ("1.15.0-test1.1", "1.15.0-test1.0", 1),
            ("1.15.0-test2.0", "1.15.0-test1.9", 1),
            ("1.15.0-test1.10", "1.15.0-test1.9", 1),
            ("1.15.0-test1.0", "1.14.1", 1),
            ("1.15.0-test1.0", "1.15.0-test1.0", 0),
            ("v1.15.0", "1.15.0", 0),
            ("1.15", "1.15.0", 0),
            ("garbage", "1.0.0", -1),
            ("1.15.0-beta1", "1.0.0", -1),
        ];
        for (a, b, s) in cases {
            assert_eq!(s, sign_of(UpdateVersion::compare(Some(a), Some(b))), "{} {}", a, b);
            assert_eq!(-s, sign_of(UpdateVersion::compare(Some(b), Some(a))), "{} {}", b, a);
        }
    }

    /// UpdaterTests.Describe
    #[test]
    fn describe() {
        assert_eq!(AppVersion::describe(Some("1.15.0")), "1.15.0");
        assert_eq!(AppVersion::describe(Some("1.15.0-test1.1")), "1.15.0 Test 1.1");
    }

    /// UpdaterTests.StableSeesOnlyStable
    #[test]
    fn stable_sees_only_stable() {
        let stable = UpdateFeed { toolbox: Some(f("1.15.0")), playbook: Some(f("2.2.0")) };
        let test = UpdateFeed { toolbox: Some(f("1.16.0-test1.0")), playbook: None };
        let r = Updater::choose("1.14.1", "stable", Some("2.1.1"), Some(&stable), Some(&test));
        assert_eq!(r.toolbox.unwrap().version, "1.15.0");
        assert_eq!(r.playbook.unwrap().version, "2.2.0");
        assert!(Updater::choose("1.15.0", "stable", Some("2.2.0"), Some(&stable), Some(&test)).toolbox.is_none());
    }

    /// UpdaterTests.TestTakesNewestOfBoth
    #[test]
    fn test_takes_newest_of_both() {
        let stable = UpdateFeed { toolbox: Some(f("1.14.1")), playbook: None };
        let test = UpdateFeed { toolbox: Some(f("1.15.0-test1.1")), playbook: None };
        assert_eq!(Updater::choose("1.15.0-test1.0", "test", None, Some(&stable), Some(&test)).toolbox.unwrap().version, "1.15.0-test1.1");
        // the stable release of the same number wins over every test build
        let stable2 = UpdateFeed { toolbox: Some(f("1.15.0")), playbook: None };
        assert_eq!(Updater::choose("1.15.0-test1.1", "test", None, Some(&stable2), Some(&test)).toolbox.unwrap().version, "1.15.0");
        // nothing newer
        assert!(Updater::choose("1.15.0-test1.1", "test", None, Some(&stable), Some(&test)).toolbox.is_none());
        // no playbook applied on this PC: no playbook offer
        let pb = UpdateFeed { toolbox: None, playbook: Some(f("9.0.0")) };
        assert!(Updater::choose("1.0.0", "test", None, Some(&pb), None).playbook.is_none());
    }

    /// UpdaterTests.FeedParsingRejectsBadEntries
    #[test]
    fn feed_parsing_rejects_bad_entries() {
        let s = sha();
        let fd = Updater::parse_feed(&format!(
            "{{\"toolbox\":{{\"version\":\"1.15.0\",\"url\":\"https://a/b.exe\",\"sha256\":\"{}\",\"size\":5,\"notes_en\":\"n\"}},\"playbook\":{{\"version\":\"2.2.0\",\"url\":\"http://a/b.apbx\",\"sha256\":\"{}\",\"size\":5}}}}",
            s.to_uppercase(),
            s
        ))
        .unwrap();
        assert_eq!(fd.toolbox.as_ref().unwrap().version, "1.15.0");
        assert_eq!(fd.toolbox.as_ref().unwrap().sha256, s);
        assert!(fd.playbook.is_none()); // plain http is refused
        assert!(Updater::parse_feed("{\"toolbox\":{\"version\":\"1.15.0\",\"url\":\"https://a\",\"sha256\":\"abc\",\"size\":5}}").unwrap().toolbox.is_none());
    }

    /// UpdaterTests.InstalledPlaybookFromHistory
    #[test]
    fn installed_playbook_from_history() {
        let p = std::env::temp_dir().join(format!("hist-{}", super::super::util::guid_n()));
        std::fs::write(&p, "2026-11-17 10:00:00  applied  2.1.0\r\n2026-11-18 10:00:00  applied  2.1.1\r\n").unwrap();
        assert_eq!(Updater::installed_playbook(Some(&p)).as_deref(), Some("2.1.1"));
        let mut text = std::fs::read_to_string(&p).unwrap();
        text.push_str("2026-11-18 11:00:00  undone\r\n");
        std::fs::write(&p, text).unwrap();
        assert!(Updater::installed_playbook(Some(&p)).is_none());
        assert!(Updater::installed_playbook(Some(&with_suffix(&p, ".missing"))).is_none());
        let _ = std::fs::remove_file(&p);
    }

    /// UpdaterTests.SwapAndSwapBack
    #[test]
    fn swap_and_swap_back() {
        let d = std::env::temp_dir().join(format!("upd-{}", super::super::util::guid_n()));
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("DomainScheduler.exe");
        std::fs::write(&exe, "old").unwrap();
        Updater::swap(&exe, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(with_suffix(&exe, ".prev")).unwrap(), "old");
        Updater::swap_back(&exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(with_suffix(&exe, ".bad")).unwrap(), "new");
        assert!(!with_suffix(&exe, ".prev").exists());
        // a failing install leaves the old file in place
        assert!(Updater::swap(&exe, &[]).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// UpdaterTests.DownloadRejectsWrongChecksum
    #[test]
    fn download_rejects_wrong_checksum() {
        let p = std::env::temp_dir().join(format!("sha-{}", super::super::util::guid_n()));
        std::fs::write(&p, "abc").unwrap();
        assert_eq!(Updater::sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(&p);
    }

    /// UpdateSignatureTests.OnlyTheRightKeyAndBytesPass
    #[test]
    fn only_the_right_key_and_bytes_pass() {
        let k = new_key();
        let pk = pub_of(&k);
        let data = b"{\"toolbox\":{}}".to_vec();
        let sig = sign(&k, &data);
        assert!(Updater::verify_with(&data, Some(&sig), &pk));
        let mut changed = data.clone();
        changed[3] ^= 1;
        assert!(!Updater::verify_with(&changed, Some(&sig), &pk));
        assert!(!Updater::verify(Some(&data), Some(&sig))); // the built-in Domain OS key did not sign it
        assert!(!Updater::verify_with(&data, None, &pk));
        assert!(!Updater::verify_with(&data, Some("not base64!"), &pk));
    }

    /// UpdateSignatureTests.OpensslSignatureWithTheRealKeyChecks
    #[test]
    fn openssl_signature_with_the_real_key_checks() {
        let data = sample("feed.json");
        let sig = String::from_utf8(sample("feed.json.sig")).unwrap();
        assert!(Updater::verify(Some(&data), Some(&sig)));
    }

    /// UpdateKeyRotationTests.CurrentAndBackupKeyBothVerify
    #[test]
    fn current_and_backup_key_both_verify() {
        let feed = b"{\"toolbox\":{\"version\":\"9.9.9\"}}";
        let (main, backup) = (new_key(), new_key());
        let keys = vec![t("main", &main), t("backup", &backup)];
        assert_eq!(Updater::verify_any(Some(feed), Some(&sign(&main, feed)), &keys, &[]).unwrap().name, "main");
        assert_eq!(Updater::verify_any(Some(feed), Some(&sign(&backup, feed)), &keys, &[]).unwrap().name, "backup");
    }

    /// UpdateKeyRotationTests.UnknownKeyIsRejected
    #[test]
    fn unknown_key_is_rejected() {
        let feed = b"{\"toolbox\":{\"version\":\"9.9.9\"}}";
        let (main, backup, stranger) = (new_key(), new_key(), new_key());
        let keys = vec![t("main", &main), t("backup", &backup)];
        assert!(Updater::verify_any(Some(feed), Some(&sign(&stranger, feed)), &keys, &[]).is_none());
        assert!(!Updater::verify(Some(feed), Some(&sign(&stranger, feed))));
    }

    /// UpdateKeyRotationTests.TamperedFeedIsRejected
    #[test]
    fn tampered_feed_is_rejected() {
        let feed = b"{\"toolbox\":{\"version\":\"9.9.9\"}}".to_vec();
        let (main, backup) = (new_key(), new_key());
        let keys = vec![t("main", &main), t("backup", &backup)];
        for k in [&main, &backup] {
            let sig = sign(k, &feed);
            let mut changed = feed.clone();
            let n = changed.len();
            changed[n - 2] ^= 1;
            assert!(Updater::verify_any(Some(&changed), Some(&sig), &keys, &[]).is_none());
            let mut longer = feed.clone();
            longer.push(b' ');
            assert!(Updater::verify_any(Some(&longer), Some(&sig), &keys, &[]).is_none());
            let mut raw = from_base64(&sig).unwrap();
            let n = raw.len();
            raw[n - 1] ^= 1;
            assert!(Updater::verify_any(Some(&feed), Some(&to_base64(&raw)), &keys, &[]).is_none());
        }
        assert!(Updater::verify_any(Some(&feed), None, &keys, &[]).is_none());
        assert!(Updater::verify_any(Some(&feed), Some(""), &keys, &[]).is_none());
        assert!(Updater::verify_any(Some(&feed), Some("not base64!"), &keys, &[]).is_none());
    }

    /// UpdateKeyRotationTests.RevokedKeyIsRejectedOthersStillWork
    #[test]
    fn revoked_key_is_rejected_others_still_work() {
        let feed = b"{\"toolbox\":{\"version\":\"9.9.9\"}}";
        let (main, backup) = (new_key(), new_key());
        let keys = vec![t("main", &main), t("backup", &backup)];
        let revoked = vec![Updater::key_id(Some(&pub_of(&main))).unwrap().to_uppercase()]; // case does not matter
        assert!(Updater::verify_any(Some(feed), Some(&sign(&main, feed)), &keys, &revoked).is_none());
        assert_eq!(Updater::verify_any(Some(feed), Some(&sign(&backup, feed)), &keys, &revoked).unwrap().name, "backup");
        // the id written next to a key is not trusted: a revoked key listed under another id is still refused
        let disguised = vec![TrustedKey::new("0000000000000000", "main", &pub_of(&main))];
        assert!(Updater::verify_any(Some(feed), Some(&sign(&main, feed)), &disguised, &revoked).is_none());
    }

    /// UpdateKeyRotationTests.BuiltInKeyListIsConsistent
    #[test]
    fn built_in_key_list_is_consistent() {
        let keys = Updater::trusted_keys();
        assert_eq!(Updater::PUBLIC_KEY, keys[0].public_key);
        assert!(keys.iter().any(|k| k.public_key == Updater::BACKUP_PUBLIC_KEY));
        assert_ne!(Updater::PUBLIC_KEY, Updater::BACKUP_PUBLIC_KEY);
        for k in &keys {
            assert_eq!(Updater::key_id(Some(&k.public_key)).unwrap(), k.id); // ids are the real fingerprints
            use p256::pkcs8::DecodePublicKey;
            assert!(p256::ecdsa::VerifyingKey::from_public_key_der(&from_base64(&k.public_key).unwrap()).is_ok()); // a P-256 key
            assert!(!Updater::REVOKED_KEY_IDS.contains(&k.id.as_str()));
        }
        let mut ids: Vec<&str> = keys.iter().map(|k| k.id.as_str()).collect();
        ids.dedup();
        assert_eq!(keys.len(), ids.len());
    }

    /// UpdateKeyRotationTests.FeedSignedTodayStillVerifiesWithTheMainKey
    #[test]
    fn feed_signed_today_still_verifies_with_the_main_key() {
        let data = sample("feed.json");
        let sig = String::from_utf8(sample("feed.json.sig")).unwrap();
        assert!(Updater::verify_with(&data, Some(&sig), Updater::PUBLIC_KEY));
        assert_eq!(Updater::verify_any(Some(&data), Some(&sig), &Updater::trusted_keys(), &Updater::revoked_key_ids()).unwrap().name, "main");
        assert!(Updater::verify_any(Some(&data), Some(&sig), &Updater::trusted_keys(), &[Updater::trusted_keys()[0].id.clone()]).is_none());
    }

    /// RotationDrillTests.LeakedMainKeyIsShutOutByTheBackupSignedRotation (keys made here; the DS_DRILL_DIR variant is
    /// the same with openssl keys)
    #[test]
    fn leaked_main_key_is_shut_out_by_the_backup_signed_rotation() {
        let (main, backup, next) = (new_key(), new_key(), new_key());
        let mut feeds: std::collections::HashMap<&str, (Vec<u8>, String)> = std::collections::HashMap::new();
        let (dm, db, dn);
        if let Ok(dir) = std::env::var("DS_DRILL_DIR") {
            let rd = |n: &str| std::fs::read_to_string(Path::new(&dir).join(n)).unwrap().trim().to_string();
            dm = rd("main.pub.b64");
            db = rd("backup.pub.b64");
            dn = rd("next.pub.b64");
            for fname in ["feed1", "attack", "rotation", "after"] {
                feeds.insert(fname, (std::fs::read(Path::new(&dir).join(format!("{}.json", fname))).unwrap(), rd(&format!("{}.json.sig", fname))));
            }
        } else {
            dm = pub_of(&main);
            db = pub_of(&backup);
            dn = pub_of(&next);
            let mut signed = |n: &'static str, k: &SigningKey, j: &str| {
                feeds.insert(n, (j.as_bytes().to_vec(), sign(k, j.as_bytes())));
            };
            signed("feed1", &main, "{\"phase\":1}");
            signed("attack", &main, "{\"phase\":\"attacker\"}");
            signed("rotation", &backup, "{\"phase\":3}");
            signed("after", &next, "{\"phase\":4}");
        }
        let ok = |keys: &[TrustedKey], revoked: &[String], feed: &str| {
            let (d, s) = &feeds[feed];
            Updater::verify_any(Some(d), Some(s), keys, revoked).is_some()
        };
        let id = |k: &str| Updater::key_id(Some(k)).unwrap();
        let v1 = vec![TrustedKey::new(&id(&dm), "main", &dm), TrustedKey::new(&id(&db), "backup", &db)];
        assert!(ok(&v1, &[], "feed1"));
        assert!(ok(&v1, &[], "attack"));
        assert!(ok(&v1, &[], "rotation"));
        let v2 = vec![TrustedKey::new(&id(&db), "main", &db), TrustedKey::new(&id(&dn), "backup", &dn)];
        let revoked = vec![id(&dm)];
        assert!(!ok(&v2, &revoked, "attack"));
        assert!(!ok(&v2, &revoked, "feed1"));
        assert!(ok(&v2, &revoked, "rotation"));
        assert!(ok(&v2, &revoked, "after"));
        let mut smuggled = v2.clone();
        smuggled.push(TrustedKey::new("0000000000000000", "main", &dm));
        assert!(!ok(&smuggled, &revoked, "attack"));
    }

    /// TesterKeyTests.FormatAndParseRoundTrip
    #[test]
    fn format_and_parse_round_trip() {
        let k = random_bytes(32);
        let t = TesterKey::format(&k);
        assert!(t.starts_with("DOST-"));
        assert_eq!(TesterKey::parse(Some(&t)).unwrap(), k);
        assert_eq!(TesterKey::parse(Some(&format!("  {} ", t.to_lowercase().replace('-', " ")))).unwrap(), k);
        assert!(TesterKey::parse(Some("DOST-AAAA")).is_none());
        assert!(TesterKey::parse(Some("DOST-1111-0000")).is_none());
        assert!(TesterKey::parse(Some("")).is_none());
    }

    /// TesterKeyTests.SealOpenAndWrongKey
    #[test]
    fn seal_open_and_wrong_key() {
        let k = random_bytes(32);
        let mut bx = TesterKey::seal(b"feed", &k).unwrap();
        assert_eq!(TesterKey::open(&bx, &k).unwrap(), b"feed");
        let other = random_bytes(32);
        assert_eq!(TesterKey::open(&bx, &other), Err(SealError::Crypto));
        let n = bx.len();
        bx[n - 1] ^= 1;
        assert_eq!(TesterKey::open(&bx, &k), Err(SealError::Crypto));
    }

    /// TesterKeyTests.OpensWhatThePublishToolSealed
    #[test]
    fn opens_what_the_publish_tool_sealed() {
        let key = TesterKey::parse(Some(&String::from_utf8(sample("sealed.key")).unwrap())).unwrap();
        assert_eq!(key, (0..32u8).collect::<Vec<u8>>());
        assert_eq!(TesterKey::open(&sample("sealed.bin"), &key).unwrap(), b"hello domain os");
    }

    /// TesterKeyTests.SealedFeedEntryIsRead
    #[test]
    fn sealed_feed_entry_is_read() {
        let s = "b".repeat(64);
        let fd = Updater::parse_feed(&format!("{{\"toolbox\":{{\"version\":\"1.15.0-test1.2\",\"sealed\":true,\"url\":\"https://a/tb.bin\",\"sha256\":\"{}\",\"size\":9}}}}", s)).unwrap();
        assert!(fd.toolbox.unwrap().sealed_);
    }

    /// TesterKeyTests.FeedTokenOnlyForSealedEntriesAndTheGitHubApi
    #[test]
    fn feed_token_only_for_sealed_entries_and_the_github_api() {
        let s = "b".repeat(64);
        let tok = format!("github_pat_{}", "A".repeat(40));
        let e = |sealed_on: bool, url: &str, token: &str| {
            format!(
                "{{\"toolbox\":{{\"version\":\"1.16.0-test1.8\",\"sealed\":{},\"url\":\"{}\",\"sha256\":\"{}\",\"size\":9,\"token\":\"{}\"}}}}",
                if sealed_on { "true" } else { "false" },
                url,
                s,
                token
            )
        };
        let api = "https://api.github.com/repos/6kane1/DomainOS-test/releases/assets/1";
        let ok = Updater::parse_feed(&e(true, api, &tok)).unwrap().toolbox.unwrap();
        assert_eq!(ok.token.as_deref(), Some(tok.as_str()));
        assert!(!ok.to_string().contains(&tok));
        assert!(!format!("{:?}", ok).contains(&tok));
        assert!(Updater::parse_feed(&e(false, api, &tok)).unwrap().toolbox.is_none()); // never in the open stable feed
        assert!(Updater::parse_feed(&e(true, "https://evil.example/x", &tok)).unwrap().toolbox.is_none()); // never to another host
        assert!(Updater::parse_feed(&e(true, "https://api.github.com.evil.example/repos/x", &tok)).unwrap().toolbox.is_none());
        assert!(Updater::parse_feed(&e(true, api, "abc\\r\\nX-Evil: 1")).unwrap().toolbox.is_none()); // no header injection
        assert!(Updater::parse_feed(&e(true, api, "short")).unwrap().toolbox.is_none());
        let plain = format!("{{\"toolbox\":{{\"version\":\"1.16.0\",\"sealed\":true,\"url\":\"https://a/tb.bin\",\"sha256\":\"{}\",\"size\":9}}}}", s);
        assert!(Updater::parse_feed(&plain).unwrap().toolbox.unwrap().token.is_none());
    }

    /// TesterKeyTests.ASignedFeedFileSplitsIntoSignatureAndFeed
    #[test]
    fn a_signed_feed_file_splits_into_signature_and_feed() {
        let feed = b"{\"toolbox\":null}\n".to_vec();
        let mut combined = b"MEUCIQ==\n".to_vec();
        combined.extend_from_slice(&feed);
        let (body, sig) = Updater::split_signed(Some(&combined));
        assert_eq!(body.unwrap(), feed);
        assert_eq!(sig.as_deref(), Some("MEUCIQ=="));
        let mut nosig = vec![b'\n'];
        nosig.extend_from_slice(&feed[1..]);
        assert!(Updater::split_signed(Some(&nosig)).0.is_none()); // no signature line
        assert!(Updater::split_signed(None).0.is_none());
    }

    /// Release1160Tests.ChannelNeedsKeyAndChoice
    #[test]
    fn channel_needs_key_and_choice() {
        assert_eq!(AppVersion::channel_for(true, true), "test");
        assert_eq!(AppVersion::channel_for(true, false), "stable"); // key kept, stable releases only
        assert_eq!(AppVersion::channel_for(false, true), "stable");
        assert_eq!(AppVersion::channel_for(false, false), "stable");
    }

    /// SecurityTests.UpdaterRefusesBadVersionsWrongBytesAndPlantedFiles
    #[test]
    fn updater_refuses_bad_versions_wrong_bytes_and_planted_files() {
        assert!(Updater::version_rx("1.15.0"));
        assert!(Updater::version_rx("1.15.0-test1.3"));
        for v in ["1.15.0\\..\\x", "1.15.0 ", "v1.15.0", "1.15.0-test1.3/../a", "1.15", "1.15.0-beta"] {
            assert!(!Updater::version_rx(v), "{}", v);
        }
        let feed = format!("{{\"toolbox\":{{\"version\":\"1.16.0\\\\..\\\\..\\\\evil\",\"url\":\"https://x/y\",\"sha256\":\"{}\",\"size\":5}}}}", sha());
        assert!(Updater::parse_feed(&feed).unwrap().toolbox.is_none());

        let data = b"hello".to_vec();
        let fl = UpdateFile::new("1.16.0", "https://x/y", &Updater::sha256(&data), data.len() as i64, "", "");
        assert_eq!(Updater::check_bytes(data.clone(), &fl).unwrap(), data);
        assert!(Updater::check_bytes(b"hellO".to_vec(), &fl).is_err());
        assert!(Updater::read_all(&mut std::io::Cursor::new(b"hello!".to_vec()), &mut [0u8; 5], None).is_err());
        assert!(Updater::read_all(&mut std::io::Cursor::new(b"hell".to_vec()), &mut [0u8; 5], None).is_err());

        // Downloads: a file planted under the expected name is never reused; each write gets a new random folder.
        let state = std::env::temp_dir().join(format!("ds-state-{}", super::super::util::guid_n()));
        let dl = state.join("Updates");
        std::fs::create_dir_all(dl.join("aaaa")).unwrap();
        std::fs::write(dl.join("Domain-OS-2.1.2.apbx"), "planted").unwrap();
        std::fs::write(dl.join("aaaa").join("Domain-OS-2.1.2.apbx"), "planted").unwrap();
        let a = Updater::write_new(&data, "Domain-OS-2.1.2.apbx", Some(&dl)).unwrap();
        let b = Updater::write_new(&data, "Domain-OS-2.1.2.apbx", Some(&dl)).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "hello");
        fn all_files(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    all_files(&e.path(), out)
                } else {
                    out.push(e.path())
                }
            }
        }
        let mut files = Vec::new();
        all_files(&dl, &mut files);
        assert!(files.iter().all(|x| std::fs::read_to_string(x).unwrap() != "planted"));
        assert!(matches!(Updater::write_new(&data, "..\\..\\DomainScheduler.exe", Some(&dl)), Err(UpdError::InvalidData(_))));
        assert!(matches!(Updater::write_new(&data, "a/b.apbx", Some(&dl)), Err(UpdError::InvalidData(_))));
        let _ = std::fs::remove_dir_all(&state);
    }

    /// SecurityTests.RollbackSwapsThePreviousFileDirectly
    #[test]
    fn rollback_swaps_the_previous_file_directly() {
        let _g = static_hooks();
        RETRY_SLEEP.set(Some(Arc::new(|_| {})));
        let state = std::env::temp_dir().join(format!("ds-state-{}", super::super::util::guid_n()));
        std::fs::create_dir_all(&state).unwrap();
        let exe = state.join("DomainScheduler.exe");
        std::fs::write(&exe, "new").unwrap();
        std::fs::write(with_suffix(&exe, ".prev"), "old").unwrap();
        Updater::swap_to_previous(&exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(with_suffix(&exe, ".next")).unwrap(), "new");
        Updater::undo_swap_to_previous(&exe).unwrap(); // the previous one did not start: the newer one comes back
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(with_suffix(&exe, ".prev")).unwrap(), "old");
        std::fs::remove_file(with_suffix(&exe, ".prev")).unwrap();
        assert!(matches!(Updater::swap_back(&exe), Err(UpdError::NotFound(_))));
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new"); // never left without a program file
        RETRY_SLEEP.set(None);
        let _ = std::fs::remove_dir_all(&state);
    }

    const GOOD_FEED: &str = "{\"toolbox\":{\"version\":\"1.16.0\",\"url\":\"https://github.com/6kane1/DomainOS/releases/download/v1/DomainOS-Toolbox-1.16.0.exe\",\"sha256\":\"f82487cd4d692fc0a8d63b2b510b2ad27a2b1b06ea718551de45e6634dc0a604\",\"size\":74565547,\"notes_en\":\"x\",\"notes_ru\":\"y\"},\"playbook\":{\"version\":\"2.2.4\",\"url\":\"https://github.com/6kane1/DomainOS/releases/download/v2.2.4/Domain-OS-2.2.4.apbx\",\"sha256\":\"56c647a50f3bc15370a380ef829f925e4fbf550c112022e2cbe50c2c25cb064b\",\"size\":82336513}}";

    fn assert_valid_file(f: &Option<UpdateFile>) {
        let Some(f) = f else { return };
        assert!(ci::starts_with(&f.url, "https://"));
        assert_eq!(f.sha256.len(), 64);
        assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(f.size >= 1 && f.size <= Updater::MAX_DOWNLOAD_BYTES);
        assert!(UpdateVersion::parse(Some(&f.version)).is_some());
    }

    /// FuzzTests.TextParsersNeverThrow (the UpdateVersion part)
    #[test]
    fn text_parsers_never_throw() {
        let mut r = Rand::new(1604);
        for _ in 0..iterations() {
            let s = r.text(400);
            UpdateVersion::parse(Some(&s));
            UpdateVersion::compare(Some(&s), Some("1.15.0"));
        }
    }

    /// FuzzTests.FeedParserAcceptsOnlyValidFiles
    #[test]
    fn feed_parser_accepts_only_valid_files() {
        assert!(Updater::parse_feed(GOOD_FEED).unwrap().toolbox.is_some());
        let mut r = Rand::new(1605);
        let good = GOOD_FEED.as_bytes();
        let mut accepted = 0;
        for _ in 0..iterations() {
            let json = if r.next(3) == 0 { r.text(300) } else { String::from_utf8_lossy(&r.mutate(good)).into_owned() };
            let Ok(fd) = Updater::parse_feed(&json) else { continue };
            if fd.toolbox.is_some() || fd.playbook.is_some() {
                accepted += 1;
            }
            assert_valid_file(&fd.toolbox);
            assert_valid_file(&fd.playbook);
        }
        assert!(accepted > 0, "the mutations never produced an accepted feed: the test checks nothing");
    }

    /// FuzzTests.FeedFieldsOfWrongTypesAreRefused
    #[test]
    fn feed_fields_of_wrong_types_are_refused() {
        let a = "a".repeat(64);
        let g = "g".repeat(64);
        let bad = [
            format!("{{\"toolbox\":{{\"version\":1,\"url\":\"https://a/b\",\"sha256\":\"{}\",\"size\":1}}}}", a),
            format!("{{\"toolbox\":{{\"version\":\"1.0.0\",\"url\":\"http://a/b\",\"sha256\":\"{}\",\"size\":1}}}}", a),
            format!("{{\"toolbox\":{{\"version\":\"1.0.0\",\"url\":\"https://a/b\",\"sha256\":\"{}\",\"size\":1}}}}", g),
            format!("{{\"toolbox\":{{\"version\":\"1.0.0\",\"url\":\"https://a/b\",\"sha256\":\"{}\",\"size\":0}}}}", a),
            format!("{{\"toolbox\":{{\"version\":\"1.0.0\",\"url\":\"https://a/b\",\"sha256\":\"{}\",\"size\":99999999999999}}}}", a),
            "{\"toolbox\":[]}".into(),
            "{\"toolbox\":\"x\"}".into(),
            "[]".into(),
            "1".into(),
        ];
        for json in bad {
            let Ok(fd) = Updater::parse_feed(&json) else { continue };
            assert!(fd.toolbox.is_none(), "{}", json);
        }
    }

    /// FuzzTests.TesterKeyParsesOnlyWholeKeysAndRoundTrips
    #[test]
    fn tester_key_parses_only_whole_keys_and_round_trips() {
        let mut r = Rand::new(1606);
        for _ in 0..iterations() {
            let k = TesterKey::parse(Some(&r.text(120)));
            assert!(k.map(|k| k.len() == 32).unwrap_or(true));
            let key = random_bytes(32);
            assert_eq!(TesterKey::parse(Some(&TesterKey::format(&key))).unwrap(), key);
            assert_eq!(TesterKey::parse(Some(&format!(" {} ", TesterKey::format(&key).to_lowercase().replace('-', " ")))).unwrap(), key);
        }
    }

    /// FuzzTests.SealedFilesOpenOnlyUntouchedWithTheRightKey
    #[test]
    fn sealed_files_open_only_untouched_with_the_right_key() {
        let mut r = Rand::new(1607);
        let (key, other) = (random_bytes(32), random_bytes(32));
        for _ in 0..iterations().min(1500) {
            let plain = r.bytes(300);
            let bx = TesterKey::seal(&plain, &key).unwrap();
            assert_eq!(TesterKey::open(&bx, &key).unwrap(), plain);
            assert!(TesterKey::open(&bx, &other).is_err());
            let bad = r.mutate(&bx);
            if bad == bx {
                continue;
            }
            assert!(TesterKey::open(&bad, &key).is_err(), "a changed sealed file was opened");
        }
    }

    /// The feed fetch: the .signed file first, then the separate pair; a 404 is "no feed"; a bad signature is an error;
    /// the sealed test feed opens only with the saved key.
    #[test]
    fn check_reads_signed_feeds_and_the_sealed_test_feed() {
        use super::super::http::{Response, Transport, TRANSPORT_HOOK};
        use std::collections::HashMap;
        struct Site(HashMap<String, Vec<u8>>);
        impl Transport for Site {
            fn send(&self, url: &str, _h: &[(&str, &str)], _f: bool, _t: TimeSpan, ua: &str) -> Result<Response, HttpError> {
                assert!(ua.starts_with("DomainOS-Toolbox/"));
                let name = url.trim_start_matches(Updater::FEED_BASE);
                let name = &name[..name.find('?').unwrap_or(name.len())];
                match self.0.get(name) {
                    Some(b) => Ok(Response { status: 200, reason: "OK".into(), content_length: Some(b.len() as u64), location: None, body: Box::new(std::io::Cursor::new(b.clone())) }),
                    None => Ok(Response { status: 404, reason: "Not Found".into(), content_length: None, location: None, body: Box::new(std::io::empty()) }),
                }
            }
        }
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        AppVersion::override_full(Some("1.16.0"));
        let feed = sample("feed.json");
        let sig = sample("feed.json.sig");
        // only the pair, signed with the real key
        let mut site = HashMap::new();
        site.insert("stable-native.json".to_string(), feed.clone());
        site.insert("stable-native.json.sig".to_string(), sig.clone());
        TRANSPORT_HOOK.set(Some(Arc::new(Site(site.clone()))));
        let r = Updater::check();
        assert!(r.error.is_none(), "{:?}", r.error);
        // the same as one .signed file
        let mut signed = String::from_utf8(sig.clone()).unwrap().trim().as_bytes().to_vec();
        signed.push(b'\n');
        signed.extend_from_slice(&feed);
        let mut site2 = HashMap::new();
        site2.insert("stable-native.json.signed".to_string(), signed);
        TRANSPORT_HOOK.set(Some(Arc::new(Site(site2))));
        assert!(Updater::check().error.is_none());
        // a changed feed: refused
        let mut site3 = site.clone();
        let mut changed = feed.clone();
        changed.push(b' ');
        site3.insert("stable-native.json".to_string(), changed);
        TRANSPORT_HOOK.set(Some(Arc::new(Site(site3))));
        assert!(Updater::check().error.unwrap().contains("has no valid Domain OS signature"));
        // no feed at all: nothing offered, no error
        TRANSPORT_HOOK.set(Some(Arc::new(Site(HashMap::new()))));
        assert_eq!(Updater::check(), UpdateCheck::default());
        assert_eq!(Updater::try_key(&[0u8; 32]).unwrap(), "There are no test builds right now.");
        TRANSPORT_HOOK.set(None);
        AppVersion::override_full(None);
    }

    /// Owner's decision 2026-11-27: the native feeds never share a name with the C# ones (stable.json, test.bin), so a
    /// native release cannot reach a C# client, and the native exe never reads a feed meant for the C# exe.
    #[test]
    fn native_feeds_are_not_the_csharp_feeds() {
        assert_eq!(Updater::STABLE_FEED, "stable-native.json");
        assert_eq!(Updater::TEST_FEED, "test-native.bin");
        for n in [Updater::STABLE_FEED, Updater::TEST_FEED] {
            assert!(n != "stable.json" && n != "test.bin");
            assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
        }
    }
}
