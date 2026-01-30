//! Validation derive macro implementation.
//!
//! This module implements the `#[derive(Validate)]` macro that generates
//! validation code from field attributes.
//!
//! # Supported Validators
//!
//! - `length(min = N, max = M)` - String/Vec length bounds
//! - `range(gt, ge, lt, le)` - Numeric range bounds
//! - `email` - Email format validation
//! - `url` - URL format validation
//! - `regex = "pattern"` - Regex pattern matching
//! - `custom = function` - Custom validation function
//! - `nested` - Validate nested structs
//! - `multiple_of = N` - Divisibility check
//!
//! # Example
//!
//! ```ignore
//! #[derive(Validate)]
//! struct CreateUser {
//!     #[validate(length(min = 1, max = 100))]
//!     name: String,
//!
//!     #[validate(email)]
//!     email: String,
//!
//!     #[validate(range(ge = 0, le = 150))]
//!     age: i32,
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, ExprPath, Field, Fields, Ident, Lit, Meta,
    MetaList, MetaNameValue, Token, parse_macro_input, punctuated::Punctuated,
};

/// Parsed validation rules for a field.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct FieldValidators {
    /// Minimum length for strings/collections.
    length_min: Option<usize>,
    /// Maximum length for strings/collections.
    length_max: Option<usize>,
    /// Greater than (exclusive).
    range_gt: Option<String>,
    /// Greater than or equal (inclusive).
    range_ge: Option<String>,
    /// Less than (exclusive).
    range_lt: Option<String>,
    /// Less than or equal (inclusive).
    range_le: Option<String>,
    /// Email format validation.
    email: bool,
    /// URL format validation.
    url: bool,
    /// Regex pattern.
    regex: Option<String>,
    /// Custom validation function path.
    custom: Option<String>,
    /// Validate nested struct.
    nested: bool,
    /// Multiple of (divisibility).
    multiple_of: Option<String>,
    /// Phone number format validation.
    phone: bool,
    /// String must contain this substring.
    contains: Option<String>,
    /// String must start with this prefix.
    starts_with: Option<String>,
    /// String must end with this suffix.
    ends_with: Option<String>,
}

impl FieldValidators {
    fn is_empty(&self) -> bool {
        self.length_min.is_none()
            && self.length_max.is_none()
            && self.range_gt.is_none()
            && self.range_ge.is_none()
            && self.range_lt.is_none()
            && self.range_le.is_none()
            && !self.email
            && !self.url
            && self.regex.is_none()
            && self.custom.is_none()
            && !self.nested
            && self.multiple_of.is_none()
            && !self.phone
            && self.contains.is_none()
            && self.starts_with.is_none()
            && self.ends_with.is_none()
    }
}

/// Parse validate attributes from a field.
fn parse_validate_attrs(attrs: &[Attribute]) -> FieldValidators {
    let mut validators = FieldValidators::default();

    for attr in attrs {
        if !attr.path().is_ident("validate") {
            continue;
        }

        // Parse #[validate(...)]
        if let Meta::List(meta_list) = &attr.meta {
            parse_validate_list(meta_list, &mut validators);
        }
    }

    validators
}

/// Parse the contents of #[validate(...)].
fn parse_validate_list(meta_list: &MetaList, validators: &mut FieldValidators) {
    // Parse as a punctuated list of nested meta items
    let nested: Result<Punctuated<Meta, Token![,]>, _> =
        meta_list.parse_args_with(Punctuated::parse_terminated);

    if let Ok(items) = nested {
        for item in items {
            match &item {
                // Handle simple validators like `email`, `url`, `nested`
                Meta::Path(path) => {
                    if path.is_ident("email") {
                        validators.email = true;
                    } else if path.is_ident("url") {
                        validators.url = true;
                    } else if path.is_ident("nested") {
                        validators.nested = true;
                    } else if path.is_ident("phone") {
                        validators.phone = true;
                    }
                }
                // Handle key = value like `regex = "pattern"`, `custom = fn_name`
                Meta::NameValue(nv) => {
                    parse_name_value(nv, validators);
                }
                // Handle nested lists like `length(min = 1, max = 100)`, `range(ge = 0)`
                Meta::List(list) => {
                    parse_nested_validator(list, validators);
                }
            }
        }
    }
}

/// Helper to convert ident to string.
fn ident_to_string(ident: &Ident) -> String {
    ident.to_string()
}

/// Parse key = value attributes.
fn parse_name_value(nv: &MetaNameValue, validators: &mut FieldValidators) {
    let name = nv.path.get_ident().map(ident_to_string);

    match name.as_deref() {
        Some("regex") => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                validators.regex = Some(s.value());
            }
        }
        Some("custom") => {
            // custom = validate_fn or custom = "validate_fn"
            match &nv.value {
                Expr::Path(ExprPath { path, .. }) => {
                    validators.custom = Some(path.to_token_stream().to_string());
                }
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => {
                    validators.custom = Some(s.value());
                }
                _ => {}
            }
        }
        Some("multiple_of") => {
            validators.multiple_of = Some(expr_to_string(&nv.value));
        }
        Some("contains") => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                validators.contains = Some(s.value());
            }
        }
        Some("starts_with") => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                validators.starts_with = Some(s.value());
            }
        }
        Some("ends_with") => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                validators.ends_with = Some(s.value());
            }
        }
        _ => {}
    }
}

/// Parse nested validators like `length(...)` or `range(...)`.
fn parse_nested_validator(list: &MetaList, validators: &mut FieldValidators) {
    let name = list.path.get_ident().map(ident_to_string);

    match name.as_deref() {
        Some("length") => {
            // Parse length(min = N, max = M)
            if let Ok(nested) =
                list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            {
                for item in nested {
                    if let Meta::NameValue(nv) = item {
                        let key = nv.path.get_ident().map(ident_to_string);
                        match key.as_deref() {
                            Some("min") => {
                                validators.length_min = expr_to_usize(&nv.value);
                            }
                            Some("max") => {
                                validators.length_max = expr_to_usize(&nv.value);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Some("range") => {
            // Parse range(gt = N, ge = N, lt = N, le = N)
            if let Ok(nested) =
                list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            {
                for item in nested {
                    if let Meta::NameValue(nv) = item {
                        let key = nv.path.get_ident().map(ident_to_string);
                        let value = expr_to_string(&nv.value);
                        match key.as_deref() {
                            Some("gt") => validators.range_gt = Some(value),
                            // Also support min as alias for ge
                            Some("ge" | "min") => validators.range_ge = Some(value),
                            Some("lt") => validators.range_lt = Some(value),
                            // Also support max as alias for le
                            Some("le" | "max") => validators.range_le = Some(value),
                            _ => {}
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Convert expression to usize (for length constraints).
fn expr_to_usize(expr: &Expr) -> Option<usize> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Int(i), ..
    }) = expr
    {
        i.base10_parse().ok()
    } else {
        None
    }
}

/// Convert expression to string representation (for numeric values).
fn expr_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Lit(ExprLit { lit, .. }) => match lit {
            Lit::Int(i) => i.to_string(),
            Lit::Float(f) => f.to_string(),
            Lit::Str(s) => s.value(),
            _ => expr.to_token_stream().to_string(),
        },
        _ => expr.to_token_stream().to_string(),
    }
}

/// Generate validation code for a single field.
///
/// This function is necessarily large because it handles all validator types
/// and generates the corresponding code for each one.
#[allow(clippy::too_many_lines)]
fn generate_field_validation(field: &Field, validators: &FieldValidators) -> TokenStream2 {
    let field_name = field.ident.as_ref().expect("Named field required");
    let field_name_str = field_name.to_string();

    let mut validations = Vec::new();

    // Length validation (for String and collections)
    if let Some(min) = validators.length_min {
        validations.push(quote! {
            if self.#field_name.len() < #min {
                errors.push(
                    fastapi_core::ValidationError::string_too_short(
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                        #min,
                    )
                );
            }
        });
    }

    if let Some(max) = validators.length_max {
        validations.push(quote! {
            if self.#field_name.len() > #max {
                errors.push(
                    fastapi_core::ValidationError::string_too_long(
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                        #max,
                    )
                );
            }
        });
    }

    // Range validation (for numeric types)
    if let Some(ref gt) = validators.range_gt {
        let gt_val: TokenStream2 = gt.parse().unwrap_or_else(|_| quote!(0));
        validations.push(quote! {
            if !(self.#field_name > #gt_val) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::GREATER_THAN_EQUAL,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("Input should be greater than {}", #gt))
                    .with_ctx_value("gt", serde_json::json!(#gt_val))
                );
            }
        });
    }

    if let Some(ref ge) = validators.range_ge {
        let ge_val: TokenStream2 = ge.parse().unwrap_or_else(|_| quote!(0));
        validations.push(quote! {
            if !(self.#field_name >= #ge_val) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::GREATER_THAN_EQUAL,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("Input should be greater than or equal to {}", #ge))
                    .with_ctx_value("ge", serde_json::json!(#ge_val))
                );
            }
        });
    }

    if let Some(ref lt) = validators.range_lt {
        let lt_val: TokenStream2 = lt.parse().unwrap_or_else(|_| quote!(0));
        validations.push(quote! {
            if !(self.#field_name < #lt_val) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::LESS_THAN_EQUAL,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("Input should be less than {}", #lt))
                    .with_ctx_value("lt", serde_json::json!(#lt_val))
                );
            }
        });
    }

    if let Some(ref le) = validators.range_le {
        let le_val: TokenStream2 = le.parse().unwrap_or_else(|_| quote!(0));
        validations.push(quote! {
            if !(self.#field_name <= #le_val) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::LESS_THAN_EQUAL,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("Input should be less than or equal to {}", #le))
                    .with_ctx_value("le", serde_json::json!(#le_val))
                );
            }
        });
    }

    // Email validation (RFC 5321 simplified)
    if validators.email {
        validations.push(quote! {
            {
                let email = &self.#field_name;

                // Helper to validate local part characters
                fn is_valid_local_char(c: char) -> bool {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '+' | '-' | '_' | '!' | '#' | '$' | '%'
                            | '&' | '\'' | '*' | '/' | '=' | '?' | '^' | '`' | '{' | '|' | '}' | '~')
                }

                // Helper to validate domain label (between dots)
                fn is_valid_domain_label(label: &str) -> bool {
                    !label.is_empty()
                        && label.len() <= 63
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                }

                let is_valid = if let Some(at_pos) = email.find('@') {
                    let (local, domain) = email.split_at(at_pos);
                    let domain = &domain[1..]; // Skip the @

                    // Local part validation
                    let local_valid = !local.is_empty()
                        && local.len() <= 64
                        && !local.starts_with('.')
                        && !local.ends_with('.')
                        && !local.contains("..")
                        && local.chars().all(is_valid_local_char);

                    // Domain validation
                    let domain_valid = !domain.is_empty()
                        && domain.len() <= 253
                        && domain.contains('.')
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                        && !domain.contains("..")
                        && domain.split('.').all(is_valid_domain_label);

                    local_valid && domain_valid
                } else {
                    false
                };

                if !is_valid {
                    errors.push(
                        fastapi_core::ValidationError::new(
                            fastapi_core::error::error_types::VALUE_ERROR,
                            vec![
                                fastapi_core::error::LocItem::field("body"),
                                fastapi_core::error::LocItem::field(#field_name_str),
                            ],
                        )
                        .with_msg("value is not a valid email address")
                    );
                }
            }
        });
    }

    // URL validation
    if validators.url {
        validations.push(quote! {
            {
                let url = &self.#field_name;

                // URL validation: protocol + valid host structure
                let is_valid = {
                    let rest = if let Some(r) = url.strip_prefix("https://") {
                        Some(r)
                    } else if let Some(r) = url.strip_prefix("http://") {
                        Some(r)
                    } else {
                        None
                    };

                    match rest {
                        Some(after_protocol) => {
                            // Must have content after protocol
                            !after_protocol.is_empty()
                            // Host part (before first / or ? or #) must be non-empty
                            && {
                                let host_end = after_protocol
                                    .find(|c| c == '/' || c == '?' || c == '#')
                                    .unwrap_or(after_protocol.len());
                                let host = &after_protocol[..host_end];
                                // Host must not be empty and must not be only whitespace
                                !host.is_empty() && !host.chars().all(|c| c.is_whitespace())
                            }
                        }
                        None => false,
                    }
                };
                if !is_valid {
                    errors.push(
                        fastapi_core::ValidationError::new(
                            fastapi_core::error::error_types::URL_TYPE,
                            vec![
                                fastapi_core::error::LocItem::field("body"),
                                fastapi_core::error::LocItem::field(#field_name_str),
                            ],
                        )
                        .with_msg("value is not a valid URL")
                    );
                }
            }
        });
    }

    // Phone validation (bd-apwl)
    if validators.phone {
        validations.push(quote! {
            {
                let phone = &self.#field_name;

                // Phone number validation rules:
                // 1. Allowed characters: digits, +, -, space, (, ), .
                // 2. '+' can only appear at the start (for international format)
                // 3. Parentheses must be balanced and not empty
                // 4. No consecutive separators (except space which can repeat)
                // 5. Must have 7-15 digits (E.164 standard)
                // 6. Must not start/end with separators (except leading +)

                fn is_valid_phone(s: &str) -> bool {
                    let s = s.trim();
                    if s.is_empty() {
                        return false;
                    }

                    let chars: Vec<char> = s.chars().collect();
                    let mut digit_count = 0;
                    let mut paren_depth = 0;
                    let mut prev_was_separator = false;
                    let mut has_content_in_parens = false;

                    for (i, &c) in chars.iter().enumerate() {
                        match c {
                            '0'..='9' => {
                                digit_count += 1;
                                prev_was_separator = false;
                                if paren_depth > 0 {
                                    has_content_in_parens = true;
                                }
                            }
                            '+' => {
                                // '+' only allowed at start
                                if i != 0 {
                                    return false;
                                }
                                prev_was_separator = false;
                            }
                            '-' | '.' => {
                                // No consecutive separators (except space)
                                if prev_was_separator {
                                    return false;
                                }
                                // Can't be first char (after optional +) or last
                                if i == 0 || (i == 1 && chars[0] == '+') || i == chars.len() - 1 {
                                    return false;
                                }
                                prev_was_separator = true;
                            }
                            ' ' => {
                                // Spaces are more permissive but still can't be first/last
                                if i == 0 || (i == 1 && chars[0] == '+') || i == chars.len() - 1 {
                                    return false;
                                }
                                // Multiple spaces are allowed, don't set prev_was_separator
                            }
                            '(' => {
                                // Can't have nested parens, can't be last char
                                if paren_depth > 0 || i == chars.len() - 1 {
                                    return false;
                                }
                                paren_depth += 1;
                                has_content_in_parens = false;
                                prev_was_separator = false;
                            }
                            ')' => {
                                // Must have matching open paren, must have content inside
                                if paren_depth == 0 || !has_content_in_parens {
                                    return false;
                                }
                                paren_depth -= 1;
                                prev_was_separator = false;
                            }
                            _ => {
                                // Invalid character
                                return false;
                            }
                        }
                    }

                    // Must have balanced parens and 7-15 digits (E.164 standard)
                    paren_depth == 0 && (7..=15).contains(&digit_count)
                }

                if !is_valid_phone(phone) {
                    errors.push(
                        fastapi_core::ValidationError::new(
                            fastapi_core::error::error_types::VALUE_ERROR,
                            vec![
                                fastapi_core::error::LocItem::field("body"),
                                fastapi_core::error::LocItem::field(#field_name_str),
                            ],
                        )
                        .with_msg("value is not a valid phone number")
                    );
                }
            }
        });
    }

    // Contains validation
    if let Some(ref substring) = validators.contains {
        validations.push(quote! {
            if !self.#field_name.contains(#substring) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::VALUE_ERROR,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("value must contain {:?}", #substring))
                );
            }
        });
    }

    // Starts_with validation
    if let Some(ref prefix) = validators.starts_with {
        validations.push(quote! {
            if !self.#field_name.starts_with(#prefix) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::VALUE_ERROR,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("value must start with {:?}", #prefix))
                );
            }
        });
    }

    // Ends_with validation
    if let Some(ref suffix) = validators.ends_with {
        validations.push(quote! {
            if !self.#field_name.ends_with(#suffix) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::VALUE_ERROR,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("value must end with {:?}", #suffix))
                );
            }
        });
    }

    // Regex validation with cached compiled regex
    if let Some(ref pattern) = validators.regex {
        // Validate the regex pattern at compile time
        if let Err(e) = regex::Regex::new(pattern) {
            let msg = format!("Invalid regex pattern in #[validate(regex = \"{pattern}\")]: {e}");
            return quote! { compile_error!(#msg); };
        }

        validations.push(quote! {
            {
                // Use OnceLock to compile regex once and cache for all validations
                use std::sync::OnceLock;
                static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
                let re = PATTERN.get_or_init(|| {
                    regex::Regex::new(#pattern)
                        .expect("regex validated at compile time")
                });
                if !re.is_match(&self.#field_name) {
                    errors.push(
                        fastapi_core::ValidationError::new(
                            fastapi_core::error::error_types::STRING_PATTERN_MISMATCH,
                            vec![
                                fastapi_core::error::LocItem::field("body"),
                                fastapi_core::error::LocItem::field(#field_name_str),
                            ],
                        )
                        .with_msg("String should match pattern")
                        .with_ctx_value("pattern", serde_json::json!(#pattern))
                    );
                }
            }
        });
    }

    // Custom validation function
    if let Some(ref func) = validators.custom {
        let func_path: TokenStream2 = func.parse().unwrap_or_else(|_| quote!(validate_field));
        validations.push(quote! {
            if let Err(msg) = #func_path(&self.#field_name) {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::VALUE_ERROR,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(msg)
                );
            }
        });
    }

    // Nested validation
    if validators.nested {
        validations.push(quote! {
            if let Err(nested_errors) = self.#field_name.validate() {
                // Add the field name prefix to all nested errors
                for mut err in nested_errors {
                    let mut new_loc = vec![
                        fastapi_core::error::LocItem::field("body"),
                        fastapi_core::error::LocItem::field(#field_name_str),
                    ];
                    // Skip "body" prefix from nested if present
                    let skip = if err.loc.first().map(|l| l.as_str()) == Some(Some("body")) {
                        1
                    } else {
                        0
                    };
                    new_loc.extend(err.loc.into_iter().skip(skip));
                    err.loc = new_loc;
                    errors.push(err);
                }
            }
        });
    }

    // Multiple of validation
    if let Some(ref divisor) = validators.multiple_of {
        let div_val: TokenStream2 = divisor.parse().unwrap_or_else(|_| quote!(1));
        validations.push(quote! {
            if self.#field_name % #div_val != 0 {
                errors.push(
                    fastapi_core::ValidationError::new(
                        fastapi_core::error::error_types::VALUE_ERROR,
                        vec![
                            fastapi_core::error::LocItem::field("body"),
                            fastapi_core::error::LocItem::field(#field_name_str),
                        ],
                    )
                    .with_msg(format!("Input should be a multiple of {}", #divisor))
                    .with_ctx_value("multiple_of", serde_json::json!(#div_val))
                );
            }
        });
    }

    quote! {
        #(#validations)*
    }
}

/// Main entry point for the derive macro.
pub fn derive_validate_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Only support structs with named fields
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "Validate can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input, "Validate can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    // Generate validation code for each field
    let mut field_validations = Vec::new();
    for field in fields {
        let validators = parse_validate_attrs(&field.attrs);
        if !validators.is_empty() {
            field_validations.push(generate_field_validation(field, &validators));
        }
    }

    let expanded = quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            /// Validate this value according to the `#[validate(...)]` attributes.
            ///
            /// # Errors
            ///
            /// Returns a collection of validation errors if any constraints are violated.
            ///
            /// # Example
            ///
            /// ```ignore
            /// let user = CreateUser { name: "".into(), email: "invalid".into(), age: -1 };
            /// match user.validate() {
            ///     Ok(()) => println!("Valid!"),
            ///     Err(errors) => println!("Errors: {}", errors.len()),
            /// }
            /// ```
            pub fn validate(&self) -> Result<(), fastapi_core::ValidationErrors> {
                let mut errors = Vec::<fastapi_core::ValidationError>::new();

                #(#field_validations)*

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(fastapi_core::ValidationErrors::from_errors(errors))
                }
            }
        }
    };

    TokenStream::from(expanded)
}

#[cfg(test)]
mod tests {
    // Tests would go here but proc-macro crates can't have unit tests
    // that actually expand macros. Use integration tests instead.
}
