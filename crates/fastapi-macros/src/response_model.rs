use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, LitStr, Token, parse_macro_input};

pub fn derive_response_model_aliases_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;

    let Data::Struct(data_struct) = input.data else {
        return syn::Error::new_spanned(
            ident,
            "ResponseModelAliases can only be derived for structs",
        )
        .to_compile_error()
        .into();
    };

    let rename_all = parse_serde_rename_all(&input.attrs);

    let Fields::Named(fields_named) = data_struct.fields else {
        return syn::Error::new_spanned(
            ident,
            "ResponseModelAliases requires a struct with named fields",
        )
        .to_compile_error()
        .into();
    };

    let mut pairs: Vec<(String, String)> = Vec::new();
    for f in fields_named.named {
        let Some(field_ident) = f.ident else {
            continue;
        };
        let canonical = field_ident.to_string();
        let alias = parse_field_serde_rename(&f.attrs)
            .or_else(|| {
                rename_all
                    .as_deref()
                    .map(|rule| apply_rename_all(rule, &canonical))
            })
            .unwrap_or_else(|| canonical.clone());

        if alias != canonical {
            pairs.push((canonical, alias));
        }
    }

    let const_ident = format_ident!("__FASTAPI_RESPONSE_ALIASES_FOR_{}", ident);
    let pairs_tokens = pairs.iter().map(|(canonical, alias)| {
        quote! { (#canonical, #alias) }
    });

    let expanded = quote! {
        const #const_ident: &[(&'static str, &'static str)] = &[
            #(#pairs_tokens),*
        ];

        impl fastapi_core::ResponseModelAliases for #ident {
            fn response_model_aliases() -> &'static [(&'static str, &'static str)] {
                #const_ident
            }
        }
    };

    expanded.into()
}

fn parse_serde_rename_all(attrs: &[syn::Attribute]) -> Option<String> {
    let mut out: Option<String> = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("rename_all") {
                return Ok(());
            }

            // rename_all = "camelCase"
            if meta.input.peek(Token![=]) {
                let v: LitStr = meta.value()?.parse()?;
                out = Some(v.value());
                return Ok(());
            }

            // rename_all(serialize = "camelCase", ...)
            let mut serialize: Option<String> = None;
            meta.parse_nested_meta(|inner| {
                if inner.path.is_ident("serialize") && inner.input.peek(Token![=]) {
                    let v: LitStr = inner.value()?.parse()?;
                    serialize = Some(v.value());
                }
                Ok(())
            })?;
            if serialize.is_some() {
                out = serialize;
            }
            Ok(())
        });
        if out.is_some() {
            break;
        }
    }
    out
}

fn parse_field_serde_rename(attrs: &[syn::Attribute]) -> Option<String> {
    let mut out: Option<String> = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("rename") {
                return Ok(());
            }

            // rename = "userId"
            if meta.input.peek(Token![=]) {
                let v: LitStr = meta.value()?.parse()?;
                out = Some(v.value());
                return Ok(());
            }

            // rename(serialize = "userId", ...)
            let mut serialize: Option<String> = None;
            meta.parse_nested_meta(|inner| {
                if inner.path.is_ident("serialize") && inner.input.peek(Token![=]) {
                    let v: LitStr = inner.value()?.parse()?;
                    serialize = Some(v.value());
                }
                Ok(())
            })?;
            if serialize.is_some() {
                out = serialize;
            }
            Ok(())
        });
        if out.is_some() {
            break;
        }
    }
    out
}

pub(super) fn apply_rename_all(rule: &str, s: &str) -> String {
    // This mirrors serde's common rename_all rules for struct field names.
    // It's intentionally conservative: it only operates on ASCII identifiers.
    match rule {
        "lowercase" => s.to_ascii_lowercase(),
        "UPPERCASE" | "SCREAMING_SNAKE_CASE" => s.to_ascii_uppercase(),
        "kebab-case" => s.replace('_', "-"),
        "SCREAMING-KEBAB-CASE" => s.replace('_', "-").to_ascii_uppercase(),
        "camelCase" => snake_to_camel(s, false),
        "PascalCase" => snake_to_camel(s, true),
        _ => s.to_string(),
    }
}

fn snake_to_camel(s: &str, pascal: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for part in s.split('_').filter(|p| !p.is_empty()) {
        if first && !pascal {
            out.push_str(&part.to_ascii_lowercase());
        } else {
            let mut chars = part.chars();
            if let Some(c0) = chars.next() {
                out.push(c0.to_ascii_uppercase());
                out.push_str(&chars.as_str().to_ascii_lowercase());
            }
        }
        first = false;
    }
    out
}
