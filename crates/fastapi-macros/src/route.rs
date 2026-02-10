//! Route attribute macro implementation.
//!
//! This module provides the `#[route]` and HTTP method macros (`#[get]`, `#[post]`, etc.)
//! that mark functions as HTTP handlers and generate extraction code.
//!
//! # Example
//!
//! ```ignore
//! #[get("/users/{id}")]
//! async fn get_user(path: Path<UserId>, json: Json<UserQuery>) -> Json<User> {
//!     // ...
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    FnArg, Ident, ItemFn, LitStr, Pat, PatIdent, PatType, ReturnType, Type, parse_macro_input,
};

/// Parsed function parameter info for code generation.
struct ParamInfo {
    /// The parameter name (identifier).
    name: Ident,
    /// The parameter type.
    ty: Type,
}

pub fn route_impl(method: &str, attr: TokenStream, item: TokenStream) -> TokenStream {
    let path = parse_macro_input!(attr as LitStr);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_vis = &input_fn.vis;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_output = &input_fn.sig.output;
    let fn_asyncness = &input_fn.sig.asyncness;
    let fn_attrs = &input_fn.attrs;

    // Parse all parameters
    let params: Vec<ParamInfo> = fn_inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(PatType { pat, ty, .. }) => {
                if let Pat::Ident(PatIdent { ident, .. }) = pat.as_ref() {
                    Some(ParamInfo {
                        name: ident.clone(),
                        ty: ty.as_ref().clone(),
                    })
                } else {
                    None
                }
            }
            FnArg::Receiver(_) => None,
        })
        .collect();

    // Generate the internal handler function name
    let handler_fn_name = format_ident!("__{}_handler", fn_name);
    let route_fn_name = format_ident!("__route_{}", fn_name);
    let reg_name = format_ident!("__FASTAPI_ROUTE_REG_{}", fn_name);

    let method_ident = Ident::new(method, proc_macro2::Span::call_site());
    let path_str = path.value();

    // Generate extraction code for each parameter
    let extraction_code = generate_extraction_code(&params);

    // Generate the parameter names for calling the inner function
    let param_names: Vec<_> = params.iter().map(|p| &p.name).collect();

    // Determine if function is async
    let is_async = fn_asyncness.is_some();

    // Generate the inner call (with or without await)
    let inner_call = if is_async {
        quote! { #fn_name(#(#param_names),*).await }
    } else {
        quote! { #fn_name(#(#param_names),*) }
    };

    // Generate return type handling
    let return_handling = match fn_output {
        ReturnType::Default => quote! {
            let _ = #inner_call;
            fastapi_core::Response::new(fastapi_core::StatusCode::OK)
        },
        ReturnType::Type(_, _) => quote! {
            let result = #inner_call;
            fastapi_core::IntoResponse::into_response(result)
        },
    };

    let expanded = quote! {
        // Original function preserved for direct calling
        #(#fn_attrs)*
        #fn_vis #fn_asyncness fn #fn_name(#fn_inputs) #fn_output #fn_block

        // Handler wrapper that extracts parameters from request
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #handler_fn_name<'a>(
            ctx: &'a fastapi_core::RequestContext,
            req: &'a mut fastapi_core::Request,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = fastapi_core::Response> + Send + 'a>> {
            Box::pin(async move {
                use fastapi_core::FromRequest;
                use fastapi_core::IntoResponse;

                #extraction_code

                #return_handling
            })
        }

        // Route metadata function
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #route_fn_name() -> fastapi_router::Route {
            fastapi_router::Route::new(
                fastapi_core::Method::#method_ident,
                #path_str,
            ).handler(#handler_fn_name)
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

/// Generate extraction code for all parameters.
fn generate_extraction_code(params: &[ParamInfo]) -> TokenStream2 {
    let extractions: Vec<TokenStream2> = params
        .iter()
        .map(|param| {
            let name = &param.name;
            let ty = &param.ty;
            let name_str = name.to_string();

            quote! {
                let #name: #ty = match <#ty as FromRequest>::from_request(ctx, req).await {
                    Ok(val) => val,
                    Err(e) => {
                        // Log extraction failure
                        tracing::warn!(
                            param = #name_str,
                            error = ?e,
                            "Parameter extraction failed"
                        );
                        return e.into_response();
                    }
                };
            }
        })
        .collect();

    quote! {
        #(#extractions)*
    }
}
