//! HTTP Digest authentication (RFC 7616 / RFC 2617).
//!
//! Scope (bd-gl3v):
//! - Parse `Authorization: Digest ...`
//! - Provide response computation + verification helpers
//! - Keep dependencies minimal (no external crypto crates)

use crate::extract::FromRequest;
use crate::password::constant_time_eq;
use crate::response::IntoResponse;
use crate::{Method, Request, RequestContext};
use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Md5,
    Md5Sess,
    Sha256,
    Sha256Sess,
}

impl DigestAlgorithm {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("md5") {
            Some(Self::Md5)
        } else if s.eq_ignore_ascii_case("md5-sess") {
            Some(Self::Md5Sess)
        } else if s.eq_ignore_ascii_case("sha-256") {
            Some(Self::Sha256)
        } else if s.eq_ignore_ascii_case("sha-256-sess") {
            Some(Self::Sha256Sess)
        } else {
            None
        }
    }

    #[must_use]
    pub fn is_sess(self) -> bool {
        matches!(self, Self::Md5Sess | Self::Sha256Sess)
    }

    #[must_use]
    fn response_hex_len(self) -> usize {
        match self {
            Self::Md5 | Self::Md5Sess => 32,
            Self::Sha256 | Self::Sha256Sess => 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestQop {
    Auth,
    AuthInt,
}

impl DigestQop {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("auth") {
            Some(Self::Auth)
        } else if s.eq_ignore_ascii_case("auth-int") {
            Some(Self::AuthInt)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::AuthInt => "auth-int",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestAuth {
    pub username: String,
    pub realm: Option<String>,
    pub nonce: String,
    pub uri: String,
    pub response: String,
    pub opaque: Option<String>,
    pub algorithm: DigestAlgorithm,
    pub qop: Option<DigestQop>,
    pub nc: Option<String>,
    pub cnonce: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DigestAuthError {
    pub kind: DigestAuthErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigestAuthErrorKind {
    MissingHeader,
    InvalidUtf8,
    InvalidScheme,
    InvalidFormat(&'static str),
    MissingField(&'static str),
    UnsupportedQop,
    UnsupportedAlgorithm,
    InvalidNc,
    InvalidResponseHex,
}

impl fmt::Display for DigestAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            DigestAuthErrorKind::MissingHeader => write!(f, "Missing Authorization header"),
            DigestAuthErrorKind::InvalidUtf8 => write!(f, "Invalid Authorization header encoding"),
            DigestAuthErrorKind::InvalidScheme => {
                write!(f, "Authorization header must use Digest scheme")
            }
            DigestAuthErrorKind::InvalidFormat(m) => write!(f, "Invalid Digest header: {m}"),
            DigestAuthErrorKind::MissingField(k) => write!(f, "Digest header missing field: {k}"),
            DigestAuthErrorKind::UnsupportedQop => write!(f, "Unsupported Digest qop"),
            DigestAuthErrorKind::UnsupportedAlgorithm => write!(f, "Unsupported Digest algorithm"),
            DigestAuthErrorKind::InvalidNc => write!(f, "Invalid Digest nc value"),
            DigestAuthErrorKind::InvalidResponseHex => write!(f, "Invalid Digest response value"),
        }
    }
}

impl std::error::Error for DigestAuthError {}

impl IntoResponse for DigestAuthError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let detail = match self.kind {
            DigestAuthErrorKind::MissingHeader => "Not authenticated",
            DigestAuthErrorKind::InvalidUtf8 => "Invalid authentication credentials",
            DigestAuthErrorKind::InvalidScheme => "Invalid authentication credentials",
            DigestAuthErrorKind::InvalidFormat(_) => "Invalid authentication credentials",
            DigestAuthErrorKind::MissingField(_) => "Invalid authentication credentials",
            DigestAuthErrorKind::UnsupportedQop => "Invalid authentication credentials",
            DigestAuthErrorKind::UnsupportedAlgorithm => "Invalid authentication credentials",
            DigestAuthErrorKind::InvalidNc => "Invalid authentication credentials",
            DigestAuthErrorKind::InvalidResponseHex => "Invalid authentication credentials",
        };

        let body = serde_json::json!({ "detail": detail });
        Response::with_status(StatusCode::UNAUTHORIZED)
            .header(
                "www-authenticate",
                b"Digest realm=\"api\", qop=\"auth\", algorithm=MD5".to_vec(),
            )
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for DigestAuth {
    type Error = DigestAuthError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let auth_header = req.headers().get("authorization").ok_or(DigestAuthError {
            kind: DigestAuthErrorKind::MissingHeader,
        })?;
        let auth_str = std::str::from_utf8(auth_header).map_err(|_| DigestAuthError {
            kind: DigestAuthErrorKind::InvalidUtf8,
        })?;
        Self::parse(auth_str)
    }
}

impl DigestAuth {
    /// Parse an `Authorization` header value of the form `Digest ...`.
    pub fn parse(header_value: &str) -> Result<Self, DigestAuthError> {
        let mut it = header_value.splitn(2, char::is_whitespace);
        let scheme = it.next().unwrap_or("");
        if !scheme.eq_ignore_ascii_case("digest") {
            return Err(DigestAuthError {
                kind: DigestAuthErrorKind::InvalidScheme,
            });
        }
        let rest = it.next().unwrap_or("").trim();
        if rest.is_empty() {
            return Err(DigestAuthError {
                kind: DigestAuthErrorKind::InvalidFormat("missing parameters"),
            });
        }

        let params = parse_kv_list(rest).map_err(|m| DigestAuthError {
            kind: DigestAuthErrorKind::InvalidFormat(m),
        })?;

        let username = params
            .get("username")
            .ok_or(DigestAuthError {
                kind: DigestAuthErrorKind::MissingField("username"),
            })?
            .clone();

        let nonce = params
            .get("nonce")
            .ok_or(DigestAuthError {
                kind: DigestAuthErrorKind::MissingField("nonce"),
            })?
            .clone();

        let uri = params
            .get("uri")
            .ok_or(DigestAuthError {
                kind: DigestAuthErrorKind::MissingField("uri"),
            })?
            .clone();

        let response = params
            .get("response")
            .ok_or(DigestAuthError {
                kind: DigestAuthErrorKind::MissingField("response"),
            })?
            .clone();

        let realm = params.get("realm").map(ToString::to_string);
        let opaque = params.get("opaque").map(ToString::to_string);

        let algorithm = match params.get("algorithm") {
            Some(v) => DigestAlgorithm::parse(v).ok_or(DigestAuthError {
                kind: DigestAuthErrorKind::UnsupportedAlgorithm,
            })?,
            None => DigestAlgorithm::Md5,
        };

        if response.len() != algorithm.response_hex_len() || !is_hex(&response) {
            return Err(DigestAuthError {
                kind: DigestAuthErrorKind::InvalidResponseHex,
            });
        }

        let qop = match params.get("qop") {
            Some(v) => Some(DigestQop::parse(v).ok_or(DigestAuthError {
                kind: DigestAuthErrorKind::UnsupportedQop,
            })?),
            None => None,
        };

        let nc = params.get("nc").map(|v| v.to_ascii_lowercase());
        if let Some(nc) = &nc {
            if nc.len() != 8 || !nc.as_bytes().iter().all(u8::is_ascii_hexdigit) {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::InvalidNc,
                });
            }
        }

        let cnonce = params.get("cnonce").map(ToString::to_string);
        if qop.is_some() {
            if nc.is_none() {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::MissingField("nc"),
                });
            }
            if cnonce.is_none() {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::MissingField("cnonce"),
                });
            }
        }

        Ok(Self {
            username,
            realm,
            nonce,
            uri,
            response: response.to_ascii_lowercase(),
            opaque,
            algorithm,
            qop,
            nc,
            cnonce,
        })
    }

    /// Compute the expected `response=` value for this challenge (lower hex).
    ///
    /// Supports:
    /// - algorithms: MD5, MD5-sess, SHA-256, SHA-256-sess
    /// - qop: auth (auth-int is rejected)
    pub fn compute_expected_response(
        &self,
        method: Method,
        realm: &str,
        password: &str,
    ) -> Result<String, DigestAuthError> {
        let qop = match self.qop {
            Some(DigestQop::Auth) => Some("auth"),
            Some(DigestQop::AuthInt) => {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::UnsupportedQop,
                });
            }
            None => None,
        };

        let ha1_0 = hash_hex(
            self.algorithm,
            format_args!("{}:{}:{}", self.username, realm, password),
        );
        let ha1 = if self.algorithm.is_sess() {
            let Some(cnonce) = self.cnonce.as_deref() else {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::MissingField("cnonce"),
                });
            };
            hash_hex(
                self.algorithm,
                format_args!("{}:{}:{}", ha1_0, self.nonce, cnonce),
            )
        } else {
            ha1_0
        };

        let ha2 = hash_hex(
            self.algorithm,
            format_args!("{}:{}", method.as_str(), self.uri),
        );

        let response = if let Some(qop) = qop {
            let Some(nc) = self.nc.as_deref() else {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::MissingField("nc"),
                });
            };
            let Some(cnonce) = self.cnonce.as_deref() else {
                return Err(DigestAuthError {
                    kind: DigestAuthErrorKind::MissingField("cnonce"),
                });
            };
            hash_hex(
                self.algorithm,
                format_args!("{}:{}:{}:{}:{}:{}", ha1, self.nonce, nc, cnonce, qop, ha2),
            )
        } else {
            // RFC 2069 compatibility (no qop).
            hash_hex(
                self.algorithm,
                format_args!("{}:{}:{}", ha1, self.nonce, ha2),
            )
        };

        Ok(response)
    }

    /// Verify `response=` against the expected value (timing-safe).
    pub fn verify(
        &self,
        method: Method,
        realm: &str,
        password: &str,
    ) -> Result<bool, DigestAuthError> {
        let expected = self.compute_expected_response(method, realm, password)?;
        Ok(constant_time_eq(
            expected.as_bytes(),
            self.response.as_bytes(),
        ))
    }

    /// Verify with challenge constraints, including nonce/realm matching.
    pub fn verify_for_challenge(
        &self,
        method: Method,
        realm: &str,
        nonce: &str,
        password: &str,
    ) -> Result<bool, DigestAuthError> {
        if self.nonce != nonce {
            return Ok(false);
        }
        if let Some(header_realm) = self.realm.as_deref() {
            if header_realm != realm {
                return Ok(false);
            }
        }
        self.verify(method, realm, password)
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn parse_kv_list(input: &str) -> Result<std::collections::HashMap<String, String>, &'static str> {
    let mut out = std::collections::HashMap::new();
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        // Skip whitespace + commas.
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        // Key token.
        let key_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
        {
            i += 1;
        }
        if i == key_start {
            return Err("expected key");
        }
        let key = std::str::from_utf8(&bytes[key_start..i]).map_err(|_| "non-utf8 key")?;
        let key = key.to_ascii_lowercase();

        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            return Err("expected '='");
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return Err("expected value");
        }

        let value = if bytes[i] == b'"' {
            i += 1;
            let mut buf = String::new();
            let mut closed = false;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                match b {
                    b'\\' => {
                        if i >= bytes.len() {
                            return Err("invalid escape");
                        }
                        let esc = bytes[i];
                        i += 1;
                        buf.push(esc as char);
                    }
                    b'"' => {
                        closed = true;
                        break;
                    }
                    _ => buf.push(b as char),
                }
            }
            if !closed {
                return Err("unterminated quoted value");
            }
            buf
        } else {
            let v_start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            let raw = std::str::from_utf8(&bytes[v_start..i]).map_err(|_| "non-utf8 value")?;
            raw.trim().to_string()
        };

        out.insert(key, value);
    }

    Ok(out)
}

fn hash_hex(alg: DigestAlgorithm, args: fmt::Arguments<'_>) -> String {
    let s = args.to_string();
    match alg {
        DigestAlgorithm::Md5 | DigestAlgorithm::Md5Sess => {
            let d = md5(s.as_bytes());
            hex_lower(&d)
        }
        DigestAlgorithm::Sha256 | DigestAlgorithm::Sha256Sess => {
            let d = sha256(s.as_bytes());
            hex_lower(&d)
        }
    }
}

fn hex_lower<const N: usize>(bytes: &[u8; N]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(N * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 0x0f) as usize]);
    }
    String::from_utf8(out).expect("hex is ascii")
}

// =============================================================================
// MD5 (minimal, pure Rust)
// =============================================================================

#[allow(clippy::many_single_char_names)]
fn md5(data: &[u8]) -> [u8; 16] {
    // RFC 1321.
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let bit_len = (data.len() as u64) * 8;
    let mut msg = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let j = i * 4;
            *word = u32::from_le_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };

            let tmp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(MD5_K[i]).wrapping_add(m[g])).rotate_left(MD5_S[i]),
            );
            a = tmp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

#[allow(clippy::many_single_char_names)]
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut words = [0u32; 64];
        for (i, word) in words.iter_mut().enumerate().take(16) {
            let offset = i * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for i in 16..64 {
            let sigma0 = words[i - 15].rotate_right(7)
                ^ words[i - 15].rotate_right(18)
                ^ (words[i - 15] >> 3);
            let sigma1 = words[i - 2].rotate_right(17)
                ^ words[i - 2].rotate_right(19)
                ^ (words[i - 2] >> 10);
            words[i] = words[i - 16]
                .wrapping_add(sigma0)
                .wrapping_add(words[i - 7])
                .wrapping_add(sigma1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sigma1)
                .wrapping_add(choose)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(words[i]);
            let sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sigma0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut out = [0u8; 32];
    for (i, value) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    out
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::IntoResponse;

    #[test]
    fn rfc_2617_mufasa_vector_md5_auth() {
        // RFC 2617 example.
        let hdr = concat!(
            "Digest username=\"Mufasa\",",
            " realm=\"testrealm@host.com\",",
            " nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\",",
            " uri=\"/dir/index.html\",",
            " qop=auth,",
            " nc=00000001,",
            " cnonce=\"0a4f113b\",",
            " response=\"6629fae49393a05397450978507c4ef1\",",
            " opaque=\"5ccc069c403ebaf9f0171e9517f40e41\""
        );

        let d = DigestAuth::parse(hdr).expect("parse");
        assert_eq!(d.algorithm, DigestAlgorithm::Md5);
        assert_eq!(d.qop, Some(DigestQop::Auth));

        let ok = d
            .verify(Method::Get, "testrealm@host.com", "Circle Of Life")
            .expect("verify");
        assert!(ok);
    }

    #[test]
    fn md5_known_vector_empty() {
        // MD5("") = d41d8cd98f00b204e9800998ecf8427e
        let d = md5(b"");
        assert_eq!(hex_lower(&d), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn parse_uppercase_response_is_accepted_and_normalized() {
        let hdr = concat!(
            "Digest username=\"Mufasa\",",
            " realm=\"testrealm@host.com\",",
            " nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\",",
            " uri=\"/dir/index.html\",",
            " qop=auth,",
            " nc=00000001,",
            " cnonce=\"0a4f113b\",",
            " response=\"6629FAE49393A05397450978507C4EF1\""
        );
        let d = DigestAuth::parse(hdr).expect("parse");
        assert_eq!(d.response, "6629fae49393a05397450978507c4ef1");
    }

    #[test]
    fn verify_for_challenge_rejects_nonce_mismatch() {
        let hdr = concat!(
            "Digest username=\"Mufasa\",",
            " realm=\"testrealm@host.com\",",
            " nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\",",
            " uri=\"/dir/index.html\",",
            " qop=auth,",
            " nc=00000001,",
            " cnonce=\"0a4f113b\",",
            " response=\"6629fae49393a05397450978507c4ef1\""
        );
        let d = DigestAuth::parse(hdr).expect("parse");
        let ok = d
            .verify_for_challenge(
                Method::Get,
                "testrealm@host.com",
                "different_nonce",
                "Circle Of Life",
            )
            .expect("verify");
        assert!(!ok);
    }

    #[test]
    fn from_request_missing_header_produces_401() {
        let cx = asupersync::Cx::for_testing();
        let ctx = RequestContext::new(cx, 17);
        let mut req = Request::new(Method::Get, "/");
        let err = futures_executor::block_on(DigestAuth::from_request(&ctx, &mut req)).unwrap_err();
        assert_eq!(err.kind, DigestAuthErrorKind::MissingHeader);
        assert_eq!(err.into_response().status().as_u16(), 401);
    }

    #[test]
    fn parse_rejects_unterminated_quoted_value() {
        let hdr = "Digest username=\"Mufasa, nonce=\"abc\", uri=\"/\", response=\"0123456789abcdef0123456789abcdef\"";
        let err = DigestAuth::parse(hdr).expect_err("unterminated quoted values must be rejected");
        assert!(matches!(err.kind, DigestAuthErrorKind::InvalidFormat(_)));
    }
}
