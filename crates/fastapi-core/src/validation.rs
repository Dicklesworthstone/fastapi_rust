//! Validation helper functions for the `#[derive(Validate)]` macro.
//!
//! These functions provide runtime validation for common constraints like
//! email format, URL format, and regex pattern matching.

use crate::error::ValidationErrors;

/// Trait for types that can be validated.
///
/// Types implementing this trait can have their values checked against
/// defined constraints, returning validation errors if any constraints
/// are violated.
///
/// # Deriving
///
/// Use the `#[derive(Validate)]` macro from `fastapi_macros` to automatically
/// implement this trait based on field attributes.
///
/// # Example
///
/// ```ignore
/// use fastapi_macros::Validate;
/// use fastapi_core::validation::Validate;
///
/// #[derive(Validate)]
/// struct CreateUser {
///     #[validate(email)]
///     email: String,
///     #[validate(length(min = 3, max = 50))]
///     username: String,
/// }
///
/// let user = CreateUser {
///     email: "test@example.com".to_string(),
///     username: "testuser".to_string(),
/// };
///
/// assert!(user.validate().is_ok());
/// ```
pub trait Validate {
    /// Validate this value against all defined constraints.
    ///
    /// # Errors
    ///
    /// Returns `ValidationErrors` if any constraints are violated.
    fn validate(&self) -> Result<(), Box<ValidationErrors>>;
}

/// Check if a string is a valid email address.
///
/// Uses a simple but practical regex that matches most real-world emails
/// without being overly permissive.
///
/// # Examples
///
/// ```
/// use fastapi_core::validation::is_valid_email;
///
/// assert!(is_valid_email("user@example.com"));
/// assert!(is_valid_email("user.name+tag@sub.domain.org"));
/// assert!(!is_valid_email("invalid"));
/// assert!(!is_valid_email("@domain.com"));
/// assert!(!is_valid_email("user@"));
/// ```
#[must_use]
pub fn is_valid_email(value: &str) -> bool {
    // Simple but practical email validation
    // Must have exactly one @, non-empty local and domain parts
    let parts: Vec<&str> = value.split('@').collect();
    if parts.len() != 2 {
        return false;
    }

    let local = parts[0];
    let domain = parts[1];

    // Local part must be non-empty and not start/end with dot
    if local.is_empty() || local.starts_with('.') || local.ends_with('.') {
        return false;
    }

    // Domain must have at least one dot and valid characters
    if domain.is_empty() || !domain.contains('.') {
        return false;
    }

    // Check domain format (no leading/trailing dots, no consecutive dots)
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return false;
    }

    // Check for valid characters in local part
    for c in local.chars() {
        if !c.is_alphanumeric() && !".!#$%&'*+/=?^_`{|}~-".contains(c) {
            return false;
        }
    }

    // Check for valid characters in domain
    for c in domain.chars() {
        if !c.is_alphanumeric() && c != '.' && c != '-' {
            return false;
        }
    }

    // Domain parts must not start or end with hyphen
    for part in domain.split('.') {
        if part.is_empty() || part.starts_with('-') || part.ends_with('-') {
            return false;
        }
    }

    true
}

/// Check if a string is a valid URL.
///
/// Validates that the string starts with http:// or https:// and has
/// a valid domain structure.
///
/// # Examples
///
/// ```
/// use fastapi_core::validation::is_valid_url;
///
/// assert!(is_valid_url("https://example.com"));
/// assert!(is_valid_url("http://sub.domain.org/path?query=value"));
/// assert!(!is_valid_url("not-a-url"));
/// assert!(!is_valid_url("ftp://example.com")); // Only http/https
/// ```
#[must_use]
pub fn is_valid_url(value: &str) -> bool {
    // Must start with http:// or https://
    let rest = if let Some(rest) = value.strip_prefix("https://") {
        rest
    } else if let Some(rest) = value.strip_prefix("http://") {
        rest
    } else {
        return false;
    };

    // Must have something after the protocol
    if rest.is_empty() {
        return false;
    }

    // Extract the host part (before any path, query, or fragment)
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("");

    // Remove port if present
    let host = host.split(':').next().unwrap_or("");

    // Host must be non-empty
    if host.is_empty() {
        return false;
    }

    // Check for valid hostname characters
    for c in host.chars() {
        if !c.is_alphanumeric() && c != '.' && c != '-' {
            return false;
        }
    }

    // Must have at least one dot (or be localhost)
    if host != "localhost" && !host.contains('.') {
        return false;
    }

    true
}

/// Check if a string matches a regex pattern.
///
/// This function compiles the regex on each call, which is fine for
/// validation but may not be ideal for hot paths. Consider caching
/// the compiled regex if validating many values.
///
/// # Examples
///
/// ```
/// use fastapi_core::validation::matches_pattern;
///
/// // Simple exact match patterns (no regex features)
/// assert!(matches_pattern("hello", r"^hello$"));
/// assert!(!matches_pattern("world", r"^hello$"));
/// assert!(matches_pattern("anything", "")); // Empty pattern matches all
/// ```
#[must_use]
pub fn matches_pattern(value: &str, pattern: &str) -> bool {
    // Pattern matching without pulling in a full regex engine.
    //
    // This intentionally supports a small, practical subset used by our derive
    // tests and common validation cases:
    // - anchors: ^ and $
    // - literals and '.' wildcard
    // - escapes: \\d (digit)
    // - character classes: [a-z0-9-] with ranges and literals
    // - quantifiers: +, *, ?, and {n}
    //
    // Anything outside this subset returns false (no match).

    // Handle common simple patterns.
    if pattern.is_empty() {
        return true;
    }

    // Fast path: exact match patterns.
    if pattern.starts_with('^') && pattern.ends_with('$') {
        let inner = &pattern[1..pattern.len() - 1];
        // If no special chars, it's an exact match
        if !inner.contains(['[', ']', '*', '+', '?', '\\', '(', ')', '|', '.']) {
            return value == inner;
        }
    }

    let Ok(compiled) = SimpleRegex::compile(pattern) else {
        return false;
    };

    compiled.is_match(value)
}

/// Check if a string is a "reasonably formatted" phone number.
///
/// This is not a full E.164 validator. It is tuned to catch obvious bad input
/// while allowing common human formats (spaces, parens, hyphens, dots).
#[must_use]
pub fn is_valid_phone(value: &str) -> bool {
    let s = value.trim();
    if s.is_empty() {
        return false;
    }

    // '+' is allowed only at the beginning (and at most once).
    if let Some(pos) = s.find('+') {
        if pos != 0 {
            return false;
        }
        if s[1..].contains('+') {
            return false;
        }
    }

    // Can't start or end with a separator (unless starting with '+').
    let first = s.chars().next().unwrap();
    if first != '+' && matches!(first, '-' | '.' | ' ') {
        return false;
    }
    let last = s.chars().last().unwrap();
    if matches!(last, '-' | '.' | ' ') {
        return false;
    }

    let mut digits = 0usize;
    let mut open_parens = 0usize;
    let mut last_sep: Option<char> = None; // only tracks '-' and '.'
    let mut paren_digit_count: usize = 0; // digits since last '('

    for (i, c) in s.chars().enumerate() {
        match c {
            '0'..='9' => {
                digits += 1;
                if open_parens > 0 {
                    paren_digit_count += 1;
                }
                last_sep = None;
            }
            '+' => {
                if i != 0 {
                    return false;
                }
                last_sep = None;
            }
            ' ' => {
                // Multiple spaces are allowed.
                last_sep = None;
            }
            '-' | '.' => {
                // No consecutive '-' or '.' (including mixed like "-.").
                if let Some(prev) = last_sep
                    && matches!(prev, '-' | '.')
                {
                    return false;
                }
                last_sep = Some(c);
            }
            '(' => {
                open_parens += 1;
                paren_digit_count = 0;
                last_sep = None;
            }
            ')' => {
                if open_parens == 0 {
                    return false;
                }
                // Disallow empty parentheses, e.g. "()".
                if paren_digit_count == 0 {
                    return false;
                }
                open_parens -= 1;
                last_sep = None;
            }
            _ => return false, // letters or other punctuation
        }
    }

    if open_parens != 0 {
        return false;
    }

    // Practical minimum length: 10 digits.
    digits >= 10
}

#[derive(Debug, Clone)]
struct SimpleRegex {
    anchored_start: bool,
    anchored_end: bool,
    tokens: Vec<Token>,
}

#[derive(Debug, Clone)]
struct Token {
    atom: Atom,
    min: usize,
    max: Option<usize>, // None means unbounded
}

#[derive(Debug, Clone)]
enum Atom {
    Any,
    Literal(char),
    Digit,
    CharClass(CharClass),
}

#[derive(Debug, Clone)]
struct CharClass {
    parts: Vec<CharClassPart>,
}

#[derive(Debug, Clone)]
enum CharClassPart {
    Single(char),
    Range(char, char),
}

impl CharClass {
    fn matches(&self, c: char) -> bool {
        for part in &self.parts {
            match *part {
                CharClassPart::Single(x) if c == x => return true,
                CharClassPart::Range(a, b) if a <= c && c <= b => return true,
                _ => {}
            }
        }
        false
    }
}

impl Atom {
    fn matches(&self, c: char) -> bool {
        match self {
            Atom::Any => true,
            Atom::Literal(x) => *x == c,
            Atom::Digit => c.is_ascii_digit(),
            Atom::CharClass(cc) => cc.matches(c),
        }
    }
}

impl SimpleRegex {
    fn compile(pattern: &str) -> Result<Self, ()> {
        let mut chars: Vec<char> = pattern.chars().collect();
        let mut anchored_start = false;
        let mut anchored_end = false;

        if chars.first() == Some(&'^') {
            anchored_start = true;
            chars.remove(0);
        }
        if chars.last() == Some(&'$') {
            anchored_end = true;
            chars.pop();
        }

        let mut i = 0usize;
        let mut tokens = Vec::<Token>::new();

        while i < chars.len() {
            let atom = match chars[i] {
                '.' => {
                    i += 1;
                    Atom::Any
                }
                '\\' => {
                    i += 1;
                    if i >= chars.len() {
                        return Err(());
                    }
                    let esc = chars[i];
                    i += 1;
                    match esc {
                        'd' => Atom::Digit,
                        other => Atom::Literal(other),
                    }
                }
                '[' => {
                    i += 1;
                    let (cc, next) = parse_char_class(&chars, i)?;
                    i = next;
                    Atom::CharClass(cc)
                }
                c => {
                    i += 1;
                    Atom::Literal(c)
                }
            };

            // Quantifier (optional).
            let mut min = 1usize;
            let mut max: Option<usize> = Some(1);

            if i < chars.len() {
                match chars[i] {
                    '+' => {
                        min = 1;
                        max = None;
                        i += 1;
                    }
                    '*' => {
                        min = 0;
                        max = None;
                        i += 1;
                    }
                    '?' => {
                        min = 0;
                        max = Some(1);
                        i += 1;
                    }
                    '{' => {
                        i += 1;
                        let (n, next) = parse_braced_number(&chars, i)?;
                        i = next;
                        min = n;
                        max = Some(n);
                    }
                    _ => {}
                }
            }

            tokens.push(Token { atom, min, max });
        }

        Ok(Self {
            anchored_start,
            anchored_end,
            tokens,
        })
    }

    fn is_match(&self, value: &str) -> bool {
        let s: Vec<char> = value.chars().collect();

        if self.anchored_start {
            return self.is_match_at(&s, 0) && (!self.anchored_end || self.matches_end(&s));
        }

        // Unanchored: allow a match starting at any position.
        for start in 0..=s.len() {
            if self.is_match_at(&s, start) && (!self.anchored_end || self.matches_end(&s)) {
                return true;
            }
        }
        false
    }

    fn matches_end(&self, s: &[char]) -> bool {
        // If anchored_end, we require a match that consumes to the end of the string.
        // Our DP matcher returns true only for full consumption from the chosen start.
        // So here we just return true and rely on is_match_at's end-check.
        //
        // This function exists to keep the calling logic clear if we later extend
        // the matcher to support partial-consumption modes.
        let _ = s;
        true
    }

    fn is_match_at(&self, s: &[char], start: usize) -> bool {
        use std::collections::HashMap;

        fn dp(
            tokens: &[Token],
            s: &[char],
            ti: usize,
            si: usize,
            memo: &mut HashMap<(usize, usize), bool>,
        ) -> bool {
            if let Some(&v) = memo.get(&(ti, si)) {
                return v;
            }

            let ans = if ti == tokens.len() {
                si == s.len()
            } else {
                let t = &tokens[ti];
                let remaining = s.len().saturating_sub(si);
                let max_rep = t.max.unwrap_or(remaining).min(remaining);

                // Try all repetition counts in [min, max_rep].
                let mut ok = false;
                for rep in t.min..=max_rep {
                    let mut good = true;
                    for k in 0..rep {
                        if !t.atom.matches(s[si + k]) {
                            good = false;
                            break;
                        }
                    }
                    if good && dp(tokens, s, ti + 1, si + rep, memo) {
                        ok = true;
                        break;
                    }
                }
                ok
            };

            memo.insert((ti, si), ans);
            ans
        }

        // Match must consume to end of string if anchored_end is set; otherwise we accept
        // consumption until token exhaustion (and ignore trailing chars) similar to a
        // plain "find" match. Our tests always anchor with ^...$ so we keep strict
        // behavior when anchored_end is true.
        if self.anchored_end {
            let mut memo = HashMap::new();
            dp(&self.tokens, s, 0, start, &mut memo)
        } else {
            // Non-anchored end: accept any prefix match from start.
            // Implement by checking dp and allowing trailing characters.
            // We do this by running dp against all possible end positions.
            for end in start..=s.len() {
                let slice = &s[..end];
                let mut memo = HashMap::new();
                if dp(&self.tokens, slice, 0, start, &mut memo) {
                    return true;
                }
            }
            false
        }
    }
}

fn parse_char_class(chars: &[char], mut i: usize) -> Result<(CharClass, usize), ()> {
    let mut parts = Vec::<CharClassPart>::new();
    if i >= chars.len() {
        return Err(());
    }

    while i < chars.len() {
        if chars[i] == ']' {
            return Ok((CharClass { parts }, i + 1));
        }

        let first = chars[i];
        i += 1;

        if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] != ']' {
            // Range like a-z.
            let second = chars[i + 1];
            i += 2;
            parts.push(CharClassPart::Range(first, second));
        } else {
            parts.push(CharClassPart::Single(first));
        }
    }

    Err(())
}

fn parse_braced_number(chars: &[char], mut i: usize) -> Result<(usize, usize), ()> {
    let mut n: usize = 0;
    let mut saw_digit = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '}' {
            if !saw_digit {
                return Err(());
            }
            return Ok((n, i + 1));
        }
        if let Some(d) = c.to_digit(10) {
            saw_digit = true;
            n = n
                .checked_mul(10)
                .and_then(|x| x.checked_add(d as usize))
                .ok_or(())?;
            i += 1;
        } else {
            return Err(());
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_emails() {
        assert!(is_valid_email("user@example.com"));
        assert!(is_valid_email("user.name@example.com"));
        assert!(is_valid_email("user+tag@example.com"));
        assert!(is_valid_email("user@sub.domain.org"));
    }

    #[test]
    fn test_invalid_emails() {
        assert!(!is_valid_email(""));
        assert!(!is_valid_email("invalid"));
        assert!(!is_valid_email("@domain.com"));
        assert!(!is_valid_email("user@"));
        assert!(!is_valid_email("user@@domain.com"));
        assert!(!is_valid_email(".user@domain.com"));
        assert!(!is_valid_email("user.@domain.com"));
        assert!(!is_valid_email("user@.domain.com"));
        assert!(!is_valid_email("user@domain.com."));
        assert!(!is_valid_email("user@domain..com"));
    }

    #[test]
    fn test_valid_urls() {
        assert!(is_valid_url("https://example.com"));
        assert!(is_valid_url("http://example.com"));
        assert!(is_valid_url("https://sub.domain.org"));
        assert!(is_valid_url("https://example.com/path"));
        assert!(is_valid_url("https://example.com/path?query=value"));
        assert!(is_valid_url("https://example.com:8080"));
        assert!(is_valid_url("http://localhost"));
    }

    #[test]
    fn test_invalid_urls() {
        assert!(!is_valid_url(""));
        assert!(!is_valid_url("not-a-url"));
        assert!(!is_valid_url("ftp://example.com"));
        assert!(!is_valid_url("https://"));
        assert!(!is_valid_url("http://"));
    }

    #[test]
    fn test_simple_patterns() {
        assert!(matches_pattern("hello", "^hello$"));
        assert!(matches_pattern("test", "^test$"));
        assert!(!matches_pattern("hello", "^world$"));
        assert!(matches_pattern("abc", "abc"));
    }
}
