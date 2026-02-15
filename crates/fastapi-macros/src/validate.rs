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
//! - `#[validate(phone)]` - Phone number validation
//! - `#[validate(contains = "substr")]` - Substring containment
//! - `#[validate(starts_with = "prefix")]` - Prefix matching
//! - `#[validate(ends_with = "suffix")]` - Suffix matching
//! - `#[validate(multiple_of = N)]` - Numeric divisibility
//! - `#[validate(nested)]` - Nested struct validation (calls `Validate` on the field)
//! - `#[validate(custom = path::to::fn_name)]` - Custom validation function

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, Index, Lit, Member, Type, parse_macro_input,
};

/// Validation constraint parsed from attributes.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct FieldValidation {
    /// Minimum string length.
    length_min: Option<usize>,
    /// Maximum string length.
    length_max: Option<usize>,
    /// Numeric range: >= bound.
    range_ge: Option<f64>,
    /// Numeric range: > bound.
    range_gt: Option<f64>,
    /// Numeric range: <= bound.
    range_le: Option<f64>,
    /// Numeric range: < bound.
    range_lt: Option<f64>,
    /// Email format validation.
    email: bool,
    /// URL format validation.
    url: bool,
    /// Regex pattern.
    regex: Option<String>,
    /// Phone format validation.
    phone: bool,
    /// Value must contain substring.
    contains: Option<String>,
    /// Value must start with prefix.
    starts_with: Option<String>,
    /// Value must end with suffix.
    ends_with: Option<String>,
    /// Divisibility constraint.
    multiple_of: Option<Expr>,
    /// Nested struct validation (delegates to `Validate`).
    nested: bool,
    /// Custom validation function.
    custom: Option<syn::Path>,
}

pub fn derive_validate_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let validations = match &input.data {
        Data::Struct(data) => match generate_struct_validations(&data.fields) {
            Ok(v) => v,
            Err(e) => return e.to_compile_error().into(),
        },
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
        impl fastapi_core::validation::Validate for #name {
            /// Validate this value against all field constraints.
            ///
            /// # Errors
            ///
            /// Returns `ValidationErrors` if any constraints are violated.
            fn validate(&self) -> Result<(), Box<fastapi_core::ValidationErrors>> {
                use fastapi_core::error::{ValidationError, ValidationErrors, LocItem};

                let mut errors = ValidationErrors::new();

                #validations

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(Box::new(errors))
                }
            }
        }

        impl #name {
            /// Validate this value against all field constraints.
            ///
            /// This is a convenience inherent wrapper so callers do not need to import
            /// the [`fastapi_core::validation::Validate`] trait to call `.validate()`.
            pub fn validate(&self) -> Result<(), Box<fastapi_core::ValidationErrors>> {
                <Self as fastapi_core::validation::Validate>::validate(self)
            }
        }
    };

    TokenStream::from(expanded)
}

fn generate_struct_validations(fields: &Fields) -> Result<TokenStream2, syn::Error> {
    let mut validations = Vec::new();

    match fields {
        Fields::Named(named) => {
            for field in &named.named {
                let field_name = field.ident.as_ref().unwrap();
                let field_name_str = field_name.to_string();
                let field_type = &field.ty;

                let validation = parse_validation_attrs(&field.attrs)?;
                let member = Member::Named(field_name.clone());
                let loc = quote! { vec![LocItem::field("body"), LocItem::field(#field_name_str)] };
                let prefix =
                    quote! { vec![LocItem::field("body"), LocItem::field(#field_name_str)] };
                let field_validations =
                    generate_field_validation(&member, &loc, &prefix, field_type, &validation);

                if !field_validations.is_empty() {
                    validations.push(field_validations);
                }
            }
        }
        Fields::Unnamed(unnamed) => {
            for (idx, field) in unnamed.unnamed.iter().enumerate() {
                let index_u32 = u32::try_from(idx).map_err(|_| {
                    syn::Error::new(Span::call_site(), "tuple struct has too many fields")
                })?;
                let member = Member::Unnamed(Index {
                    index: index_u32,
                    span: Span::call_site(),
                });
                let field_type = &field.ty;

                let validation = parse_validation_attrs(&field.attrs)?;
                let loc = quote! { vec![LocItem::field("body"), LocItem::index(#idx)] };
                let prefix = quote! { vec![LocItem::field("body"), LocItem::index(#idx)] };
                let field_validations =
                    generate_field_validation(&member, &loc, &prefix, field_type, &validation);

                if !field_validations.is_empty() {
                    validations.push(field_validations);
                }
            }
        }
        Fields::Unit => {
            // Unit structs have no fields to validate.
        }
    }

    Ok(quote! { #(#validations)* })
}

fn parse_validation_attrs(attrs: &[Attribute]) -> Result<FieldValidation, syn::Error> {
    let mut validation = FieldValidation::default();

    for attr in attrs {
        if !attr.path().is_ident("validate") {
            continue;
        }

        // Parse using syn 2.x's parse_nested_meta.
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("email") {
                validation.email = true;
            } else if meta.path.is_ident("phone") {
                validation.phone = true;
            } else if meta.path.is_ident("url") {
                validation.url = true;
            } else if meta.path.is_ident("nested") {
                validation.nested = true;
            } else if meta.path.is_ident("length") {
                // Parse length(min = N, max = M)
                meta.parse_nested_meta(|nested| {
                    if nested.path.is_ident("min") {
                        let value: syn::LitInt = nested.value()?.parse()?;
                        validation.length_min = Some(value.base10_parse()?);
                    } else if nested.path.is_ident("max") {
                        let value: syn::LitInt = nested.value()?.parse()?;
                        validation.length_max = Some(value.base10_parse()?);
                    } else {
                        return Err(nested.error("unsupported length validator key"));
                    }
                    Ok(())
                })?;
            } else if meta.path.is_ident("range") {
                // Parse range(ge/gt/le/lt = N). For backwards-compat, also accept min/max.
                meta.parse_nested_meta(|nested| {
                    let expr: Expr = nested.value()?.parse()?;
                    let val = parse_number_expr_to_f64(&expr)?;

                    if nested.path.is_ident("ge") || nested.path.is_ident("min") {
                        validation.range_ge = Some(val);
                    } else if nested.path.is_ident("gt") {
                        validation.range_gt = Some(val);
                    } else if nested.path.is_ident("le") || nested.path.is_ident("max") {
                        validation.range_le = Some(val);
                    } else if nested.path.is_ident("lt") {
                        validation.range_lt = Some(val);
                    } else {
                        return Err(nested.error("unsupported range validator key"));
                    }
                    Ok(())
                })?;
            } else if meta.path.is_ident("regex") || meta.path.is_ident("pattern") {
                let value: syn::LitStr = meta.value()?.parse()?;
                validation.regex = Some(value.value());
            } else if meta.path.is_ident("contains") {
                let value: syn::LitStr = meta.value()?.parse()?;
                validation.contains = Some(value.value());
            } else if meta.path.is_ident("starts_with") {
                let value: syn::LitStr = meta.value()?.parse()?;
                validation.starts_with = Some(value.value());
            } else if meta.path.is_ident("ends_with") {
                let value: syn::LitStr = meta.value()?.parse()?;
                validation.ends_with = Some(value.value());
            } else if meta.path.is_ident("multiple_of") {
                let value: Expr = meta.value()?.parse()?;
                validation.multiple_of = Some(value);
            } else if meta.path.is_ident("custom") {
                let value: syn::Path = meta.value()?.parse()?;
                validation.custom = Some(value);
            } else {
                return Err(meta.error("unsupported validate() attribute"));
            }
            Ok(())
        })?;
    }

    Ok(validation)
}

#[allow(clippy::too_many_lines)]
fn generate_field_validation(
    member: &Member,
    loc: &TokenStream2,
    prefix: &TokenStream2,
    field_type: &Type,
    validation: &FieldValidation,
) -> TokenStream2 {
    let mut checks = Vec::new();

    // Check if this is an Option<T> type - extract inner type for validation
    let (is_optional, _inner_type) = extract_option_inner(field_type);

    // Length validation (for String types)
    if let Some(min) = validation.length_min {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if val.len() < #min {
                        errors.push(ValidationError::string_too_short(#loc, #min)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if self.#member.len() < #min {
                    errors.push(ValidationError::string_too_short(#loc, #min)
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    if let Some(max) = validation.length_max {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if val.len() > #max {
                        errors.push(ValidationError::string_too_long(#loc, #max)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if self.#member.len() > #max {
                    errors.push(ValidationError::string_too_long(#loc, #max)
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // Range validation (for numeric types)
    #[allow(clippy::cast_precision_loss)]
    if let Some(ge) = validation.range_ge {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if (*val as f64) < #ge {
                        errors.push(ValidationError::greater_than_equal(#loc, #ge)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if (self.#member as f64) < #ge {
                    errors.push(ValidationError::greater_than_equal(#loc, #ge)
                        .with_input(serde_json::json!(self.#member)));
                }
            }
        };
        checks.push(check);
    }

    #[allow(clippy::cast_precision_loss)]
    if let Some(gt) = validation.range_gt {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if (*val as f64) <= #gt {
                        errors.push(ValidationError::value_error(#loc, format!("Input should be greater than {}", #gt))
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if (self.#member as f64) <= #gt {
                    errors.push(ValidationError::value_error(#loc, format!("Input should be greater than {}", #gt))
                        .with_input(serde_json::json!(self.#member)));
                }
            }
        };
        checks.push(check);
    }

    #[allow(clippy::cast_precision_loss)]
    if let Some(le) = validation.range_le {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if (*val as f64) > #le {
                        errors.push(ValidationError::less_than_equal(#loc, #le)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if (self.#member as f64) > #le {
                    errors.push(ValidationError::less_than_equal(#loc, #le)
                        .with_input(serde_json::json!(self.#member)));
                }
            }
        };
        checks.push(check);
    }

    #[allow(clippy::cast_precision_loss)]
    if let Some(lt) = validation.range_lt {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if (*val as f64) >= #lt {
                        errors.push(ValidationError::value_error(#loc, format!("Input should be less than {}", #lt))
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if (self.#member as f64) >= #lt {
                    errors.push(ValidationError::value_error(#loc, format!("Input should be less than {}", #lt))
                        .with_input(serde_json::json!(self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // Email validation
    if validation.email {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !fastapi_core::validation::is_valid_email(val) {
                        errors.push(ValidationError::invalid_email(#loc)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::is_valid_email(&self.#member) {
                    errors.push(ValidationError::invalid_email(#loc)
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // URL validation
    if validation.url {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !fastapi_core::validation::is_valid_url(val) {
                        errors.push(ValidationError::invalid_url(#loc)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::is_valid_url(&self.#member) {
                    errors.push(ValidationError::invalid_url(#loc)
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // Phone validation
    if validation.phone {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !fastapi_core::validation::is_valid_phone(val) {
                        errors.push(ValidationError::value_error(#loc, "Invalid phone number")
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::is_valid_phone(&self.#member) {
                    errors.push(ValidationError::value_error(#loc, "Invalid phone number")
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // Regex pattern validation
    if let Some(ref pattern) = validation.regex {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !fastapi_core::validation::matches_pattern(val, #pattern) {
                        errors.push(ValidationError::pattern_mismatch(#loc, #pattern)
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !fastapi_core::validation::matches_pattern(&self.#member, #pattern) {
                    errors.push(ValidationError::pattern_mismatch(#loc, #pattern)
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // contains / starts_with / ends_with
    if let Some(ref needle) = validation.contains {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !val.contains(#needle) {
                        errors.push(ValidationError::value_error(#loc, format!("Input should contain '{}'", #needle))
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !self.#member.contains(#needle) {
                    errors.push(ValidationError::value_error(#loc, format!("Input should contain '{}'", #needle))
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    if let Some(ref prefix) = validation.starts_with {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !val.starts_with(#prefix) {
                        errors.push(ValidationError::value_error(#loc, format!("Input should start with '{}'", #prefix))
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !self.#member.starts_with(#prefix) {
                    errors.push(ValidationError::value_error(#loc, format!("Input should start with '{}'", #prefix))
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    if let Some(ref suffix) = validation.ends_with {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if !val.ends_with(#suffix) {
                        errors.push(ValidationError::value_error(#loc, format!("Input should end with '{}'", #suffix))
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if !self.#member.ends_with(#suffix) {
                    errors.push(ValidationError::value_error(#loc, format!("Input should end with '{}'", #suffix))
                        .with_input(serde_json::json!(&self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // multiple_of
    if let Some(ref multiple_of) = validation.multiple_of {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if *val % (#multiple_of) != 0 {
                        errors.push(ValidationError::value_error(#loc, format!("Input should be a multiple of {}", #multiple_of))
                            .with_input(serde_json::json!(val)));
                    }
                }
            }
        } else {
            quote! {
                if self.#member % (#multiple_of) != 0 {
                    errors.push(ValidationError::value_error(#loc, format!("Input should be a multiple of {}", #multiple_of))
                        .with_input(serde_json::json!(self.#member)));
                }
            }
        };
        checks.push(check);
    }

    // nested
    if validation.nested {
        let check = if is_optional {
            quote! {
                if let Some(ref val) = self.#member {
                    if let Err(err) = fastapi_core::validation::Validate::validate(val) {
                        let mut err = *err;
                        for e in &mut err.errors {
                            if e.loc.first().and_then(LocItem::as_str) == Some("body") {
                                e.loc.remove(0);
                            }
                        }
                        let prefix = #prefix;
                        errors.merge(err.with_loc_prefix(prefix));
                    }
                }
            }
        } else {
            quote! {
                if let Err(err) = fastapi_core::validation::Validate::validate(&self.#member) {
                    let mut err = *err;
                    for e in &mut err.errors {
                        if e.loc.first().and_then(LocItem::as_str) == Some("body") {
                            e.loc.remove(0);
                        }
                    }
                    let prefix = #prefix;
                    errors.merge(err.with_loc_prefix(prefix));
                }
            }
        };
        checks.push(check);
    }

    // Custom validation function
    if let Some(ref func_path) = validation.custom {
        let check = quote! {
            if let Err(msg) = #func_path(&self.#member) {
                errors.push(ValidationError::value_error(#loc, msg)
                    .with_input(serde_json::json!(&self.#member)));
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

fn parse_number_expr_to_f64(expr: &Expr) -> Result<f64, syn::Error> {
    match expr {
        Expr::Lit(expr_lit) => match &expr_lit.lit {
            Lit::Int(i) => {
                let v: i64 = i.base10_parse()?;
                #[allow(clippy::cast_precision_loss)]
                Ok(v as f64)
            }
            Lit::Float(f) => Ok(f.base10_parse()?),
            _ => Err(syn::Error::new_spanned(expr, "expected numeric literal")),
        },
        Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
            let v = parse_number_expr_to_f64(&unary.expr)?;
            Ok(-v)
        }
        _ => Err(syn::Error::new_spanned(expr, "expected numeric literal")),
    }
}
