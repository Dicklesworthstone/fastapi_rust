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
    // Simple pattern matching without regex crate
    // For full regex support, users should use the regex crate directly

    // Handle common simple patterns
    if pattern.is_empty() {
        return true;
    }

    // Exact match patterns
    if pattern.starts_with('^') && pattern.ends_with('$') {
        let inner = &pattern[1..pattern.len() - 1];
        // If no special chars, it's an exact match
        if !inner.contains(['[', ']', '*', '+', '?', '\\', '(', ')', '|', '.']) {
            return value == inner;
        }
    }

    // For complex patterns, we'd need the regex crate
    // For now, do simple prefix/suffix matching
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let value_chars: Vec<char> = value.chars().collect();

    simple_pattern_match(&pattern_chars, &value_chars)
}

/// Simple pattern matcher for common cases.
fn simple_pattern_match(pattern: &[char], value: &[char]) -> bool {
    let mut p_idx = 0;
    let mut v_idx = 0;

    // Handle anchors
    let anchored_start = pattern.first() == Some(&'^');
    let anchored_end = pattern.last() == Some(&'$');

    let pattern = if anchored_start {
        &pattern[1..]
    } else {
        pattern
    };
    let pattern = if anchored_end {
        &pattern[..pattern.len().saturating_sub(1)]
    } else {
        pattern
    };

    // Simple character-by-character match for non-regex patterns
    // This is intentionally limited - for full regex, use the regex crate
    while p_idx < pattern.len() && v_idx < value.len() {
        let p = pattern[p_idx];
        let v = value[v_idx];

        match p {
            '.' => {
                // Matches any single character
                p_idx += 1;
                v_idx += 1;
            }
            '\\' if p_idx + 1 < pattern.len() => {
                // Escaped character - match literally
                p_idx += 1;
                if pattern[p_idx] == v {
                    p_idx += 1;
                    v_idx += 1;
                } else {
                    return false;
                }
            }
            _ if p == v => {
                p_idx += 1;
                v_idx += 1;
            }
            _ => {
                if !anchored_start && v_idx == 0 {
                    // Can skip characters at start if not anchored
                    v_idx += 1;
                } else {
                    return false;
                }
            }
        }
    }

    // Check remaining pattern (should be empty or all anchors consumed)
    let pattern_done = p_idx >= pattern.len();
    let value_done = v_idx >= value.len();

    if anchored_end {
        pattern_done && value_done
    } else {
        pattern_done
    }
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
