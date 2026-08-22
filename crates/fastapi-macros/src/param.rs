//! Parameter metadata attribute parsing.
//!
//! This module provides parsing for `#[param(...)]` attributes on struct fields
//! to generate OpenAPI parameter metadata.
//!
//! This module provides infrastructure for derive macros to parse parameter
//! attributes. It is used by extractor derive implementations.

#![allow(dead_code)] // Infrastructure for downstream extractor derives
#![allow(clippy::cast_precision_loss)] // i64 to f64 for constraint values is acceptable
//!
//! # Supported Attributes
//!
//! - `#[param(title = "...")]` - Display title
//! - `#[param(description = "...")]` - Description (also extracted from doc comments)
//! - `#[param(deprecated)]` - Mark as deprecated
//! - `#[param(exclude)]` - Exclude from OpenAPI schema
//! - `#[param(example = ...)]` - Example value (JSON literal)
//! - `#[param(ge = N)]` - Minimum value (>=)
//! - `#[param(le = N)]` - Maximum value (<=)
//! - `#[param(gt = N)]` - Exclusive minimum (>)
//! - `#[param(lt = N)]` - Exclusive maximum (<)
//! - `#[param(min_length = N)]` - Minimum string length
//! - `#[param(max_length = N)]` - Maximum string length
//! - `#[param(pattern = "...")]` - Regex pattern
//! - `#[param(alias = "...")]` - Alternative name in request (propagates to validation/serialization)
//! - `#[param(validation_alias = "...")]` - Name for validation (overrides alias)
//! - `#[param(serialization_alias = "...")]` - Name for OpenAPI serialization (overrides alias)

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Expr, ExprLit, Lit, Meta, MetaNameValue};

/// Parsed parameter attributes.
#[derive(Default)]
pub struct ParamAttrs {
    pub title: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub exclude: bool,
    pub example: Option<TokenStream2>,
    pub ge: Option<f64>,
    pub le: Option<f64>,
    pub gt: Option<f64>,
    pub lt: Option<f64>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub pattern: Option<String>,
    /// Alternative name used in request (query/header/form).
    /// If set and `validation_alias` is None, this is also used for validation.
    /// If set and `serialization_alias` is None, this is also used for serialization.
    pub alias: Option<String>,
    /// Name used specifically for validation (overrides `alias` for validation).
    pub validation_alias: Option<String>,
    /// Name used specifically for serialization/OpenAPI (overrides `alias` for serialization).
    pub serialization_alias: Option<String>,
}

impl ParamAttrs {
    /// Parse `#[param(...)]` attributes from a list of attributes.
    #[allow(clippy::too_many_lines)]
    pub fn from_attributes(attrs: &[Attribute]) -> Self {
        let mut result = Self::default();

        for attr in attrs {
            if !attr.path().is_ident("param") {
                continue;
            }

            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("title") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Str(s)) = value.parse::<Lit>()
                    {
                        result.title = Some(s.value());
                    }
                } else if meta.path.is_ident("description") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Str(s)) = value.parse::<Lit>()
                    {
                        result.description = Some(s.value());
                    }
                } else if meta.path.is_ident("deprecated") {
                    result.deprecated = true;
                } else if meta.path.is_ident("exclude") {
                    result.exclude = true;
                } else if meta.path.is_ident("example") {
                    if let Ok(value) = meta.value()
                        && let Ok(expr) = value.parse::<syn::Expr>()
                    {
                        result.example = Some(quote! { #expr });
                    }
                } else if meta.path.is_ident("ge") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Float(f)) = value.parse::<Lit>() {
                            result.ge = f.base10_parse().ok();
                        } else if let Ok(Lit::Int(i)) = value.parse::<Lit>() {
                            result.ge = i.base10_parse::<i64>().ok().map(|v| v as f64);
                        }
                    }
                } else if meta.path.is_ident("le") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Float(f)) = value.parse::<Lit>() {
                            result.le = f.base10_parse().ok();
                        } else if let Ok(Lit::Int(i)) = value.parse::<Lit>() {
                            result.le = i.base10_parse::<i64>().ok().map(|v| v as f64);
                        }
                    }
                } else if meta.path.is_ident("gt") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Float(f)) = value.parse::<Lit>() {
                            result.gt = f.base10_parse().ok();
                        } else if let Ok(Lit::Int(i)) = value.parse::<Lit>() {
                            result.gt = i.base10_parse::<i64>().ok().map(|v| v as f64);
                        }
                    }
                } else if meta.path.is_ident("lt") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Float(f)) = value.parse::<Lit>() {
                            result.lt = f.base10_parse().ok();
                        } else if let Ok(Lit::Int(i)) = value.parse::<Lit>() {
                            result.lt = i.base10_parse::<i64>().ok().map(|v| v as f64);
                        }
                    }
                } else if meta.path.is_ident("min_length") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Int(i)) = value.parse::<Lit>()
                    {
                        result.min_length = i.base10_parse().ok();
                    }
                } else if meta.path.is_ident("max_length") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Int(i)) = value.parse::<Lit>()
                    {
                        result.max_length = i.base10_parse().ok();
                    }
                } else if meta.path.is_ident("pattern") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Str(s)) = value.parse::<Lit>()
                    {
                        result.pattern = Some(s.value());
                    }
                } else if meta.path.is_ident("alias") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Str(s)) = value.parse::<Lit>()
                    {
                        result.alias = Some(s.value());
                    }
                } else if meta.path.is_ident("validation_alias") {
                    if let Ok(value) = meta.value()
                        && let Ok(Lit::Str(s)) = value.parse::<Lit>()
                    {
                        result.validation_alias = Some(s.value());
                    }
                } else if meta.path.is_ident("serialization_alias")
                    && let Ok(value) = meta.value()
                    && let Ok(Lit::Str(s)) = value.parse::<Lit>()
                {
                    result.serialization_alias = Some(s.value());
                }
                Ok(())
            });
        }

        // Also check doc comments for description
        if result.description.is_none() {
            result.description = extract_doc_comment(attrs);
        }

        result
    }

    /// Generate code to create a `ParamMeta` from these attributes.
    pub fn to_param_meta_tokens(&self) -> TokenStream2 {
        let title = match &self.title {
            Some(t) => quote! { .title(#t) },
            None => quote! {},
        };

        let description = match &self.description {
            Some(d) => quote! { .description(#d) },
            None => quote! {},
        };

        let deprecated = if self.deprecated {
            quote! { .deprecated() }
        } else {
            quote! {}
        };

        let exclude = if self.exclude {
            quote! { .exclude_from_schema() }
        } else {
            quote! {}
        };

        let example = match &self.example {
            Some(e) => quote! { .example(serde_json::json!(#e)) },
            None => quote! {},
        };

        let ge = match self.ge {
            Some(v) => quote! { .ge(#v) },
            None => quote! {},
        };

        let le = match self.le {
            Some(v) => quote! { .le(#v) },
            None => quote! {},
        };

        let gt = match self.gt {
            Some(v) => quote! { .gt(#v) },
            None => quote! {},
        };

        let lt = match self.lt {
            Some(v) => quote! { .lt(#v) },
            None => quote! {},
        };

        let min_length = match self.min_length {
            Some(v) => quote! { .min_length(#v) },
            None => quote! {},
        };

        let max_length = match self.max_length {
            Some(v) => quote! { .max_length(#v) },
            None => quote! {},
        };

        let pattern = match &self.pattern {
            Some(p) => quote! { .pattern(#p) },
            None => quote! {},
        };

        let alias = match &self.alias {
            Some(a) => quote! { .alias(#a) },
            None => quote! {},
        };

        let validation_alias = match &self.validation_alias {
            Some(a) => quote! { .validation_alias(#a) },
            None => quote! {},
        };

        let serialization_alias = match &self.serialization_alias {
            Some(a) => quote! { .serialization_alias(#a) },
            None => quote! {},
        };

        quote! {
            fastapi_openapi::ParamMeta::new()
                #title
                #description
                #deprecated
                #exclude
                #example
                #ge
                #le
                #gt
                #lt
                #min_length
                #max_length
                #pattern
                #alias
                #validation_alias
                #serialization_alias
        }
    }
}

/// Extract doc comments from attributes.
fn extract_doc_comment(attrs: &[Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            match &attr.meta {
                Meta::NameValue(MetaNameValue {
                    value:
                        Expr::Lit(ExprLit {
                            lit: Lit::Str(s), ..
                        }),
                    ..
                }) => Some(s.value().trim().to_string()),
                _ => None,
            }
        })
        .collect();

    if docs.is_empty() {
        None
    } else {
        Some(docs.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_param_attrs_default() {
        let attrs = ParamAttrs::default();
        assert!(attrs.title.is_none());
        assert!(attrs.description.is_none());
        assert!(!attrs.deprecated);
        assert!(!attrs.exclude);
        assert!(attrs.alias.is_none());
        assert!(attrs.validation_alias.is_none());
        assert!(attrs.serialization_alias.is_none());
    }

    #[test]
    fn test_param_attrs_alias_defaults() {
        let attrs = ParamAttrs::default();

        // All alias fields should be None by default
        assert!(attrs.alias.is_none());
        assert!(attrs.validation_alias.is_none());
        assert!(attrs.serialization_alias.is_none());
    }

    #[test]
    fn test_param_attrs_alias_set() {
        let attrs = ParamAttrs {
            alias: Some("q".to_string()),
            validation_alias: Some("query_param".to_string()),
            serialization_alias: Some("search_query".to_string()),
            ..Default::default()
        };

        assert_eq!(attrs.alias.as_deref(), Some("q"));
        assert_eq!(attrs.validation_alias.as_deref(), Some("query_param"));
        assert_eq!(attrs.serialization_alias.as_deref(), Some("search_query"));
    }

    #[test]
    fn test_to_param_meta_tokens_with_alias() {
        let attrs = ParamAttrs {
            alias: Some("x-custom-token".to_string()),
            ..Default::default()
        };

        let tokens = attrs.to_param_meta_tokens();
        let token_string = tokens.to_string();

        // The tokens should include .alias("x-custom-token")
        assert!(token_string.contains("alias"));
    }
}
