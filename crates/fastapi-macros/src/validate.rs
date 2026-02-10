//! Validation derive macro implementation.
//!
//! Generates a `validate()` method that checks field-level constraints.
//!
//! # Supported Attributes
//!
//! - `#[validate(length(min = N, max = M))]` - String length constraints
//! - `#[validate(range(min = N, max = M))]` - Numeric range constraints
//! - `#[validate(email)]` - Email format validation
//! - `#[validate(url)]` - URL format validation
//! - `#[validate(regex = "pattern")]` - Regex pattern matching
//! - `#[validate(custom = "function_name")]` - Custom validation function

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Fields, Ident, Lit, Type, parse_macro_input};

/// Validation constraint parsed from attributes.
#[derive(Debug, Default)]
struct FieldValidation {
    /// Minimum string length.
    length_min: Option<usize>,
    /// Maximum string length.
    length_max: Option<usize>,
    /// Minimum numeric value.
    range_min: Option<f64>,
    /// Maximum numeric value.
    range_max: Option<f64>,
    /// Email format validation.
    email: bool,
    /// URL format validation.
    url: bool,
    /// Regex pattern.
    regex: Option<String>,
    /// Custom validation function name.
    custom: Option<String>,
}

pub fn derive_validate_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let validations = match &input.data {
        Data::Struct(data) => generate_struct_validations(&data.fields),
        Data::Enum(_) => {
            return syn::Error::new_spanned(&input, "Validate can only be derived for structs")
                .to_compile_error()
                .into();
        }
        Data::Union(_) => {
            return syn::Error::new_spanned(&input, "Validate cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    let expanded = quote! {
        impl #name {
            /// Validate this value against all field constraints.
            ///
            /// # Errors
            ///
            /// Returns `ValidationErrors` if any constraints are violated.
            pub fn validate(&self) -> Result<(), fastapi_core::ValidationErrors> {
                use fastapi_core::error::{ValidationError, ValidationErrors, LocItem};

                let mut errors = ValidationErrors::new();

                #validations

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }
        }
    };

    TokenStream::from(expanded)
}

fn generate_struct_validations(fields: &Fields) -> TokenStream2 {
    let mut validations = Vec::new();

    match fields {
        Fields::Named(named) => {
            for field in &named.named {
                let field_name = field.ident.as_ref().unwrap();
                let field_name_str = field_name.to_string();
                let field_type = &field.ty;

                let validation = parse_validation_attrs(&field.attrs);
                let field_validations =
                    generate_field_validation(field_name, &field_name_str, field_type, &validation);

                if !field_validations.is_empty() {
                    validations.push(field_validations);
                }
            }
        }
        Fields::Unnamed(_) | Fields::Unit => {
            // Tuple structs and unit structs not supported for now
        }
    }

    quote! {
        #(#validations)*
    }
}

fn parse_validation_attrs(attrs: &[Attribute]) -> FieldValidation {
    let mut validation = FieldValidation::default();

    for attr in attrs {
        if !attr.path().is_ident("validate") {
            continue;
        }

        // Parse using syn 2.x's parse_nested_meta
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("email") {
                validation.email = true;
            } else if meta.path.is_ident("url") {
                validation.url = true;
            } else if meta.path.is_ident("length") {
                // Parse length(min = N, max = M)
                meta.parse_nested_meta(|nested| {
                    if nested.path.is_ident("min") {
                        let value: syn::LitInt = nested.value()?.parse()?;
                        validation.length_min = Some(value.base10_parse()?);
                    } else if nested.path.is_ident("max") {
                        let value: syn::LitInt = nested.value()?.parse()?;
                        validation.length_max = Some(value.base10_parse()?);
                    }
                    Ok(())
                })?;
            } else if meta.path.is_ident("range") {
                // Parse range(min = N, max = M)
                #[allow(clippy::cast_precision_loss)]
                meta.parse_nested_meta(|nested| {
                    if nested.path.is_ident("min") {
                        let value: syn::Lit = nested.value()?.parse()?;
                        match value {
                            Lit::Int(lit_int) => {
                                if let Ok(v) = lit_int.base10_parse::<i64>() {
                                    validation.range_min = Some(v as f64);
                                }
                            }
                            Lit::Float(lit_float) => {
                                if let Ok(v) = lit_float.base10_parse::<f64>() {
                                    validation.range_min = Some(v);
                                }
                            }
                            _ => {}
                        }
                    } else if nested.path.is_ident("max") {
                        let value: syn::Lit = nested.value()?.parse()?;
                        match value {
                            Lit::Int(lit_int) => {
                                if let Ok(v) = lit_int.base10_parse::<i64>() {
                                    validation.range_max = Some(v as f64);
                                }
                            }
                            Lit::Float(lit_float) => {
                                if let Ok(v) = lit_float.base10_parse::<f64>() {
                                    validation.range_max = Some(v);
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                })?;
            } else if meta.path.is_ident("regex") || meta.path.is_ident("pattern") {
                let value: syn::LitStr = meta.value()?.parse()?;
                validation.regex = Some(value.value());
            } else if meta.path.is_ident("custom") {
                let value: syn::LitStr = meta.value()?.parse()?;
                validation.custom = Some(value.value());
            }
            Ok(())
        });
    }

    validation
}

#[allow(clippy::too_many_lines)]
fn generate_field_validation(
    field_name: &Ident,
    field_name_str: &str,
    field_type: &Type,
    validation: &FieldValidation,
) -> TokenStream2 {
    let mut checks = Vec::new();

    // Check if this is an Option<T> type - extract inner type for validation
    let (is_optional, _inner_type) = extract_option_inner(field_type);

    // Generate location path for this field
    let loc = quote! {
        vec![LocItem::field("body"), LocItem::field(#field_name_str)]
    };

    // Length validation (for String types)
    if let Some(min) = validation.length_min {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#field_name {
                    if val.len() < #min {
                        errors.push(ValidationError::string_too_short(#loc, #min)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if self.#field_name.len() < #min {
                    errors.push(ValidationError::string_too_short(#loc, #min)
                        .with_input(serde_json::json!(&self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    if let Some(max) = validation.length_max {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#field_name {
                    if val.len() > #max {
                        errors.push(ValidationError::string_too_long(#loc, #max)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if self.#field_name.len() > #max {
                    errors.push(ValidationError::string_too_long(#loc, #max)
                        .with_input(serde_json::json!(&self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    // Range validation (for numeric types)
    if let Some(min) = validation.range_min {
        let check = if is_optional {
            quote! {
                if let Some(val) = self.#field_name {
                    if (val as f64) < #min {
                        errors.push(ValidationError::greater_than_equal(#loc, #min)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if (self.#field_name as f64) < #min {
                    errors.push(ValidationError::greater_than_equal(#loc, #min)
                        .with_input(serde_json::json!(self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    if let Some(max) = validation.range_max {
        let check = if is_optional {
            quote! {
                if let Some(val) = self.#field_name {
                    if (val as f64) > #max {
                        errors.push(ValidationError::less_than_equal(#loc, #max)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if (self.#field_name as f64) > #max {
                    errors.push(ValidationError::less_than_equal(#loc, #max)
                        .with_input(serde_json::json!(self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    // Email validation
    if validation.email {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#field_name {
                    if !fastapi_core::validation::is_valid_email(val) {
                        errors.push(ValidationError::invalid_email(#loc)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::is_valid_email(&self.#field_name) {
                    errors.push(ValidationError::invalid_email(#loc)
                        .with_input(serde_json::json!(&self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    // URL validation
    if validation.url {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#field_name {
                    if !fastapi_core::validation::is_valid_url(val) {
                        errors.push(ValidationError::invalid_url(#loc)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::is_valid_url(&self.#field_name) {
                    errors.push(ValidationError::invalid_url(#loc)
                        .with_input(serde_json::json!(&self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    // Regex pattern validation
    if let Some(ref pattern) = validation.regex {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#field_name {
                    if !fastapi_core::validation::matches_pattern(val, #pattern) {
                        errors.push(ValidationError::pattern_mismatch(#loc, #pattern)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::matches_pattern(&self.#field_name, #pattern) {
                    errors.push(ValidationError::pattern_mismatch(#loc, #pattern)
                        .with_input(serde_json::json!(&self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    // Custom validation function
    if let Some(ref func_name) = validation.custom {
        let func_ident = format_ident!("{}", func_name);
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#field_name {
                    if let Err(msg) = #func_ident(val) {
                        errors.push(ValidationError::value_error(#loc, msg)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if let Err(msg) = #func_ident(&self.#field_name) {
                    errors.push(ValidationError::value_error(#loc, msg)
                        .with_input(serde_json::json!(&self.#field_name)));
                }
            }
        };
        checks.push(check);
    }

    quote! {
        #(#checks)*
    }
}

/// Check if a type is Option<T> and extract the inner type.
fn extract_option_inner(ty: &Type) -> (bool, Option<&Type>) {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return (true, Some(inner));
                    }
                }
            }
        }
    }
    (false, None)
}
