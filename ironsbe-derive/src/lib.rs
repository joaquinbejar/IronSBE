//! # IronSBE Derive
//!
//! Procedural macros for SBE message definitions.
//!
//! This crate provides derive macros for automatically implementing
//! SBE encoder/decoder traits.

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

/// Derives the SbeMessage trait for a struct.
///
/// # Example
/// ```ignore
/// #[derive(SbeMessage)]
/// #[sbe(template_id = 1, block_length = 56)]
/// struct NewOrderSingle {
///     #[sbe(offset = 0, type = "ClOrdId")]
///     cl_ord_id: [u8; 20],
///     #[sbe(offset = 20, type = "Symbol")]
///     symbol: [u8; 8],
/// }
/// ```
#[proc_macro_derive(SbeMessage, attributes(sbe))]
pub fn derive_sbe_message(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // For now, generate a simple implementation
    // Full implementation would parse attributes and generate field accessors
    let expanded = quote! {
        impl #name {
            /// Returns the template ID for this message.
            pub const fn template_id() -> u16 {
                0 // Would be parsed from attribute
            }
        }
    };

    TokenStream::from(expanded)
}

/// Derives field accessors for SBE fields.
#[proc_macro_derive(SbeField, attributes(sbe))]
pub fn derive_sbe_field(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let _name = &input.ident;

    // Placeholder implementation
    let expanded = quote! {};

    TokenStream::from(expanded)
}
