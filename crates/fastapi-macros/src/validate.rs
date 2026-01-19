//! Validation derive macro implementation.

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

pub fn derive_validate_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // TODO: Parse #[validate(...)] attributes on fields
    // and generate validation code

    let expanded = quote! {
        impl #name {
            /// Validate this value.
            ///
            /// # Errors
            ///
            /// Returns validation errors if any constraints are violated.
            pub fn validate(&self) -> Result<(), fastapi_core::ValidationErrors> {
                let mut errors = fastapi_core::ValidationErrors::new();

                // TODO: Generate field validations from attributes

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
