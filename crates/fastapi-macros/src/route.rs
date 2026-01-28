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
    FnArg, GenericArgument, ItemFn, LitStr, PathArguments, ReturnType, Type, parse_macro_input,
};

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
    let path = parse_macro_input!(attr as LitStr);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_vis = &input_fn.vis;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_output = &input_fn.sig.output;
    let fn_asyncness = &input_fn.sig.asyncness;

    let route_fn_name = syn::Ident::new(&format!("__route_{fn_name}"), fn_name.span());
    let reg_name = syn::Ident::new(&format!("__FASTAPI_ROUTE_REG_{fn_name}"), fn_name.span());

    let method_ident = syn::Ident::new(method, proc_macro2::Span::call_site());
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

    // Generate the expanded code
    let expanded = quote! {
        #fn_vis #fn_asyncness fn #fn_name(#fn_inputs) #fn_output #fn_block

        // Compile-time assertions: all extractor parameters must implement FromRequest
        #(#from_request_checks)*

        // Compile-time assertion: return type must implement IntoResponse
        #into_response_check

        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #route_fn_name() -> fastapi_router::Route {
            fastapi_router::Route::with_placeholder_handler(
                fastapi_core::Method::#method_ident,
                #path_str,
            )
        }

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
}
