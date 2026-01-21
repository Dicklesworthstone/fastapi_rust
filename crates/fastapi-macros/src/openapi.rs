//! OpenAPI/JSON Schema derive macro implementation.
//!
//! This module provides the `#[derive(JsonSchema)]` macro for generating
//! OpenAPI 3.1 JSON Schema from Rust types.
//!
//! # Supported Types
//!
//! - Primitive types: String, &str, i8-i64, u8-u64, f32, f64, bool
//! - Collections: `Vec<T>`, `Option<T>`, `HashMap<K, V>`
//! - Custom structs (with nested schema generation)
//!
//! # Attributes
//!
//! - `#[schema(title = "...")]` - Set schema title
//! - `#[schema(description = "...")]` - Set schema description
//! - `#[schema(format = "...")]` - Override format (e.g., "email", "date-time")
//! - `#[schema(nullable)]` - Mark field as nullable
//! - `#[schema(skip)]` - Skip field in schema generation

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, GenericArgument, Lit, Meta, MetaNameValue,
    PathArguments, Type, parse_macro_input,
};

/// Schema attributes parsed from `#[schema(...)]`.
#[derive(Default)]
struct SchemaAttrs {
    title: Option<String>,
    description: Option<String>,
    format: Option<String>,
    nullable: bool,
    skip: bool,
}

impl SchemaAttrs {
    fn from_attributes(attrs: &[Attribute]) -> Self {
        let mut result = Self::default();

        for attr in attrs {
            if !attr.path().is_ident("schema") {
                continue;
            }

            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("title") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Str(s)) = value.parse::<Lit>() {
                            result.title = Some(s.value());
                        }
                    }
                } else if meta.path.is_ident("description") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Str(s)) = value.parse::<Lit>() {
                            result.description = Some(s.value());
                        }
                    }
                } else if meta.path.is_ident("format") {
                    if let Ok(value) = meta.value() {
                        if let Ok(Lit::Str(s)) = value.parse::<Lit>() {
                            result.format = Some(s.value());
                        }
                    }
                } else if meta.path.is_ident("nullable") {
                    result.nullable = true;
                } else if meta.path.is_ident("skip") {
                    result.skip = true;
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

/// Information about a struct field.
struct FieldInfo {
    name: String,
    ty: Type,
    attrs: SchemaAttrs,
    is_optional: bool,
}

/// Analyze a type to determine if it's Option<T> and extract T if so.
fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Generate schema code for a type.
#[allow(clippy::too_many_lines)]
fn generate_type_schema(ty: &Type, attrs: &SchemaAttrs) -> TokenStream2 {
    // Check if it's Option<T>
    if let Some(inner) = unwrap_option_type(ty) {
        let inner_schema = generate_type_schema(inner, &SchemaAttrs::default());
        return quote! {
            {
                let mut schema = #inner_schema;
                if let fastapi_openapi::Schema::Primitive(ref mut p) = schema {
                    p.nullable = true;
                }
                schema
            }
        };
    }

    // Handle format override from attributes
    if let Some(ref format) = attrs.format {
        let nullable = attrs.nullable;
        return quote! {
            fastapi_openapi::Schema::Primitive(fastapi_openapi::PrimitiveSchema {
                schema_type: fastapi_openapi::SchemaType::String,
                format: Some(#format.to_string()),
                nullable: #nullable,
            })
        };
    }

    // Check for known types
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident_str = segment.ident.to_string();

            return match ident_str.as_str() {
                // String types
                "String" | "str" => quote! {
                    fastapi_openapi::Schema::string()
                },

                // Integer types
                "i8" => quote! {
                    fastapi_openapi::Schema::integer(Some("int8"))
                },
                "i16" => quote! {
                    fastapi_openapi::Schema::integer(Some("int16"))
                },
                "i32" => quote! {
                    fastapi_openapi::Schema::integer(Some("int32"))
                },
                "i64" | "isize" => quote! {
                    fastapi_openapi::Schema::integer(Some("int64"))
                },
                "u8" => quote! {
                    fastapi_openapi::Schema::integer(Some("uint8"))
                },
                "u16" => quote! {
                    fastapi_openapi::Schema::integer(Some("uint16"))
                },
                "u32" => quote! {
                    fastapi_openapi::Schema::integer(Some("uint32"))
                },
                "u64" | "usize" => quote! {
                    fastapi_openapi::Schema::integer(Some("uint64"))
                },

                // Float types
                "f32" => quote! {
                    fastapi_openapi::Schema::number(Some("float"))
                },
                "f64" => quote! {
                    fastapi_openapi::Schema::number(Some("double"))
                },

                // Boolean
                "bool" => quote! {
                    fastapi_openapi::Schema::boolean()
                },

                // Vec<T>
                "Vec" => {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(GenericArgument::Type(inner)) = args.args.first() {
                            let inner_schema = generate_type_schema(inner, &SchemaAttrs::default());
                            return quote! {
                                fastapi_openapi::Schema::array(#inner_schema)
                            };
                        }
                    }
                    // Fallback for Vec without type args
                    quote! {
                        fastapi_openapi::Schema::array(fastapi_openapi::Schema::Boolean(true))
                    }
                }

                // HashMap<K, V>
                "HashMap" | "BTreeMap" => {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        let mut args_iter = args.args.iter();
                        let _key = args_iter.next(); // Skip key type (assumed to be string)
                        if let Some(GenericArgument::Type(value_ty)) = args_iter.next() {
                            let value_schema =
                                generate_type_schema(value_ty, &SchemaAttrs::default());
                            return quote! {
                                fastapi_openapi::Schema::Object(fastapi_openapi::ObjectSchema {
                                    title: None,
                                    description: None,
                                    properties: std::collections::HashMap::new(),
                                    required: Vec::new(),
                                    additional_properties: Some(Box::new(#value_schema)),
                                })
                            };
                        }
                    }
                    // Fallback
                    quote! {
                        fastapi_openapi::Schema::Object(fastapi_openapi::ObjectSchema::default())
                    }
                }

                // Other types - use JsonSchema trait if implemented, otherwise reference
                _ => {
                    // For custom types, try to call their JsonSchema implementation
                    quote! {
                        <#ty as fastapi_openapi::JsonSchema>::schema()
                    }
                }
            };
        }
    }

    // Fallback: try to use the type's JsonSchema implementation
    quote! {
        <#ty as fastapi_openapi::JsonSchema>::schema()
    }
}

#[allow(clippy::too_many_lines)]
pub fn derive_json_schema_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let name_str = name.to_string();

    // Parse struct-level attributes
    let struct_attrs = SchemaAttrs::from_attributes(&input.attrs);
    let title = struct_attrs.title.as_ref().map_or_else(
        || quote! { Some(#name_str.to_string()) },
        |t| quote! { Some(#t.to_string()) },
    );
    let description = struct_attrs
        .description
        .as_ref()
        .map_or_else(|| quote! { None }, |d| quote! { Some(#d.to_string()) });

    // Handle struct data
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => fields
                .named
                .iter()
                .filter_map(|f| {
                    let attrs = SchemaAttrs::from_attributes(&f.attrs);
                    if attrs.skip {
                        return None;
                    }
                    let field_name = f.ident.as_ref()?.to_string();
                    let is_optional = unwrap_option_type(&f.ty).is_some();
                    Some(FieldInfo {
                        name: field_name,
                        ty: f.ty.clone(),
                        attrs,
                        is_optional,
                    })
                })
                .collect::<Vec<_>>(),
            Fields::Unnamed(_) => {
                // Tuple structs - not supported for object schema
                return quote! {
                    compile_error!("JsonSchema derive does not support tuple structs");
                }
                .into();
            }
            Fields::Unit => Vec::new(),
        },
        Data::Enum(data) => {
            // Handle enums as string enums (simple case)
            // TODO: Generate proper oneOf schema with enum variants
            let _variants: Vec<String> =
                data.variants.iter().map(|v| v.ident.to_string()).collect();

            let expanded = quote! {
                impl fastapi_openapi::JsonSchema for #name {
                    fn schema() -> fastapi_openapi::Schema {
                        // For simple enums, generate oneOf with const values
                        // TODO: Add proper enum variant handling
                        fastapi_openapi::Schema::Primitive(fastapi_openapi::PrimitiveSchema {
                            schema_type: fastapi_openapi::SchemaType::String,
                            format: None,
                            nullable: false,
                        })
                    }

                    fn schema_name() -> Option<&'static str> {
                        Some(#name_str)
                    }
                }
            };
            return TokenStream::from(expanded);
        }
        Data::Union(_) => {
            return quote! {
                compile_error!("JsonSchema derive does not support unions");
            }
            .into();
        }
    };

    // Generate property insertions
    let property_insertions: Vec<TokenStream2> = fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let schema_code = generate_type_schema(&field.ty, &field.attrs);
            quote! {
                properties.insert(#field_name.to_string(), #schema_code);
            }
        })
        .collect();

    // Generate required field names (non-optional fields)
    let required_fields: Vec<&str> = fields
        .iter()
        .filter(|f| !f.is_optional)
        .map(|f| f.name.as_str())
        .collect();

    let expanded = quote! {
        impl fastapi_openapi::JsonSchema for #name {
            fn schema() -> fastapi_openapi::Schema {
                let mut properties = std::collections::HashMap::new();
                #(#property_insertions)*

                let required = vec![#(#required_fields.to_string()),*];

                fastapi_openapi::Schema::Object(fastapi_openapi::ObjectSchema {
                    title: #title,
                    description: #description,
                    properties,
                    required,
                    additional_properties: None,
                })
            }

            fn schema_name() -> Option<&'static str> {
                Some(#name_str)
            }
        }
    };

    TokenStream::from(expanded)
}
