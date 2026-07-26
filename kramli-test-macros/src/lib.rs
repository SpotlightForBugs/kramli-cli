use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

fn is_test_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("test")
}

fn is_tokio_test_attr(attr: &syn::Attribute) -> bool {
    let path = attr.path();
    path.segments.len() == 2
        && path.segments[0].ident == "tokio"
        && path.segments[1].ident == "test"
}

fn strip_test_attrs(attrs: &mut Vec<syn::Attribute>) {
    attrs.retain(|attr| !is_test_attr(attr) && !is_tokio_test_attr(attr));
}

/// Wraps a synchronous unit test with a per-test timeout.
#[proc_macro_attribute]
pub fn test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemFn);
    strip_test_attrs(&mut input.attrs);
    let name = &input.sig.ident;
    let attrs = &input.attrs;
    let vis = &input.vis;
    let block = &input.block;

    quote! {
        #(#attrs)*
        #[::core::prelude::v1::test]
        #vis fn #name() {
            crate::test_env::run_sync_test(stringify!(#name), || #block);
        }
    }
    .into()
}

/// Wraps an async `tokio` unit test with a per-test timeout.
#[proc_macro_attribute]
pub fn tokio_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemFn);
    strip_test_attrs(&mut input.attrs);
    let name = &input.sig.ident;
    let attrs = &input.attrs;
    let vis = &input.vis;
    let sig = &input.sig;
    let block = &input.block;

    quote! {
        #(#attrs)*
        #[::tokio::test]
        #vis #sig {
            crate::test_env::run_async_test(stringify!(#name), async #block).await;
        }
    }
    .into()
}
