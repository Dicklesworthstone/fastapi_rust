//! Route attribute macro implementation.
//!
//! Provides compile-time validation that:
//! - Handler function is async
//! - All parameters implement FromRequest
//! - Return type implements IntoResponse
//! - Path parameters in routes have corresponding extractors
//! - No extractor references non-existent path parameters
//! - Type compatibility between route and handler

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    FnArg, GenericArgument, ItemFn, LitStr, PathArguments, ReturnType, Token, Type, parse::Parse,
    parse::ParseStream, parse_macro_input, punctuated::Punctuated,
};

/// A declared response type for OpenAPI documentation.
struct ResponseDecl {
    /// HTTP status code (e.g., 200, 201, 404).
    status: u16,
    /// The response type name.
    type_name: String,
    /// The full type path for compile-time checking.
    type_path: Type,
    /// Optional description for the response.
    description: Option<String>,
}

/// Parsed route attributes from `#[get("/path", summary = "...", ...)]`.
struct RouteAttrs {
    /// The route path (required).
    path: LitStr,
    /// OpenAPI summary (short description).
    summary: Option<String>,
    /// OpenAPI description (detailed explanation).
    description: Option<String>,
    /// Custom operation ID.
    operation_id: Option<String>,
    /// Tags for grouping routes.
    tags: Vec<String>,
    /// Whether the route is deprecated.
    deprecated: bool,
    /// Declared response types for compile-time checking and OpenAPI.
    responses: Vec<ResponseDecl>,
}

impl Parse for RouteAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // First argument must be the path string
        let path: LitStr = input.parse()?;

        let mut attrs = RouteAttrs {
            path,
            summary: None,
            description: None,
            operation_id: None,
            tags: Vec::new(),
            deprecated: false,
            responses: Vec::new(),
        };

        // Parse optional comma-separated key=value pairs
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;

            // Handle trailing comma
            if input.is_empty() {
                break;
            }

            let ident: syn::Ident = input.parse()?;
            let ident_str = ident.to_string();

            match ident_str.as_str() {
                "deprecated" => {
                    // `deprecated` is a flag, no value needed
                    attrs.deprecated = true;
                }
                "summary" | "description" | "operation_id" => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;
                    match ident_str.as_str() {
                        "summary" => attrs.summary = Some(value.value()),
                        "description" => attrs.description = Some(value.value()),
                        "operation_id" => attrs.operation_id = Some(value.value()),
                        _ => unreachable!(),
                    }
                }
                "tags" => {
                    input.parse::<Token![=]>()?;
                    // Parse as either a single string or an array of strings
                    if input.peek(syn::token::Bracket) {
                        let content;
                        syn::bracketed!(content in input);
                        let tags: Punctuated<LitStr, Token![,]> =
                            Punctuated::parse_terminated(&content)?;
                        attrs.tags = tags.into_iter().map(|s| s.value()).collect();
                    } else {
                        let tag: LitStr = input.parse()?;
                        attrs.tags.push(tag.value());
                    }
                }
                "response" => {
                    // Parse response(status, Type) or response(status, Type, "description")
                    let content;
                    syn::parenthesized!(content in input);

                    // Parse status code
                    let status_lit: syn::LitInt = content.parse()?;
                    let status: u16 = status_lit.base10_parse().map_err(|_| {
                        syn::Error::new(status_lit.span(), "expected HTTP status code (e.g., 200)")
                    })?;

                    content.parse::<Token![,]>()?;

                    // Parse the response type
                    let type_path: Type = content.parse()?;
                    let type_name = extract_type_name(&type_path);

                    // Parse optional description
                    let description = if content.peek(Token![,]) {
                        content.parse::<Token![,]>()?;
                        let desc: LitStr = content.parse()?;
                        Some(desc.value())
                    } else {
                        None
                    };

                    attrs.responses.push(ResponseDecl {
                        status,
                        type_name,
                        type_path,
                        description,
                    });
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown route attribute `{ident_str}`.\n\
                             Valid attributes: summary, description, operation_id, tags, deprecated, response"
                        ),
                    ));
                }
            }
        }

        Ok(attrs)
    }
}

/// Extract path parameter names from a route pattern.
///
/// Examples:
/// - "/users/{id}" -> ["id"]
/// - "/users/{user_id}/posts/{post_id}" -> ["user_id", "post_id"]
/// - "/items/{id:int}" -> ["id"]
fn extract_path_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();

    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if segment.starts_with('{') && segment.ends_with('}') {
            let inner = &segment[1..segment.len() - 1];
            // Handle type hints like {id:int}
            let name = if let Some(pos) = inner.find(':') {
                &inner[..pos]
            } else {
                inner
            };
            params.push(name.to_string());
        }
    }

    params
}

/// Check if a type is a Path extractor and extract its inner type.
fn is_path_extractor(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Path";
        }
    }
    false
}

/// Analyze function arguments to find Path extractors.
///
/// Returns the count of Path extractors found.
fn count_path_extractors(inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>) -> usize {
    inputs
        .iter()
        .filter(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                is_path_extractor(&pat_type.ty)
            } else {
                false
            }
        })
        .count()
}

/// Count tuple elements in a Path<(T1, T2, ...)> type.
fn count_tuple_elements(ty: &Type) -> Option<usize> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Path" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(Type::Tuple(tuple))) = args.args.first() {
                        return Some(tuple.elems.len());
                    }
                }
            }
        }
    }
    None
}

/// Check if a type is one of the context types that don't require FromRequest.
/// These are: &Cx, &RequestContext, &mut Request
fn is_context_type(ty: &Type) -> bool {
    if let Type::Reference(ref_type) = ty {
        if let Type::Path(type_path) = &*ref_type.elem {
            if let Some(segment) = type_path.path.segments.last() {
                let name = segment.ident.to_string();
                return matches!(name.as_str(), "Cx" | "RequestContext" | "Request");
            }
        }
    }
    false
}

fn is_mut_request_ref(ty: &Type) -> bool {
    let Type::Reference(ref_type) = ty else {
        return false;
    };
    if ref_type.mutability.is_none() {
        return false;
    }
    let Type::Path(type_path) = &*ref_type.elem else {
        return false;
    };
    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };
    segment.ident == "Request"
}

/// Extract the type from a function argument, excluding pattern binding.
fn extract_param_type(arg: &FnArg) -> Option<&Type> {
    match arg {
        FnArg::Typed(pat_type) => Some(&pat_type.ty),
        FnArg::Receiver(_) => None, // `self` parameter
    }
}

/// Extract types from function arguments that should implement FromRequest.
/// Excludes context types (&Cx, &RequestContext) which are handled specially.
fn get_extractable_types(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Vec<&Type> {
    inputs
        .iter()
        .filter_map(|arg| {
            let ty = extract_param_type(arg)?;
            // Skip reference types (context types like &Cx, &RequestContext)
            if is_context_type(ty) {
                return None;
            }
            Some(ty)
        })
        .collect()
}

/// Body extractor information for OpenAPI request body generation.
struct BodyExtractorInfo {
    /// The inner type name (e.g., "CreateUser" from Json<CreateUser>).
    type_name: String,
    /// The content type (e.g., "application/json").
    content_type: &'static str,
    /// Whether the body is required (not Option<Json<T>>).
    required: bool,
}

/// Check if a type is a body extractor (Json<T>) and extract its info.
///
/// Returns None if the type is not a body extractor.
/// Handles both `Json<T>` and `Option<Json<T>>`.
fn extract_body_info(ty: &Type) -> Option<BodyExtractorInfo> {
    // Check for Option<Json<T>> first
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        if let Some(mut info) = extract_json_info(inner_ty) {
                            info.required = false;
                            return Some(info);
                        }
                    }
                }
            }
        }
    }

    // Check for Json<T> directly
    extract_json_info(ty)
}

/// Extract type info from a Json<T> type.
fn extract_json_info(ty: &Type) -> Option<BodyExtractorInfo> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Json" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        let type_name = extract_type_name(inner_ty);
                        return Some(BodyExtractorInfo {
                            type_name,
                            content_type: "application/json",
                            required: true,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Extract a simple type name from a Type.
///
/// For complex types like `Vec<Item>`, returns the full string representation.
fn extract_type_name(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                segment.ident.to_string()
            } else {
                quote::quote!(#ty).to_string()
            }
        }
        _ => quote::quote!(#ty).to_string(),
    }
}

/// Find the first body extractor in function arguments and return its info.
fn find_body_extractor(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Option<BodyExtractorInfo> {
    for arg in inputs {
        if let Some(ty) = extract_param_type(arg) {
            if let Some(info) = extract_body_info(ty) {
                return Some(info);
            }
        }
    }
    None
}

/// Extract the inner type from Result<T, E> or impl Trait return types.
fn get_return_type(output: &ReturnType) -> Option<proc_macro2::TokenStream> {
    match output {
        ReturnType::Default => {
            // () return type - this implements IntoResponse
            Some(quote! { () })
        }
        ReturnType::Type(_, ty) => {
            // For Result<T, E>, we need to check both T and E implement IntoResponse
            // For impl IntoResponse, we can't check at macro time (handled by compiler)
            // For concrete types, we can generate the assertion
            if let Type::ImplTrait(_) = &**ty {
                // Can't validate impl Trait at macro time - compiler will check
                None
            } else {
                Some(quote! { #ty })
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn route_impl(method: &str, attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as RouteAttrs);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_vis = &input_fn.vis;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_output = &input_fn.sig.output;
    let fn_asyncness = &input_fn.sig.asyncness;
    let fn_attrs = &input_fn.attrs;

    let route_fn_name = syn::Ident::new(&format!("__route_{fn_name}"), fn_name.span());
    let route_entry_fn_name = syn::Ident::new(&format!("{fn_name}_route"), fn_name.span());
    let reg_name = syn::Ident::new(&format!("__FASTAPI_ROUTE_REG_{fn_name}"), fn_name.span());

    let method_ident = syn::Ident::new(method, proc_macro2::Span::call_site());
    let path = &attrs.path;
    let path_str = path.value();

    // =========================================================================
    // COMPILE-TIME HANDLER SIGNATURE VALIDATION
    // =========================================================================

    // Validation 0: Handler must be async
    if fn_asyncness.is_none() {
        let error_msg = format!(
            "handler '{fn_name}' must be async.\n\
             Route handlers must be async functions to work with asupersync.\n\n\
             Change:\n  fn {fn_name}(...) -> ...\n\nTo:\n  async fn {fn_name}(...) -> ..."
        );
        return syn::Error::new(fn_name.span(), error_msg)
            .to_compile_error()
            .into();
    }

    // Validation 0b: If the handler requests `&mut Request`, require it to be last.
    for (idx, arg) in fn_inputs.iter().enumerate() {
        let Some(ty) = extract_param_type(arg) else {
            continue;
        };
        if is_mut_request_ref(ty) && idx != fn_inputs.len().saturating_sub(1) {
            return syn::Error::new_spanned(
                ty,
                "`&mut Request` parameters must be the last handler argument",
            )
            .to_compile_error()
            .into();
        }
    }

    // =========================================================================
    // COMPILE-TIME PATH PARAMETER VALIDATION
    // =========================================================================

    let path_params = extract_path_params(&path_str);
    let path_param_count = path_params.len();
    let path_extractor_count = count_path_extractors(fn_inputs);

    // Validation 1: Route has parameters but no Path extractor
    if path_param_count > 0 && path_extractor_count == 0 {
        let param_list = path_params.join(", ");
        let error_msg = format!(
            "route '{}' has {} path parameter(s) [{}] but handler '{}' has no Path<_> extractor.\n\
             Add a Path extractor, e.g.:\n\
             - Path<i64> for single parameter\n\
             - Path<({})> for multiple parameters\n\
             - Path<MyParams> for named struct",
            path_str,
            path_param_count,
            param_list,
            fn_name,
            path_params
                .iter()
                .map(|_| "T".to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return syn::Error::new(path.span(), error_msg)
            .to_compile_error()
            .into();
    }

    // Validation 2: Handler has Path extractor but route has no parameters
    if path_param_count == 0 && path_extractor_count > 0 {
        let error_msg = format!(
            "handler '{fn_name}' has a Path<_> extractor but route '{path_str}' has no path parameters.\n\
             Either add path parameters to the route (e.g., '/items/{{id}}') \
             or remove the Path extractor."
        );
        return syn::Error::new(Span::call_site(), error_msg)
            .to_compile_error()
            .into();
    }

    // Validation 3: Check tuple arity matches parameter count
    for arg in fn_inputs {
        if let FnArg::Typed(pat_type) = arg {
            if let Some(tuple_count) = count_tuple_elements(&pat_type.ty) {
                if tuple_count != path_param_count {
                    let error_msg = format!(
                        "Path tuple has {} element(s) but route '{}' has {} path parameter(s) [{}].\n\
                         The tuple element count must match the number of path parameters.",
                        tuple_count,
                        path_str,
                        path_param_count,
                        path_params.join(", ")
                    );
                    return syn::Error::new(Span::call_site(), error_msg)
                        .to_compile_error()
                        .into();
                }
            }
        }
    }

    // =========================================================================
    // COMPILE-TIME TRAIT BOUND ASSERTIONS
    // =========================================================================

    // Collect types that need FromRequest validation
    let extractable_types = get_extractable_types(fn_inputs);

    // Generate compile-time assertions for FromRequest
    // These assertions will fail to compile if a type doesn't implement FromRequest
    let from_request_checks: Vec<proc_macro2::TokenStream> = extractable_types
        .iter()
        .enumerate()
        .map(|(idx, ty)| {
            let check_fn_name = syn::Ident::new(
                &format!("__assert_from_request_{fn_name}_{idx}"),
                Span::call_site(),
            );
            quote! {
                #[doc(hidden)]
                #[allow(dead_code)]
                const _: () = {
                    // This function will fail to compile if the type doesn't implement FromRequest.
                    // The error message will point to the handler function, making it clear which
                    // parameter is problematic.
                    fn #check_fn_name<T: fastapi_core::FromRequest>() {}

                    // Instantiate the check for the concrete type
                    fn __trigger_check() {
                        #check_fn_name::<#ty>();
                    }
                };
            }
        })
        .collect();

    // Generate compile-time assertion for IntoResponse
    let into_response_check = if let Some(return_ty) = get_return_type(fn_output) {
        let check_fn_name = syn::Ident::new(
            &format!("__assert_into_response_{fn_name}"),
            Span::call_site(),
        );
        Some(quote! {
            #[doc(hidden)]
            #[allow(dead_code)]
            const _: () = {
                // This function will fail to compile if the return type doesn't implement
                // IntoResponse. For Result<T, E>, both T and E must implement IntoResponse.
                fn #check_fn_name<T: fastapi_core::IntoResponse>() {}

                fn __trigger_check() {
                    #check_fn_name::<#return_ty>();
                }
            };
        })
    } else {
        None // impl IntoResponse or similar - compiler will validate
    };

    // Generate metadata builder calls
    let summary_call = attrs.summary.as_ref().map(|s| {
        quote! { .summary(#s) }
    });

    let description_call = attrs.description.as_ref().map(|d| {
        quote! { .description(#d) }
    });

    let operation_id_call = attrs.operation_id.as_ref().map(|id| {
        quote! { .operation_id(#id) }
    });

    let tags = &attrs.tags;
    let tags_call = if tags.is_empty() {
        None
    } else {
        Some(quote! { .tags([#(#tags),*]) })
    };

    let deprecated_call = if attrs.deprecated {
        Some(quote! { .deprecated() })
    } else {
        None
    };

    // Generate request body builder call if a body extractor is present
    let request_body_call = find_body_extractor(fn_inputs).map(|info| {
        let schema = &info.type_name;
        let content_type = info.content_type;
        let required = info.required;
        quote! { .request_body(#schema, #content_type, #required) }
    });

    // Generate compile-time assertions for declared response types
    // Each declared response type must implement JsonSchema
    let response_schema_checks: Vec<proc_macro2::TokenStream> = attrs
        .responses
        .iter()
        .enumerate()
        .map(|(idx, resp)| {
            let check_fn_name = syn::Ident::new(
                &format!("__assert_response_schema_{fn_name}_{idx}"),
                Span::call_site(),
            );
            let ty = &resp.type_path;
            let status = resp.status;
            quote! {
                #[doc(hidden)]
                #[allow(dead_code)]
                const _: () = {
                    // This function will fail to compile if the response type doesn't implement
                    // JsonSchema. This ensures the declared response type can be used in OpenAPI.
                    fn #check_fn_name<T: fastapi_openapi::JsonSchema>() {}

                    fn __trigger_check() {
                        // Assert that the declared response type implements JsonSchema
                        #check_fn_name::<#ty>();
                    }

                    // Store the status code and type name for debugging
                    const _STATUS: u16 = #status;
                };
            }
        })
        .collect();

    // Generate response type verification (compile-time check that return matches declared)
    // This uses a marker trait to verify the handler's return type can produce the declared schema
    let response_type_checks: Vec<proc_macro2::TokenStream> =
        if let Some(ref return_ty) = get_return_type(fn_output) {
            attrs
                .responses
                .iter()
                .filter(|r| r.status == 200) // Only check 200 responses against return type
                .map(|resp| {
                    let check_fn_name = syn::Ident::new(
                        &format!("__assert_response_type_{fn_name}"),
                        Span::call_site(),
                    );
                    let resp_ty = &resp.type_path;
                    quote! {
                        #[doc(hidden)]
                        #[allow(dead_code)]
                        const _: () = {
                            // Verify the handler can produce the declared response type.
                            // This checks that ReturnType: ResponseProduces<DeclaredType>
                            fn #check_fn_name<R, T>()
                            where
                                R: fastapi_core::ResponseProduces<T>,
                            {}

                            fn __trigger_check() {
                                #check_fn_name::<#return_ty, #resp_ty>();
                            }
                        };
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

    // Generate response metadata builder calls
    let response_calls: Vec<proc_macro2::TokenStream> = attrs
        .responses
        .iter()
        .map(|resp| {
            let status = resp.status;
            let type_name = &resp.type_name;
            let description = resp.description.as_deref().unwrap_or("Successful response");
            quote! { .response(#status, #type_name, #description) }
        })
        .collect();

    // Generate extraction + invocation wrapper for runtime routing.
    let mut arg_extracts: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut call_args: Vec<proc_macro2::TokenStream> = Vec::new();

    for (i, arg) in fn_inputs.iter().enumerate() {
        let Some(ty) = extract_param_type(arg) else {
            continue;
        };

        // Context injections are passed directly.
        if is_context_type(ty) {
            if let Type::Reference(ref_type) = ty {
                if let Type::Path(type_path) = &*ref_type.elem {
                    if let Some(segment) = type_path.path.segments.last() {
                        let name = segment.ident.to_string();
                        match name.as_str() {
                            "Cx" => {
                                call_args.push(quote! { ctx.cx() });
                                continue;
                            }
                            "RequestContext" => {
                                call_args.push(quote! { ctx });
                                continue;
                            }
                            "Request" => {
                                call_args.push(quote! { req });
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let ident = syn::Ident::new(&format!("__fastapi_arg_{i}"), Span::call_site());
        arg_extracts.push(quote! {
            let #ident: #ty = match <#ty as fastapi_core::FromRequest>::from_request(ctx, req).await {
                Ok(v) => v,
                Err(e) => return __into_response(e),
            };
        });
        call_args.push(quote! { #ident });
    }

    let call_handler = if fn_asyncness.is_some() {
        quote! { #fn_name(#(#call_args),*).await }
    } else {
        quote! { #fn_name(#(#call_args),*) }
    };

    // Generate the expanded code
    let expanded = quote! {
        // Original function preserved for direct calling
        #(#fn_attrs)*
        #fn_vis #fn_asyncness fn #fn_name(#fn_inputs) #fn_output #fn_block

        // Compile-time assertions: all extractor parameters must implement FromRequest
        #(#from_request_checks)*

        // Compile-time assertion: return type must implement IntoResponse
        #into_response_check

        // Compile-time assertions: declared response types must implement JsonSchema
        #(#response_schema_checks)*

        // Compile-time assertion: return type matches declared 200 response
        #(#response_type_checks)*

        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #route_fn_name() -> fastapi_router::Route {
            fastapi_router::Route::with_placeholder_handler(
                fastapi_core::Method::#method_ident,
                #path_str,
            )
            #summary_call
            #description_call
            #operation_id_call
            #tags_call
            #deprecated_call
            #request_body_call
            #(#response_calls)*
        }

        /// Build a runtime [`fastapi_core::RouteEntry`] for this handler.
        ///
        /// This wraps the handler with extractor evaluation (`FromRequest`) and converts
        /// both extractor errors and handler return values into a [`fastapi_core::Response`].
        #[allow(non_snake_case)]
        pub fn #route_entry_fn_name() -> fastapi_core::RouteEntry {
            fn __into_response<T: fastapi_core::IntoResponse>(v: T) -> fastapi_core::Response {
                v.into_response()
            }

            fastapi_core::RouteEntry::new(
                fastapi_core::Method::#method_ident,
                #path_str,
                |ctx, req| {
                    Box::pin(async move {
                        #(#arg_extracts)*
                        let out = #call_handler;
                        __into_response(out)
                    }) as fastapi_core::BoxFuture<'_, fastapi_core::Response>
                },
            )
        }

        // Static registration for route discovery
        #[doc(hidden)]
        #[allow(unsafe_code)]
        #[allow(non_upper_case_globals)]
        #[used]
        #[cfg_attr(
            any(target_os = "linux", target_os = "android", target_os = "freebsd"),
            unsafe(link_section = "fastapi_routes")
        )]
        static #reg_name: fastapi_router::RouteRegistration =
            fastapi_router::RouteRegistration::new(#route_fn_name);
    };

    TokenStream::from(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_path_params_empty() {
        assert!(extract_path_params("/users").is_empty());
        assert!(extract_path_params("/api/v1/items").is_empty());
        assert!(extract_path_params("/").is_empty());
    }

    #[test]
    fn test_extract_path_params_single() {
        assert_eq!(extract_path_params("/users/{id}"), vec!["id"]);
        assert_eq!(extract_path_params("/items/{item_id}"), vec!["item_id"]);
    }

    #[test]
    fn test_extract_path_params_multiple() {
        assert_eq!(
            extract_path_params("/users/{user_id}/posts/{post_id}"),
            vec!["user_id", "post_id"]
        );
        assert_eq!(
            extract_path_params("/api/v1/{org}/{repo}/issues/{id}"),
            vec!["org", "repo", "id"]
        );
    }

    #[test]
    fn test_extract_path_params_with_type_hints() {
        assert_eq!(extract_path_params("/items/{id:int}"), vec!["id"]);
        assert_eq!(
            extract_path_params("/users/{uuid:uuid}/files/{path:path}"),
            vec!["uuid", "path"]
        );
        assert_eq!(extract_path_params("/values/{val:float}"), vec!["val"]);
    }

    #[test]
    fn test_extract_path_params_mixed() {
        assert_eq!(
            extract_path_params("/api/{version}/users/{id:int}/profile"),
            vec!["version", "id"]
        );
    }

    #[test]
    fn test_is_context_type_cx() {
        // Test &Cx is recognized as context type
        let ty: Type = syn::parse_quote! { &Cx };
        assert!(is_context_type(&ty));
    }

    #[test]
    fn test_is_context_type_request_context() {
        // Test &RequestContext is recognized as context type
        let ty: Type = syn::parse_quote! { &RequestContext };
        assert!(is_context_type(&ty));
    }

    #[test]
    fn test_is_context_type_request() {
        // Test &mut Request is recognized as context type
        let ty: Type = syn::parse_quote! { &mut Request };
        assert!(is_context_type(&ty));
    }

    #[test]
    fn test_is_context_type_non_context() {
        // Test Path<i64> is not a context type
        let ty: Type = syn::parse_quote! { Path<i64> };
        assert!(!is_context_type(&ty));

        // Test Json<T> is not a context type
        let ty: Type = syn::parse_quote! { Json<User> };
        assert!(!is_context_type(&ty));
    }

    #[test]
    fn test_get_return_type_unit() {
        use syn::ReturnType;

        let ret: ReturnType = syn::parse_quote! {};
        let result = get_return_type(&ret);
        assert!(result.is_some());
    }

    #[test]
    fn test_get_return_type_concrete() {
        use syn::ReturnType;

        let ret: ReturnType = syn::parse_quote! { -> Response };
        let result = get_return_type(&ret);
        assert!(result.is_some());
    }

    #[test]
    fn test_get_return_type_impl_trait() {
        use syn::ReturnType;

        let ret: ReturnType = syn::parse_quote! { -> impl IntoResponse };
        let result = get_return_type(&ret);
        // impl Trait should return None (compiler validates)
        assert!(result.is_none());
    }

    #[test]
    fn test_route_attrs_path_only() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users" };
        assert_eq!(attrs.path.value(), "/users");
        assert!(attrs.summary.is_none());
        assert!(attrs.description.is_none());
        assert!(attrs.operation_id.is_none());
        assert!(attrs.tags.is_empty());
        assert!(!attrs.deprecated);
    }

    #[test]
    fn test_route_attrs_with_summary() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users", summary = "List all users" };
        assert_eq!(attrs.path.value(), "/users");
        assert_eq!(attrs.summary.as_deref(), Some("List all users"));
    }

    #[test]
    fn test_route_attrs_with_description() {
        let attrs: RouteAttrs =
            syn::parse_quote! { "/users", description = "A detailed description" };
        assert_eq!(attrs.path.value(), "/users");
        assert_eq!(attrs.description.as_deref(), Some("A detailed description"));
    }

    #[test]
    fn test_route_attrs_with_operation_id() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users", operation_id = "getUsers" };
        assert_eq!(attrs.path.value(), "/users");
        assert_eq!(attrs.operation_id.as_deref(), Some("getUsers"));
    }

    #[test]
    fn test_route_attrs_with_single_tag() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users", tags = "users" };
        assert_eq!(attrs.path.value(), "/users");
        assert_eq!(attrs.tags, vec!["users"]);
    }

    #[test]
    fn test_route_attrs_with_multiple_tags() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users", tags = ["users", "api", "v1"] };
        assert_eq!(attrs.path.value(), "/users");
        assert_eq!(attrs.tags, vec!["users", "api", "v1"]);
    }

    #[test]
    fn test_route_attrs_deprecated() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users", deprecated };
        assert_eq!(attrs.path.value(), "/users");
        assert!(attrs.deprecated);
    }

    #[test]
    fn test_route_attrs_all_options() {
        let attrs: RouteAttrs = syn::parse_quote! {
            "/items/{id}",
            summary = "Get an item",
            description = "Retrieves an item by its unique identifier",
            operation_id = "getItemById",
            tags = ["items", "crud"],
            deprecated
        };
        assert_eq!(attrs.path.value(), "/items/{id}");
        assert_eq!(attrs.summary.as_deref(), Some("Get an item"));
        assert_eq!(
            attrs.description.as_deref(),
            Some("Retrieves an item by its unique identifier")
        );
        assert_eq!(attrs.operation_id.as_deref(), Some("getItemById"));
        assert_eq!(attrs.tags, vec!["items", "crud"]);
        assert!(attrs.deprecated);
    }

    #[test]
    fn test_route_attrs_trailing_comma() {
        let attrs: RouteAttrs = syn::parse_quote! { "/users", summary = "Test", };
        assert_eq!(attrs.path.value(), "/users");
        assert_eq!(attrs.summary.as_deref(), Some("Test"));
    }

    #[test]
    fn test_extract_body_info_json() {
        let ty: Type = syn::parse_quote! { Json<CreateUser> };
        let info = extract_body_info(&ty);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.type_name, "CreateUser");
        assert_eq!(info.content_type, "application/json");
        assert!(info.required);
    }

    #[test]
    fn test_extract_body_info_optional_json() {
        let ty: Type = syn::parse_quote! { Option<Json<UpdateUser>> };
        let info = extract_body_info(&ty);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.type_name, "UpdateUser");
        assert_eq!(info.content_type, "application/json");
        assert!(!info.required); // Optional body is not required
    }

    #[test]
    fn test_extract_body_info_non_body() {
        // Path extractor is not a body extractor
        let ty: Type = syn::parse_quote! { Path<i64> };
        assert!(extract_body_info(&ty).is_none());

        // Query extractor is not a body extractor
        let ty: Type = syn::parse_quote! { Query<Params> };
        assert!(extract_body_info(&ty).is_none());

        // Header extractor is not a body extractor
        let ty: Type = syn::parse_quote! { Header<ContentType> };
        assert!(extract_body_info(&ty).is_none());
    }

    #[test]
    fn test_extract_type_name_simple() {
        let ty: Type = syn::parse_quote! { User };
        assert_eq!(extract_type_name(&ty), "User");

        let ty: Type = syn::parse_quote! { CreateUserRequest };
        assert_eq!(extract_type_name(&ty), "CreateUserRequest");
    }

    #[test]
    fn test_extract_type_name_vec() {
        let ty: Type = syn::parse_quote! { Vec<Item> };
        // For generic types, we just get the outer type name
        assert_eq!(extract_type_name(&ty), "Vec");
    }
}
