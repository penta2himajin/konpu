#![allow(clippy::collapsible_if)]

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    parse::Parser, punctuated::Punctuated, Expr, Ident, Lit, MetaNameValue, Token,
};

#[derive(Clone, Copy)]
enum Structure {
    Magma,
    Semigroup,
    Monoid,
    Group,
}

impl Structure {
    const fn rank(self) -> u8 {
        match self {
            Self::Magma => 0,
            Self::Semigroup => 1,
            Self::Monoid => 2,
            Self::Group => 3,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Magma => "magma",
            Self::Semigroup => "semigroup",
            Self::Monoid => "monoid",
            Self::Group => "group",
        }
    }
}

struct Args {
    op: Option<String>,
    identity: Option<String>,
    inverse: Option<String>,
    #[allow(dead_code)]
    higher: Option<String>,
    #[allow(dead_code)]
    cost: Option<String>,
}

fn str_value(expr: &Expr) -> Option<String> {
    if let Expr::Lit(lit) = expr {
        if let Lit::Str(s) = &lit.lit {
            return Some(s.value());
        }
    }
    None
}

fn parse_args(structure: Structure, attr: TokenStream) -> syn::Result<Args> {
    let metas = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse(attr)?;
    let mut args = Args {
        op: None,
        identity: None,
        inverse: None,
        higher: None,
        cost: None,
    };
    for m in metas {
        let key: String = m
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        let value = str_value(&m.value);
        match key.as_str() {
            "op" => {
                if let Some(v) = value {
                    args.op = Some(v);
                } else {
                    return Err(syn::Error::new_spanned(
                        &m.value,
                        "`op` must be a string literal",
                    ));
                }
            }
            "identity" => {
                if let Some(v) = value {
                    args.identity = Some(v);
                } else {
                    return Err(syn::Error::new_spanned(
                        &m.value,
                        "`identity` must be a string literal",
                    ));
                }
            }
            "inverse" => {
                if let Some(v) = value {
                    args.inverse = Some(v);
                } else {
                    return Err(syn::Error::new_spanned(
                        &m.value,
                        "`inverse` must be a string literal",
                    ));
                }
            }
            "higher" => {
                let v = value.ok_or_else(|| {
                    syn::Error::new_spanned(&m.value, "`higher` must be a string literal")
                })?;
                match v.as_str() {
                    "functor" | "applicative" | "monad" => args.higher = Some(v),
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &m.value,
                            "`higher` must be \"functor\", \"applicative\", or \"monad\"",
                        ))
                    }
                }
            }
            "cost" => {
                let v = value.ok_or_else(|| {
                    syn::Error::new_spanned(&m.value, "`cost` must be a string literal")
                })?;
                args.cost = Some(v);
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &m.path,
                    format!("unknown argument `{key}` for #[konpu::{}]", structure.name()),
                ))
            }
        }
    }
    Ok(args)
}

fn validate(structure: Structure, args: &Args) -> syn::Result<()> {
    let span = Span::call_site();
    if structure.rank() >= 1 && args.op.is_none() {
        return Err(syn::Error::new(
            span,
            format!("#[konpu::{}] requires `op = \"<name>\"`", structure.name()),
        ));
    }
    if structure.rank() >= 2 && args.identity.is_none() {
        return Err(syn::Error::new(
            span,
            format!("#[konpu::{}] requires `identity = \"<name>\"`", structure.name()),
        ));
    }
    if structure.rank() >= 3 && args.inverse.is_none() {
        return Err(syn::Error::new(
            span,
            format!("#[konpu::{}] requires `inverse = \"<name>\"`", structure.name()),
        ));
    }
    if let (Some(op), Some(id)) = (&args.op, &args.identity) {
        if op == id {
            return Err(syn::Error::new(
                span,
                "`identity` must differ from `op`",
            ));
        }
    }
    Ok(())
}

fn expand(structure: Structure, attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_tokens: proc_macro2::TokenStream = item.clone().into();
    match parse_args(structure, attr).and_then(|args| validate(structure, &args)) {
        Ok(()) => item,
        Err(e) => {
            let err = e.to_compile_error();
            TokenStream::from(quote! { #err #item_tokens })
        }
    }
}

#[proc_macro_attribute]
pub fn magma(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(Structure::Magma, attr, item)
}

#[proc_macro_attribute]
pub fn semigroup(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(Structure::Semigroup, attr, item)
}

#[proc_macro_attribute]
pub fn monoid(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(Structure::Monoid, attr, item)
}

#[proc_macro_attribute]
pub fn group(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(Structure::Group, attr, item)
}

fn is_known_law(name: &str) -> bool {
    matches!(
        name,
        "associativity"
            | "left_identity"
            | "right_identity"
            | "inverse_left"
            | "inverse_right"
            | "functor_identity"
            | "functor_composition"
            | "applicative_identity"
            | "applicative_composition"
            | "monad_left_identity"
            | "monad_right_identity"
            | "monad_associativity"
    )
}

#[proc_macro_attribute]
pub fn law(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_tokens: proc_macro2::TokenStream = item.clone().into();
    let laws: Punctuated<Ident, Token![,]> = match Punctuated::parse_terminated.parse(attr) {
        Ok(l) => l,
        Err(e) => {
            let err = e.to_compile_error();
            return TokenStream::from(quote! { #err #item_tokens });
        }
    };
    for law in &laws {
        let n = law.to_string();
        if !is_known_law(&n) {
            let err = syn::Error::new(law.span(), format!("unknown law `{n}`"));
            let err = err.to_compile_error();
            return TokenStream::from(quote! { #err #item_tokens });
        }
    }
    item
}

#[proc_macro_attribute]
pub fn ignore(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_tokens: proc_macro2::TokenStream = item.clone().into();
    let metas: Punctuated<MetaNameValue, Token![,]> = match Punctuated::parse_terminated.parse(attr) {
        Ok(m) => m,
        Err(e) => {
            let err = e.to_compile_error();
            return TokenStream::from(quote! { #err #item_tokens });
        }
    };
    let mut reason: Option<String> = None;
    let mut _note: Option<String> = None;
    for m in metas {
        let key: String = m
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        let value = str_value(&m.value);
        match key.as_str() {
            "reason" => {
                let v = value.ok_or_else(|| {
                    syn::Error::new_spanned(&m.value, "`reason` must be a string literal")
                });
                let v = match v {
                    Ok(v) => v,
                    Err(e) => {
                        let err = e.to_compile_error();
                        return TokenStream::from(quote! { #err #item_tokens });
                    }
                };
                match v.as_str() {
                    "intentional" | "debt" | "infeasible" => reason = Some(v),
                    _ => {
                        let err = syn::Error::new_spanned(
                            &m.value,
                            "`reason` must be \"intentional\", \"debt\", or \"infeasible\"",
                        );
                        let err = err.to_compile_error();
                        return TokenStream::from(quote! { #err #item_tokens });
                    }
                }
            }
            "note" => {
                let v = match value {
                    Some(v) => v,
                    None => {
                        let err = syn::Error::new_spanned(
                            &m.value,
                            "`note` must be a string literal",
                        );
                        let err = err.to_compile_error();
                        return TokenStream::from(quote! { #err #item_tokens });
                    }
                };
                _note = Some(v);
            }
            _ => {
                let err = syn::Error::new_spanned(
                    &m.path,
                    format!("unknown argument `{key}` for #[konpu::ignore]"),
                );
                let err = err.to_compile_error();
                return TokenStream::from(quote! { #err #item_tokens });
            }
        }
    }
    if reason.is_none() {
        let err = syn::Error::new(
            Span::call_site(),
            "#[konpu::ignore] requires `reason = \"<intentional|debt|infeasible>\"`",
        );
        let err = err.to_compile_error();
        return TokenStream::from(quote! { #err #item_tokens });
    }
    item
}