//!`Archive`/`RelOrd` derives for the block crate's archive traits. a derived
//!named-field struct `X` gets `XHandle<P>` (declaration order = comparison
//!order), a field-zipping `Archive<P>` impl (entry snapshot, rewind on Err)
//!and, under `derive(RelOrd)`, a short-circuiting `RelOrd<P>` impl.
//!v0: non-generic named-field structs whose field types implement the traits.
//!HANDLE RECORD LAW: handles are POD with EXPLICIT padding only — the derive
//!inserts `_pad_k: [u8; N]` wherever a P-INDEPENDENT gap exists and, for
//!fully-classified structs, pins layout with a monomorphized `const {}`
//!canary (boundary alignment + exact size); a violated canary is a COMPILE
//!error at the offending P width — a shape that cannot express zero padding
//!must not be used at that width. projection const-eval is dead on this
//!toolchain (unconstrained generic constants even under generic_const_exprs),
//!so known layouts are a closed table and unknown layouts disable the check.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as T2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Ident, Type, Visibility, parse_macro_input};

///handle layout class of a field type. known layouts are the closed table:
///inline primitives (Handle = Self) + StrHandle (align 8, size 16/24) + VecH
///(align P::BYTES, size 8/8/16/24); anything else is unknown.
enum Klass {
    ///inline primitive: Handle = Self
    Prim(u64, u64),
    StrH,
    VecH,
    Unk,
}

fn classify(ty: &Type) -> Klass {
    let Type::Path(tp) = ty else { return Klass::Unk };
    let Some(seg) = tp.path.segments.last() else { return Klass::Unk };
    match seg.ident.to_string().as_str() {
        "u8" | "i8" => Klass::Prim(1, 1),
        "u16" | "i16" => Klass::Prim(2, 2),
        "u32" | "i32" | "char" => Klass::Prim(4, 4),
        "u64" | "i64" | "usize" | "isize" => Klass::Prim(8, 8),
        "u128" | "i128" => Klass::Prim(16, 16),
        "String" => Klass::StrH,
        //genuine Vec<..> — a path-scoped Vec<T> without angle args is not ours
        "Vec" if matches!(seg.arguments, syn::PathArguments::AngleBracketed(_)) => Klass::VecH,
        _ => Klass::Unk,
    }
}

///static-offset tracker for pad planning: Exact = numeric offset known;
///Mult8 = offset is an unknown multiple of 8 (StrH/VecH tails align every
///≤8 boundary for free); Dead = no more static reasoning.
enum Off {
    Exact(u64),
    Mult8,
    Dead,
}

///per-field pads (before field k) + optional trailing pad, when every field
///layout is table-known; None when unknown fields disable planning.
fn plan(klasses: &[Klass]) -> Option<(Vec<Option<usize>>, Option<usize>)> {
    let mut pads: Vec<Option<usize>> = vec![None; klasses.len()];
    let mut amax = 1u64;
    let mut st = Off::Exact(0);
    for (i, k) in klasses.iter().enumerate() {
        match (k, &st) {
            (Klass::Unk, _) => return None,
            (Klass::Prim(sz, al), Off::Exact(o)) => {
                let gap = (al - o % al) % al;
                if gap > 0 {
                    pads[i] = Some(gap as usize);
                }
                amax = amax.max(*al);
                st = Off::Exact(o + gap + sz);
            }
            (Klass::Prim(sz, al), Off::Mult8) => {
                if *al > 8 {
                    //u128's gap depends on the unknown multiple — stops static
                    //reasoning; the canary still verifies per width
                    st = Off::Dead;
                } else {
                    //8-mult offsets are aligned for every align ≤ 8
                    st = if *sz % 8 == 0 { Off::Mult8 } else { Off::Dead };
                }
            }
            (Klass::Prim(_, _), Off::Dead) => {}
            (Klass::StrH, Off::Exact(o)) => {
                let gap = (8 - o % 8) % 8;
                if gap > 0 {
                    pads[i] = Some(gap as usize);
                }
                st = Off::Mult8;
            }
            (Klass::StrH, _) => {}
            (Klass::VecH, Off::Exact(o)) if o % 8 == 0 => st = Off::Mult8,
            (Klass::VecH, Off::Exact(_)) => st = Off::Dead, //align = P::BYTES
            (Klass::VecH, _) => {}
        }
    }
    //trailing pad only when the struct alignment is fully static ≥ 8 (the
    //zero-sized marker contributes align P::BYTES ≤ 8 at every width)
    let tail = match st {
        Off::Exact(o) if amax >= 8 => {
            let n = (amax - o % amax) % amax;
            (n > 0).then_some(n as usize)
        }
        _ => None,
    };
    Some((pads, tail))
}

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

///shared impl header: `P: Ptr` + per-field trait bounds.
fn impl_header(name: &Ident, types: &[Type], tr: &str) -> T2 {
    let path: syn::Path = syn::parse_str(&format!("::block::{tr}")).unwrap();
    let bounds = types.iter().map(|t| quote!(#t: #path<P>));
    quote!(impl<P: ::block::Ptr> #path<P> for #name where #(#bounds,)*)
}

fn archive_expansion(input: DeriveInput) -> T2 {
    let s = match spec(input) {
        Ok(s) => s,
        Err(e) => return e.to_compile_error(),
    };
    let Spec { vis, name, handle, fields, types } = s;

    let header = impl_header(&name, &types, "Archive");
    //HEAP = any field heaps: `f0::HEAP || f1::HEAP || false` (empty → false)
    let heap = types.iter().map(|t| quote!(<#t as ::block::Archive<P>>::HEAP ||));

    //layout plan: explicit pads + the canary's walk
    let klasses: Vec<Klass> = types.iter().map(classify).collect();
    let (pads, tail) = match plan(&klasses) {
        Some(p) => p,
        //unknown layouts: no reasoning possible — zero pads, zero checks
        None => (vec![None; fields.len()], None),
    };
    let classified = plan(&klasses).is_some();

    //declaration order; every Err rewinds to the entry snapshot (atomic)
    let archive_fields = fields.iter().zip(&types).map(|(f, t)| {
        let v = format_ident!("h_{}", f);
        quote! {
            let #v = match <#t as ::block::Archive<P>>::archive(&k.#f, al) {
                Ok(v) => v,
                Err(e) => { al.rewind(snap); return Err(e); }
            };
        }
    }).collect::<Vec<_>>();
    let snapshot = (!fields.is_empty()).then(|| quote!(let snap = al.snapshot();));
    //field init + the explicit pads before/after them, zero-written
    let mut inits: Vec<T2> = Vec::new();
    for (i, (f, _)) in fields.iter().zip(&types).enumerate() {
        let v = format_ident!("h_{}", f);
        if let Some(n) = pads[i] {
            let pad = format_ident!("_pad_{}", i);
            inits.push(quote!(#pad: [0u8; #n]));
        }
        inits.push(quote!(#f: #v));
    }
    if let Some(n) = tail {
        inits.push(quote!(_tail: [0u8; #n]));
    }

    //monomorphized layout canary: walks the handle with REAL projected
    //handle sizes/aligns (inline consts evaluate per P) and fails the
    //compilation of any width whose layout leaves implicit padding
    let canary = classified.then(|| {
        let mut walk: Vec<T2> = Vec::new();
        let mut prev = format_ident!("off_start");
        let mut next = 0u32;
        walk.push(quote!(let #prev = 0usize;));
        for (i, (f, t)) in fields.iter().zip(&types).enumerate() {
            let off = format_ident!("off_{}", next);
            next += 1;
            let mut rhs = quote!(#prev);
            if let Some(n) = pads[i] {
                rhs = quote!(#rhs + #n);
            }
            let msg = format!("{}: implicit padding before field '{}'", handle, f);
            walk.push(quote!(let #off = #rhs;));
            walk.push(quote!(assert!(
                #off % ::core::mem::align_of::<<#t as ::block::Archive<P>>::Handle>() == 0,
                #msg
            );));
            prev = format_ident!("off_{}", next);
            walk.push(quote!(let #prev = #off
                + ::core::mem::size_of::<<#t as ::block::Archive<P>>::Handle>();));
        }
        let mut end = quote!(#prev);
        if let Some(n) = tail {
            end = quote!(#end + #n);
        }
        let msg = format!("{}: implicit trailing padding", handle);
        quote! {
            const {
                #(#walk)*
                assert!(#end == ::core::mem::size_of::<#handle<P>>(), #msg);
            }
        }
    });

    //declaration-order handle fields + explicit pads + the zero-sized marker
    //(`PhantomData<[P; 0]>` names P at zero size/width-free cost at the tail)
    let mut handle_fields: Vec<T2> = Vec::new();
    for (i, (f, t)) in fields.iter().zip(&types).enumerate() {
        if let Some(n) = pads[i] {
            let pad = format_ident!("_pad_{}", i);
            handle_fields.push(quote!(#pad: [u8; #n],));
        }
        handle_fields.push(quote!(pub #f: <#t as ::block::Archive<P>>::Handle,));
    }
    if let Some(n) = tail {
        handle_fields.push(quote!(_tail: [u8; #n],));
    }

    //children first: release in reverse declaration order
    let release_fields = fields.iter().zip(&types).rev().map(|(f, t)| {
        quote!(<#t as ::block::Archive<P>>::release(&h.#f, al);)
    });
    //hand-written: derive(Debug) emits projection bounds rustc cannot solve
    //(`H: Debug` where H is a generic impl's assoc type)
    let dbg_fields = fields.iter().map(|f| {
        quote!(.field(stringify!(#f), &self.#f))
    });

    let marker_init = quote!(_p: ::core::marker::PhantomData);
    quote! {
        #[automatically_derived]
        #[derive(Clone, Copy, PartialEq, Eq)]
        #[repr(C)]
        #vis struct #handle<P>
        where P: ::block::Ptr, #(#types: ::block::Archive<P>,)*
        {
            #(#handle_fields)*
            _p: ::core::marker::PhantomData<[P; 0]>,
        }

        #[automatically_derived]
        impl<P: ::block::Ptr> ::core::fmt::Debug for #handle<P> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(#handle))
                    #(#dbg_fields)*
                    .finish()
            }
        }

        #[automatically_derived]
        #header {
            type Handle = #handle<P>;
            const HEAP: bool = #(#heap)* false;

            fn archive(k: &Self, al: &mut ::block::Allocator<'_, P>)
                -> Result<Self::Handle, ::block::CapErr> {
                #canary
                #snapshot
                #(#archive_fields)*
                Ok(#handle { #(#inits,)* #marker_init })
            }

            fn release(h: &Self::Handle, al: &mut ::block::Allocator<'_, P>) {
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
            let ord = <#t as ::block::RelOrd<P>>::#m(&a.#f, &b.#f, heap);
            if ord != #ord::Equal { return ord; }
        }
    });
    quote! { #(#steps)* #ord::Equal }
}

fn rel_cmp_body(fields: &[Ident], types: &[Type]) -> T2 {
    let ord: syn::Path = syn::parse_quote!(::core::cmp::Ordering);
    let steps = fields.iter().zip(types).map(|(f, t)| {
        quote! {
            let ord = <#t as ::block::RelOrd<P>>::rel_cmp(&h.#f, &k.#f, heap);
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
            fn rel_cmp(h: &Self::Handle, k: &Self, heap: &::block::Heap<P>)
                -> ::core::cmp::Ordering {
                #cmp
            }

            fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &::block::Heap<P>)
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
