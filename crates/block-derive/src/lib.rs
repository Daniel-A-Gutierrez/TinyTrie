//!`Archive`/`RelOrd` derives for the block crate's archive traits. a derived
//!named-field struct `X` gets `XHandle` (declaration order = comparison
//!order), a field-zipping `Archive` impl (unwind-based atomicity: a failure
//!at field k releases fields k−1..0 and rolls the front back) and, under
//!`derive(RelOrd)`, a short-circuiting `RelOrd` impl.
//!v0: non-generic named-field structs whose field types implement the traits.
//!HANDLE RECORD LAW: handles are POD with EXPLICIT padding only — the layout
//!is fully concrete (no pointer-width generics), so the derive computes each
//!pad as a CONST EXPRESSION over the real `size_of`/`align_of` of the
//!projected handles and pins the result with a const canary: any implicit
//!padding the compiler would insert is a COMPILE error, not a dirty record.
//!the same proof backs the emitted `ZeroPad`/`ReprC` impls `Block::push`
//!gates on — every byte of the handle is a field or a computed pad.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as T2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Ident, Type, Visibility, parse_macro_input};

struct Spec {
    vis: Visibility,
    name: Ident,
    handle: Ident,
    fields: Vec<Ident>,
    types: Vec<Type>,
}

fn spec(input: DeriveInput) -> Result<Spec, syn::Error> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "block-derive v0: generic structs are not supported",
        ));
    }
    let fields = match &input.data {
        Data::Struct(d) => d.fields.clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "block-derive supports structs only",
            ))
        }
    };
    let Fields::Named(named) = fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "block-derive v0: named-field structs only (tuples have no Archive impl — wrap)",
        ));
    };
    let (fields, types): (Vec<Ident>, Vec<Type>) = named
        .named
        .into_iter()
        .map(|f| (f.ident.unwrap(), f.ty))
        .unzip();
    let name = input.ident;
    Ok(Spec {
        vis: input.vis,
        handle: format_ident!("{}Handle", name),
        name,
        fields,
        types,
    })
}

///shared impl header: per-field trait bounds.
fn impl_header(name: &Ident, types: &[Type], tr: &str) -> T2 {
    let path: syn::Path = syn::parse_str(&format!("::block::{tr}")).unwrap();
    let bounds = types.iter().map(|t| quote!(#t: #path));
    quote!(impl #path for #name where #(#bounds,)*)
}

fn archive_expansion(input: DeriveInput) -> T2 {
    let s = match spec(input) {
        Ok(s) => s,
        Err(e) => return e.to_compile_error(),
    };
    let Spec { vis, name, handle, fields, types } = s;

    let header = impl_header(&name, &types, "Archive");
    //HEAP = any field heaps: `f0::HEAP || f1::HEAP || false` (empty → false)
    let heap = types.iter().map(|t| quote!(<#t as ::block::Archive>::HEAP ||));

    //const-expressions layout: the running byte offset and each pad are
    //computed from the REAL size_of/align_of of the projected handles — no
    //macro-side layout table, works for any (nested, derived, manual) handle
    let mut off = quote!(0usize);
    let mut amax = quote!(1usize);
    let mut pad_exprs: Vec<T2> = Vec::new();
    for t in &types {
        let h = quote!(<#t as ::block::Archive>::Handle);
        let align = quote!(::core::mem::align_of::<#h>());
        let pad = quote!((#align - #off % #align) % #align);
        pad_exprs.push(pad.clone());
        //const-callable max (Ord::max is not const on this toolchain)
        amax = quote!(if #amax > #align { #amax } else { #align });
        off = quote!(#off + #pad + ::core::mem::size_of::<#h>());
    }
    //tail pad to the struct alignment
    let tail_pad = quote!((#amax - #off % #amax) % #amax);
    let total = quote!(#off + #tail_pad);

    //const canary: the compiler's layout must equal the computed one — any
    //implicit padding is a COMPILE error here, which is also the proof
    //behind the ZeroPad/ReprC claims below (the align max rides in through
    //the tail pad, so the size check alone proves the layout)
    let canary = {
        let msg = format!("{}: implicit padding in the handle layout", handle);
        quote! {
            const _: () = assert!(
                ::core::mem::size_of::<#handle>() == #total,
                #msg
            );
        }
    };

    //declaration order; each field archives into a local, and a failure at
    //field k unwinds everything before it: releases of fields k−1..0 in
    //reverse (children first), then the front rolls back to the entry
    //position — unwind-based atomicity, no snapshot
    let heap0 = (!fields.is_empty()).then(|| quote!(let heap0 = al.heap_len();));
    let mut archive_fields: Vec<T2> = Vec::new();
    for (k, (f, t)) in fields.iter().zip(&types).enumerate() {
        let v = format_ident!("h_{}", f);
        let undo: Vec<T2> = fields[..k]
            .iter()
            .zip(&types[..k])
            .rev()
            .map(|(u, ut)| {
                let h = format_ident!("h_{}", u);
                quote!(<#ut as ::block::Archive>::release(&#h, al);)
            })
            .collect();
        archive_fields.push(quote! {
            let #v = match <#t as ::block::Archive>::archive(&k.#f, al) {
                Ok(v) => v,
                Err(e) => {
                    #(#undo)*
                    al.rollback(heap0);
                    return Err(e);
                }
            };
        });
    }
    //field init + the computed pads before them, zero-written
    let mut inits: Vec<T2> = Vec::new();
    for (i, (f, _)) in fields.iter().zip(&types).enumerate() {
        let v = format_ident!("h_{}", f);
        let pad = format_ident!("_pad_{}", i);
        let n = &pad_exprs[i];
        inits.push(quote!(#pad: [0u8; #n]));
        inits.push(quote!(#f: #v));
    }
    inits.push(quote!(_tail: [0u8; #tail_pad]));

    //the canary proves every byte is a field or a computed pad — the marker
    //claims are exactly what it proves
    let markers = quote! {
        // SAFETY: the canary above proves the layout holds no implicit padding.
        unsafe impl ::block::ZeroPad for #handle {}
        // SAFETY: declared #[repr(C)] below.
        unsafe impl ::block::ReprC for #handle {}
    };

    //declaration-order handle fields + computed pads
    let mut handle_fields: Vec<T2> = Vec::new();
    for (i, (f, t)) in fields.iter().zip(&types).enumerate() {
        let pad = format_ident!("_pad_{}", i);
        let n = &pad_exprs[i];
        handle_fields.push(quote!(#pad: [u8; #n],));
        handle_fields.push(quote!(pub #f: <#t as ::block::Archive>::Handle,));
    }
    handle_fields.push(quote!(_tail: [u8; #tail_pad],));

    //children first: release in reverse declaration order
    let release_fields = fields.iter().zip(&types).rev().map(|(f, t)| {
        quote!(<#t as ::block::Archive>::release(&h.#f, al);)
    });
    //hand-written: derive(Debug) emits projection bounds rustc cannot solve
    //(`H: Debug` where H is a generic impl's assoc type)
    let dbg_fields = fields.iter().map(|f| {
        quote!(.field(stringify!(#f), &self.#f))
    });

    quote! {
        #[automatically_derived]
        #[derive(Clone, Copy, PartialEq, Eq)]
        #[repr(C)]
        #vis struct #handle {
            #(#handle_fields)*
        }

        #[automatically_derived]
        impl ::core::fmt::Debug for #handle {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(#handle))
                    #(#dbg_fields)*
                    .finish()
            }
        }

        #markers

        #[automatically_derived]
        #header {
            type Handle = #handle;
            const HEAP: bool = #(#heap)* false;

            fn archive(k: &Self, al: &mut ::block::Allocator<'_>)
                -> Result<Self::Handle, ::block::CapErr> {
                #canary
                #heap0
                #(#archive_fields)*
                Ok(#handle { #(#inits,)* })
            }

            fn release(h: &Self::Handle, al: &mut ::block::Allocator<'_>) {
                #(#release_fields)*
            }
        }
    }
}

///zip against the handle fields: short-circuit at the first non-Equal, no
///view construction anywhere.
fn cmp_body(fields: &[Ident], types: &[Type], meth: &str) -> T2 {
    let ord: syn::Path = syn::parse_quote!(::core::cmp::Ordering);
    let steps = fields.iter().zip(types).map(|(f, t)| {
        let m = format_ident!("{}", meth);
        quote! {
            let ord = <#t as ::block::RelOrd>::#m(&a.#f, &b.#f, heap);
            if ord != #ord::Equal { return ord; }
        }
    });
    quote! { #(#steps)* #ord::Equal }
}

fn rel_cmp_body(fields: &[Ident], types: &[Type]) -> T2 {
    let ord: syn::Path = syn::parse_quote!(::core::cmp::Ordering);
    let steps = fields.iter().zip(types).map(|(f, t)| {
        quote! {
            let ord = <#t as ::block::RelOrd>::rel_cmp(&h.#f, &k.#f, heap);
            if ord != #ord::Equal { return ord; }
        }
    });
    quote! { #(#steps)* #ord::Equal }
}

fn rel_ord_expansion(input: DeriveInput) -> T2 {
    let s = match spec(input) {
        Ok(s) => s,
        Err(e) => return e.to_compile_error(),
    };
    let Spec { name, fields, types, .. } = s;
    let header = impl_header(&name, &types, "RelOrd");
    let cmp_h = cmp_body(&fields, &types, "rel_cmp_h");
    let cmp = rel_cmp_body(&fields, &types);
    quote! {
        #[automatically_derived]
        #header {
            fn rel_cmp(h: &Self::Handle, k: &Self, heap: &::block::Heap<'_>)
                -> ::core::cmp::Ordering {
                #cmp
            }

            fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &::block::Heap<'_>)
                -> ::core::cmp::Ordering {
                #cmp_h
            }
        }
    }
}

#[proc_macro_derive(Archive)]
pub fn archive(input: TokenStream) -> TokenStream {
    archive_expansion(parse_macro_input!(input as DeriveInput)).into()
}

#[proc_macro_derive(RelOrd)]
pub fn rel_ord(input: TokenStream) -> TokenStream {
    rel_ord_expansion(parse_macro_input!(input as DeriveInput)).into()
}