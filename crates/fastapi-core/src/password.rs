//! Password hashing utilities for secure credential storage.
//!
//! Provides a simple, safe API for hashing and verifying passwords
//! with configurable algorithms and work factors.
//!
//! # Example
//!
//! ```
//! use fastapi_core::password::{PasswordHasher, HashConfig, Algorithm};
//!
//! let hasher = PasswordHasher::new(HashConfig::default());
//! let hash = hasher.hash_password("secret123");
//! assert!(hasher.verify_password("secret123", &hash));
//! assert!(!hasher.verify_password("wrong", &hash));
//! ```

use std::fmt;

/// Supported hashing algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// PBKDF2-HMAC-SHA256 (built-in, no external deps).
    Pbkdf2Sha256,
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pbkdf2Sha256 => write!(f, "pbkdf2-sha256"),
        }
    }
}

impl Algorithm {
    /// Parse algorithm from the prefix of a stored hash string.
    fn from_prefix(s: &str) -> Option<Self> {
        if s.starts_with("$pbkdf2-sha256$") {
            Some(Self::Pbkdf2Sha256)
        } else {
            None
        }
    }
}

/// Configuration for password hashing.
#[derive(Debug, Clone)]
pub struct HashConfig {
    /// The algorithm to use.
    pub algorithm: Algorithm,
    /// Number of iterations (work factor).
    pub iterations: u32,
    /// Salt length in bytes.
    pub salt_len: usize,
    /// Output hash length in bytes.
    pub hash_len: usize,
}

impl Default for HashConfig {
    fn default() -> Self {
        Self {
            algorithm: Algorithm::Pbkdf2Sha256,
            iterations: 100_000,
            salt_len: 16,
            hash_len: 32,
        }
    }
}

impl HashConfig {
    /// Create a new config with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of iterations.
    #[must_use]
    pub fn iterations(mut self, n: u32) -> Self {
        self.iterations = n;
        self
    }

    /// Set the algorithm.
    #[must_use]
    pub fn algorithm(mut self, alg: Algorithm) -> Self {
        self.algorithm = alg;
        self
    }
}

/// Password hasher with configurable algorithm and work factors.
#[derive(Debug, Clone)]
pub struct PasswordHasher {
    config: HashConfig,
}

impl PasswordHasher {
    /// Create a new hasher with the given config.
    pub fn new(config: HashConfig) -> Self {
        Self { config }
    }

    /// Hash a password, returning a string in PHC format.
    ///
    /// Format: `$pbkdf2-sha256$iterations$base64(salt)$base64(hash)`
    pub fn hash_password(&self, password: &str) -> String {
        let salt = generate_salt(self.config.salt_len);
        self.hash_with_salt(password, &salt)
    }

    /// Hash with a specific salt (for testing determinism).
    fn hash_with_salt(&self, password: &str, salt: &[u8]) -> String {
        match self.config.algorithm {
            Algorithm::Pbkdf2Sha256 => {
                let hash = pbkdf2_hmac_sha256(
                    password.as_bytes(),
                    salt,
                    self.config.iterations,
                    self.config.hash_len,
                );
                format!(
                    "$pbkdf2-sha256${}${}${}",
                    self.config.iterations,
                    base64_encode(salt),
                    base64_encode(&hash),
                )
            }
        }
    }

    /// Verify a password against a stored hash.
    ///
    /// Uses timing-safe comparison to prevent timing attacks.
    pub fn verify_password(&self, password: &str, stored_hash: &str) -> bool {
        let Some(algorithm) = Algorithm::from_prefix(stored_hash) else {
            return false;
        };
        match algorithm {
            Algorithm::Pbkdf2Sha256 => self.verify_pbkdf2(password, stored_hash),
        }
    }

    fn verify_pbkdf2(&self, password: &str, stored_hash: &str) -> bool {
        // Parse: $pbkdf2-sha256$iterations$salt$hash
        let parts: Vec<&str> = stored_hash.split('$').collect();
        if parts.len() != 5 {
            return false;
        }
        // parts[0] = "" (before first $), parts[1] = "pbkdf2-sha256",
        // parts[2] = iterations, parts[3] = salt, parts[4] = hash
        let Ok(iterations) = parts[2].parse::<u32>() else {
            return false;
        };
        let Some(salt) = base64_decode(parts[3]) else {
            return false;
        };
        let Some(expected) = base64_decode(parts[4]) else {
            return false;
        };
        let computed = pbkdf2_hmac_sha256(password.as_bytes(), &salt, iterations, expected.len());
        constant_time_eq(&computed, &expected)
    }

    /// Returns the config.
    pub fn config(&self) -> &HashConfig {
        &self.config
    }
}

impl Default for PasswordHasher {
    fn default() -> Self {
        Self::new(HashConfig::default())
    }
}

// ============================================================
// PBKDF2-HMAC-SHA256 implementation (no external deps)
// ============================================================

/// PBKDF2 using HMAC-SHA256 per RFC 2898.
fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(dk_len);
    let blocks_needed = dk_len.div_ceil(32);

    for block_index in 1..=blocks_needed as u32 {
        let mut u = hmac_sha256(password, &[salt, &block_index.to_be_bytes()].concat());
        let mut block = u;
        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for (b, v) in block.iter_mut().zip(u.iter()) {
                *b ^= v;
            }
        }
        result.extend_from_slice(&block);
    }

    result.truncate(dk_len);
    result
}

/// HMAC-SHA256.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let block_size = 64;
    let mut padded_key = [0u8; 64];

    if key.len() > block_size {
        let hashed = sha256(key);
        padded_key[..32].copy_from_slice(&hashed);
    } else {
        padded_key[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= padded_key[i];
        opad[i] ^= padded_key[i];
    }

    let inner = sha256(&[&ipad[..], message].concat());
    sha256(&[&opad[..], &inner[..]].concat())
}

/// SHA-256 (pure Rust, no deps).
#[allow(clippy::many_single_char_names)]
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Padding
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process blocks
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut result = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        result[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

// ============================================================
// Utility functions
// ============================================================

/// Generate random salt bytes from the OS entropy source.
///
/// Uses `/dev/urandom` on Unix systems for cryptographically secure randomness.
/// Falls back to entropy mixing from multiple sources if `/dev/urandom` is unavailable.
fn generate_salt(len: usize) -> Vec<u8> {
    // Try /dev/urandom first (available on Linux, macOS, BSDs)
    if let Ok(bytes) = read_urandom(len) {
        return bytes;
    }

    // Fallback: mix multiple entropy sources via SHA-256
    fallback_salt(len)
}

/// Read `len` bytes from `/dev/urandom`.
fn read_urandom(len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Fallback salt generation using entropy mixing (**NOT cryptographically secure**).
///
/// # Security Warning
///
/// This fallback uses `DefaultHasher` which is NOT a CSPRNG. It is only used
/// when `/dev/urandom` is unavailable, which should never happen on Linux/macOS.
/// The output is fed through SHA-256 for distribution but the entropy source
/// is fundamentally weak (timestamps, PID, thread ID are predictable).
fn fallback_salt(len: usize) -> Vec<u8> {
    eprintln!(
        "WARNING: /dev/urandom unavailable, using weak entropy for salt generation. \
         Password hashes generated with this salt may be vulnerable."
    );
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let time_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let thread_id = format!("{:?}", std::thread::current().id());
    let pid = std::process::id();

    // Generate enough entropy by hashing diverse inputs through SHA-256
    let mut entropy = Vec::with_capacity(len + 64);
    for i in 0u64..=len as u64 / 32 {
        let mut hasher = DefaultHasher::new();
        time_seed.hash(&mut hasher);
        i.hash(&mut hasher);
        pid.hash(&mut hasher);
        thread_id.hash(&mut hasher);
        let h = hasher.finish();
        // Feed hash output into SHA-256 for better distribution
        let block = sha256(&h.to_le_bytes());
        entropy.extend_from_slice(&block);
    }
    entropy.truncate(len);
    entropy
}

/// Timing-safe byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Simple base64 encode (standard alphabet, no padding).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() * 4).div_ceil(3));
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 { u32::from(chunk[1]) } else { 0 };
        let b2 = if chunk.len() > 2 { u32::from(chunk[2]) } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 63) as usize] as char);
        }
    }
    result
}

/// Simple base64 decode (standard alphabet, no padding required).
///
/// Returns `None` if the input contains invalid characters. Padding
/// characters (`=`) are stripped before decoding.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn char_val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a' + 26)),
            b'0'..=b'9' => Some(u32::from(c - b'0' + 52)),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    // Strip padding before decoding
    let s = s.trim_end_matches('=');
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        // Every byte in the chunk must be a valid base64 character
        let mut vals = [0u32; 4];
        let mut count = 0;
        for &b in chunk {
            vals[count] = char_val(b)?; // Return None on invalid char
            count += 1;
        }
        if count >= 2 {
            result.push(((vals[0] << 2) | (vals[1] >> 4)) as u8);
        }
        if count >= 3 {
            result.push((((vals[1] & 0xf) << 4) | (vals[2] >> 2)) as u8);
        }
        if count >= 4 {
            result.push((((vals[2] & 0x3) << 6) | vals[3]) as u8);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let hasher = PasswordHasher::default();
        let hash = hasher.hash_password("secret123");
        assert!(hasher.verify_password("secret123", &hash));
    }

    #[test]
    fn wrong_password_fails() {
        let hasher = PasswordHasher::default();
        let hash = hasher.hash_password("correct");
        assert!(!hasher.verify_password("wrong", &hash));
    }

    #[test]
    fn unique_salts() {
        let hasher = PasswordHasher::default();
        let h1 = hasher.hash_password("same");
        let h2 = hasher.hash_password("same");
        // Different salts produce different hashes
        assert_ne!(h1, h2);
        // But both verify
        assert!(hasher.verify_password("same", &h1));
        assert!(hasher.verify_password("same", &h2));
    }

    #[test]
    fn deterministic_with_known_salt() {
        let hasher = PasswordHasher::new(HashConfig::new().iterations(1000));
        let salt = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let h1 = hasher.hash_with_salt("test", &salt);
        let h2 = hasher.hash_with_salt("test", &salt);
        assert_eq!(h1, h2);
        assert!(hasher.verify_password("test", &h1));
    }

    #[test]
    fn hash_format() {
        let hasher = PasswordHasher::default();
        let hash = hasher.hash_password("password");
        assert!(hash.starts_with("$pbkdf2-sha256$"));
        let parts: Vec<&str> = hash.split('$').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[1], "pbkdf2-sha256");
        assert_eq!(parts[2], "100000");
    }

    #[test]
    fn custom_iterations() {
        let hasher = PasswordHasher::new(HashConfig::new().iterations(10_000));
        let hash = hasher.hash_password("test");
        assert!(hash.contains("$10000$"));
        assert!(hasher.verify_password("test", &hash));
    }

    #[test]
    fn invalid_hash_string() {
        let hasher = PasswordHasher::default();
        assert!(!hasher.verify_password("test", "not-a-hash"));
        assert!(!hasher.verify_password("test", "$unknown$100$salt$hash"));
        assert!(!hasher.verify_password("test", ""));
    }

    #[test]
    fn empty_password() {
        let hasher = PasswordHasher::default();
        let hash = hasher.hash_password("");
        assert!(hasher.verify_password("", &hash));
        assert!(!hasher.verify_password("notempty", &hash));
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb924...
        let result = sha256(b"");
        assert_eq!(result[0], 0xe3);
        assert_eq!(result[1], 0xb0);
        assert_eq!(result[2], 0xc4);
        assert_eq!(result[3], 0x42);
    }

    #[test]
    fn sha256_abc_vector() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223...
        let result = sha256(b"abc");
        assert_eq!(result[0], 0xba);
        assert_eq!(result[1], 0x78);
        assert_eq!(result[2], 0x16);
        assert_eq!(result[3], 0xbf);
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"hello world";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(&decoded, data);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn algorithm_display() {
        assert_eq!(Algorithm::Pbkdf2Sha256.to_string(), "pbkdf2-sha256");
    }
}
