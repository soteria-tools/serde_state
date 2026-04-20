use crate::{
    attrs::ItemMode,
    dummy,
    type_decl::{
        EnumDecl, FieldDecl, FieldsDecl, FieldsStyle, StructDecl, TypeData, TypeDecl, VariantDecl,
    },
};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, GenericParam, Generics, Type, parse_quote};

pub fn expand_derive_deserialize(input: &DeriveInput) -> syn::Result<TokenStream> {
    if let Data::Union(u) = &input.data {
        return Err(syn::Error::new(
            u.union_token.span(),
            "DeserializeState does not support unions",
        ));
    }

    let decl = TypeDecl::from_derive_input(input)?;
    let impl_block = match &decl.data {
        TypeData::Struct(data) => derive_struct(&decl, data)?,
        TypeData::Enum(data) => derive_enum(&decl, data)?,
    };

    Ok(dummy::wrap_in_const(
        decl.attrs.serde_path.as_ref(),
        impl_block,
    ))
}

fn derive_struct(decl: &TypeDecl, data: &StructDecl) -> syn::Result<TokenStream> {
    let has_explicit_state = decl.attrs.state.is_some();
    let has_state_bound = decl.attrs.state_bound.is_some();
    let uses_generic_state = !has_explicit_state;
    let infer_bounds = !has_explicit_state && !has_state_bound;
    let impl_generics_with_state = add_state_param(
        decl.generics,
        uses_generic_state,
        decl.attrs.state_bound.as_ref(),
    );
    let (impl_generics_ref, _, _) = impl_generics_with_state.split_for_impl();
    let impl_generics = quote!(#impl_generics_ref);
    let (_, ty_generics_ref, _) = decl.generics.split_for_impl();
    let ty_generics = quote!(#ty_generics_ref);
    let mut where_clause = decl.generics.where_clause.clone();
    let state_tokens = state_type_tokens(decl);
    let field_types = collect_field_types_from_fields(&data.fields);
    let explicit_state = decl.attrs.state.as_ref();
    if infer_bounds {
        add_deserialize_bounds_from_types(&mut where_clause, &field_types, &state_tokens);
    } else {
        add_deserialize_bounds_from_type_params(
            &mut where_clause,
            decl.generics,
            &state_tokens,
            decl.attrs.mode,
        );
    }
    add_default_bounds_for_skipped(&data.fields, &mut where_clause);
    let where_clause_tokens = quote_where_clause(&where_clause);
    let ident = decl.ident;

    let body = if decl.attrs.transparent {
        deserialize_transparent(ident, &data.fields, &state_tokens)?
    } else {
        deserialize_struct_body(
            ident,
            &data.fields,
            &state_tokens,
            explicit_state,
            decl.generics,
            uses_generic_state,
            decl.attrs.state_bound.as_ref(),
            &where_clause,
        )
    };
    let default_deser_impl = default_deserialize_impl(decl, ident);

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics _serde_state::DeserializeState<'de, #state_tokens> for #ident #ty_generics #where_clause_tokens {
            fn deserialize_state<__D>(
                __state: &#state_tokens,
                __deserializer: __D,
            ) -> ::core::result::Result<Self, __D::Error>
            where
                __D: _serde::Deserializer<'de>,
            {
                #body
            }
        }

        #default_deser_impl
    })
}

fn derive_enum(decl: &TypeDecl, data: &EnumDecl) -> syn::Result<TokenStream> {
    let has_explicit_state = decl.attrs.state.is_some();
    let has_state_bound = decl.attrs.state_bound.is_some();
    let uses_generic_state = !has_explicit_state;
    let infer_bounds = !has_explicit_state && !has_state_bound;
    let impl_generics_with_state = add_state_param(
        decl.generics,
        uses_generic_state,
        decl.attrs.state_bound.as_ref(),
    );
    let (impl_generics_ref, _, _) = impl_generics_with_state.split_for_impl();
    let impl_generics = quote!(#impl_generics_ref);
    let (_, ty_generics_ref, _) = decl.generics.split_for_impl();
    let ty_generics = quote!(#ty_generics_ref);
    let mut where_clause = decl.generics.where_clause.clone();
    let state_tokens = state_type_tokens(decl);
    let field_types = collect_field_types_from_enum(data);
    let explicit_state = decl.attrs.state.as_ref();
    if infer_bounds {
        add_deserialize_bounds_from_types(&mut where_clause, &field_types, &state_tokens);
    } else {
        add_deserialize_bounds_from_type_params(
            &mut where_clause,
            decl.generics,
            &state_tokens,
            decl.attrs.mode,
        );
    }
    for variant in &data.variants {
        add_default_bounds_for_skipped(&variant.fields, &mut where_clause);
    }
    let where_clause_tokens = quote_where_clause(&where_clause);
    let ident = decl.ident;

    let body = deserialize_enum_body(
        ident,
        data,
        &state_tokens,
        explicit_state,
        decl.generics,
        uses_generic_state,
        decl.attrs.state_bound.as_ref(),
        &where_clause,
    );
    let default_deser_impl = default_deserialize_impl(decl, ident);

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics _serde_state::DeserializeState<'de, #state_tokens> for #ident #ty_generics #where_clause_tokens {
            fn deserialize_state<__D>(
                __state: &#state_tokens,
                __deserializer: __D,
            ) -> ::core::result::Result<Self, __D::Error>
            where
                __D: _serde::Deserializer<'de>,
            {
                #body
            }
        }

        #default_deser_impl
    })
}

fn deserialize_transparent(
    ident: &syn::Ident,
    fields: &FieldsDecl<'_>,
    state_tokens: &TokenStream,
) -> syn::Result<TokenStream> {
    match fields.style {
        FieldsStyle::Named if fields.fields.len() == 1 => {
            let field = &fields.fields[0];
            let field_ident = field.ident().unwrap();
            let ty = field.ty();
            if let Some(with) = &field.attrs.with {
                Ok(quote! {
                    let #field_ident: #ty = #with::deserialize_state(__state, __deserializer)?;
                    ::core::result::Result::Ok(#ident { #field_ident: #field_ident })
                })
            } else {
                Ok(match field.mode() {
                    ItemMode::Stateful => quote! {
                        let __seed = _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(__state);
                        let #field_ident = _serde::de::DeserializeSeed::deserialize(__seed, __deserializer)?;
                        ::core::result::Result::Ok(#ident { #field_ident: #field_ident })
                    },
                    ItemMode::Stateless => quote! {
                        let #field_ident: #ty = _serde::Deserialize::deserialize(__deserializer)?;
                        ::core::result::Result::Ok(#ident { #field_ident: #field_ident })
                    },
                })
            }
        }
        FieldsStyle::Unnamed if fields.fields.len() == 1 => {
            let field = &fields.fields[0];
            let ty = field.ty();
            if let Some(with) = &field.attrs.with {
                Ok(quote! {
                    let __value: #ty = #with::deserialize_state(__state, __deserializer)?;
                    ::core::result::Result::Ok(#ident(__value))
                })
            } else {
                Ok(match field.mode() {
                    ItemMode::Stateful => quote! {
                        let __seed = _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(__state);
                        let __value = _serde::de::DeserializeSeed::deserialize(__seed, __deserializer)?;
                        ::core::result::Result::Ok(#ident(__value))
                    },
                    ItemMode::Stateless => quote! {
                        let __value: #ty = _serde::Deserialize::deserialize(__deserializer)?;
                        ::core::result::Result::Ok(#ident(__value))
                    },
                })
            }
        }
        _ => Err(syn::Error::new(
            fields.span,
            "transparent structs must have exactly one field",
        )),
    }
}

fn deserialize_struct_body(
    ident: &syn::Ident,
    fields: &FieldsDecl<'_>,
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    match fields.style {
        FieldsStyle::Named => deserialize_named_struct(
            ident,
            &fields.fields,
            state_tokens,
            explicit_state,
            generics,
            include_state_param,
            state_bound,
            where_clause,
        ),
        FieldsStyle::Unnamed => deserialize_unnamed_struct(
            ident,
            &fields.fields,
            state_tokens,
            explicit_state,
            generics,
            include_state_param,
            state_bound,
            where_clause,
        ),
        FieldsStyle::Unit => deserialize_unit_struct(ident),
    }
}

fn deserialize_identifier_or_u32(deserializer: TokenStream, visitor: TokenStream) -> TokenStream {
    quote! {
        if _serde::Deserializer::is_human_readable(&#deserializer) {
            _serde::Deserializer::deserialize_identifier(#deserializer, #visitor)
        } else {
            _serde::Deserializer::deserialize_u32(#deserializer, #visitor)
        }
    }
}

fn seq_read_fields_body(
    fields: &[FieldDecl<'_>],
    included: &[&FieldDecl<'_>],
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    state_bound: Option<&Type>,
    construct: TokenStream,
) -> TokenStream {
    let included_len = included.len();
    let read_included = included.iter().enumerate().map(|(seq_index, field)| {
        let ident = field.ident().unwrap();
        let ty = field.ty();
        let idx = seq_index;
        if field.attrs.with.is_some() {
            let seed = with_deserialize_seed(field, explicit_state, state_bound);
            quote! {
                let __seed = #seed;
                let #ident = match _serde::de::SeqAccess::next_element_seed(&mut __seq, __seed)? {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                };
            }
        } else {
            match field.mode() {
                ItemMode::Stateful => quote! {
                    let __seed = _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(state);
                    let #ident = match _serde::de::SeqAccess::next_element_seed(&mut __seq, __seed)? {
                        ::core::option::Option::Some(value) => value,
                        ::core::option::Option::None =>
                            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                    };
                },
                ItemMode::Stateless => quote! {
                    let #ident = match _serde::de::SeqAccess::next_element::<#ty>(&mut __seq)? {
                        ::core::option::Option::Some(value) => value,
                        ::core::option::Option::None =>
                            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                    };
                },
            }
        }
    });
    let init_skipped = fields.iter().filter(|field| field.attrs.skip).map(|field| {
        let ident = field.ident().unwrap();
        quote! {
            let #ident = ::core::default::Default::default();
        }
    });
    quote! {
        let state = self.state;
        #(#read_included)*
        if let ::core::option::Option::Some(_) =
            _serde::de::SeqAccess::next_element::<_serde::de::IgnoredAny>(&mut __seq)?
        {
            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#included_len + 1, &self));
        }
        #(#init_skipped)*
        ::core::result::Result::Ok(#construct)
    }
}

fn deserialize_named_struct(
    ident: &syn::Ident,
    fields: &[FieldDecl<'_>],
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let included: Vec<_> = fields.iter().filter(|field| !field.attrs.skip).collect();

    let field_names: Vec<String> = included
        .iter()
        .map(|field| field.attrs.key(field.ident().unwrap()))
        .collect();

    let field_variants: Vec<_> = included
        .iter()
        .map(|field| {
            let name = field.ident().unwrap().to_string();
            format_ident!("__field_{}", name)
        })
        .collect();

    let const_fields = {
        let names = field_names.iter();
        quote! {
            const __FIELDS: &'static [&'static str] = &[#(#names),*];
        }
    };

    let field_enum = {
        let variants = field_variants.iter();
        quote! {
            #[allow(non_camel_case_types)]
            enum __Field { #(#variants,)* __Ignore }
        }
    };

    let field_visitor = {
        let deserialize_field =
            deserialize_identifier_or_u32(quote!(deserializer), quote!(__FieldVisitor));
        let match_arms = field_names
            .iter()
            .zip(field_variants.iter())
            .map(|(name, variant)| {
                quote! { #name => ::core::result::Result::Ok(__Field::#variant) }
            });
        let index_match_arms = field_variants.iter().enumerate().map(|(index, variant)| {
            let index = index as u64;
            quote! { #index => ::core::result::Result::Ok(__Field::#variant) }
        });
        quote! {
            struct __FieldVisitor;
            impl<'de> _serde::de::Visitor<'de> for __FieldVisitor {
                type Value = __Field;

                fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    formatter.write_str("field name")
                }

                fn visit_str<E>(self, value: &str) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    match value {
                        #(#match_arms,)*
                        _ => ::core::result::Result::Ok(__Field::__Ignore),
                    }
                }

                fn visit_u64<E>(self, value: u64) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    match value {
                        #(#index_match_arms,)*
                        _ => ::core::result::Result::Ok(__Field::__Ignore),
                    }
                }

                fn visit_u32<E>(self, value: u32) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    self.visit_u64(value as u64)
                }
            }

            impl<'de> _serde::Deserialize<'de> for __Field {
                fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
                where
                    D: _serde::Deserializer<'de>,
                {
                    #deserialize_field
                }
            }
        }
    };

    let init_locals = fields.iter().map(|field| {
        let ident = field.ident().unwrap();
        if field.attrs.skip {
            quote!()
        } else {
            quote!(let mut #ident = ::core::option::Option::None;)
        }
    });

    let construct = {
        let pairs = fields.iter().map(|field| {
            let ident = field.ident().unwrap();
            quote!(#ident: #ident)
        });
        quote!(#ident { #(#pairs),* })
    };

    let seq_read_fields = seq_read_fields_body(
        fields,
        &included,
        state_tokens,
        explicit_state,
        state_bound,
        construct.clone(),
    );

    let match_arms = included
        .iter()
        .zip(field_variants.iter())
        .map(|(field, variant)| {
            let ident = field.ident().unwrap();
            let name = field.attrs.key(ident);
            let ty = field.ty();
            let assignment = if field.attrs.with.is_some() {
                let seed = with_deserialize_seed(field, explicit_state, state_bound);
                quote! {
                    let __seed = #seed;
                    #ident = ::core::option::Option::Some(
                        _serde::de::MapAccess::next_value_seed(&mut __map, __seed)?,
                    );
                }
            } else {
                match field.mode() {
                    ItemMode::Stateful => quote! {
                        let __seed = _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(state);
                        #ident = ::core::option::Option::Some(
                            _serde::de::MapAccess::next_value_seed(&mut __map, __seed)?,
                        );
                    },
                    ItemMode::Stateless => quote! {
                        #ident = ::core::option::Option::Some(
                            _serde::de::MapAccess::next_value::<#ty>(&mut __map)?,
                        );
                    },
                }
            };
            quote! {
                __Field::#variant => {
                    if #ident.is_some() {
                        return ::core::result::Result::Err(_serde::de::Error::duplicate_field(#name));
                    }
                    #assignment
                }
            }
        });

    let build_fields = fields.iter().map(|field| {
        let ident = field.ident().unwrap();
        if field.attrs.skip {
            quote! {
                let #ident = ::core::default::Default::default();
            }
        } else {
            let name = field.attrs.key(ident);
            quote! {
                let #ident = match #ident {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::missing_field(#name)),
                };
            }
        }
    });

    let (visitor_struct_generics, _) =
        visitor_struct_generics_tokens(generics, include_state_param, state_bound);
    let (visitor_impl_generics, visitor_impl_type_generics) =
        visitor_impl_generics_tokens(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = generics.split_for_impl();
    let phantom_type = phantom_type(ident, generics);

    let visitor_struct = quote! {
        struct __Visitor #visitor_struct_generics {
            state: &'state #state_tokens,
            _marker: ::core::marker::PhantomData<#phantom_type>,
        }
    };

    let visitor_where_clause = quote_where_clause(where_clause);
    let visitor_impl = quote! {
        impl #visitor_impl_generics _serde::de::Visitor<'de> for __Visitor #visitor_impl_type_generics #visitor_where_clause {
            type Value = #ident #ty_generics;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("struct ")?;
                formatter.write_str(stringify!(#ident))
            }

            fn visit_map<__M>(self, mut __map: __M) -> ::core::result::Result<Self::Value, __M::Error>
            where
                __M: _serde::de::MapAccess<'de>,
            {
                let state = self.state;
                #(#init_locals)*
                while let ::core::option::Option::Some(__key) =
                    _serde::de::MapAccess::next_key::<__Field>(&mut __map)?
                {
                    match __key {
                        #(#match_arms)*
                        __Field::__Ignore => {
                            let _ = _serde::de::MapAccess::next_value::<_serde::de::IgnoredAny>(&mut __map)?;
                        }
                    }
                }
                #(#build_fields)*
                ::core::result::Result::Ok(#construct)
            }

            fn visit_seq<__A>(self, mut __seq: __A) -> ::core::result::Result<Self::Value, __A::Error>
            where
                __A: _serde::de::SeqAccess<'de>,
            {
                #seq_read_fields
            }
        }
    };

    quote! {
        #const_fields
        #field_enum
        #field_visitor

        #visitor_struct

        #visitor_impl

        _serde::Deserializer::deserialize_struct(
            __deserializer,
            stringify!(#ident),
            __FIELDS,
            __Visitor {
                state: __state,
                _marker: ::core::marker::PhantomData,
            },
        )
    }
}

fn deserialize_unnamed_struct(
    ident: &syn::Ident,
    fields: &[FieldDecl<'_>],
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    match fields.len() {
        0 => deserialize_unit_struct(ident),
        1 => {
            let field = &fields[0];
            deserialize_newtype_struct(
                ident,
                field,
                state_tokens,
                explicit_state,
                generics,
                include_state_param,
                state_bound,
                where_clause,
            )
        }
        _ => deserialize_tuple_struct(
            ident,
            fields,
            state_tokens,
            explicit_state,
            generics,
            include_state_param,
            state_bound,
            where_clause,
        ),
    }
}

fn deserialize_newtype_struct(
    ident: &syn::Ident,
    field: &FieldDecl<'_>,
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let field_ty = field.ty();
    let (visitor_struct_generics, _) =
        visitor_struct_generics_tokens(generics, include_state_param, state_bound);
    let (visitor_impl_generics, visitor_impl_type_generics) =
        visitor_impl_generics_tokens(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = generics.split_for_impl();
    let phantom_type = phantom_type(ident, generics);
    let field_mode = field.mode();

    let newtype_body = if let Some(with) = &field.attrs.with {
        quote! {
            let state = self.state;
            let __value: #field_ty = #with::deserialize_state(state, __deserializer)?;
            ::core::result::Result::Ok(#ident(__value))
        }
    } else {
        match field_mode {
            ItemMode::Stateful => quote! {
                let state = self.state;
                let __seed = _serde_state::__private::wrap_deserialize_seed::<#field_ty, #state_tokens>(state);
                let __value = _serde::de::DeserializeSeed::deserialize(__seed, __deserializer)?;
                ::core::result::Result::Ok(#ident(__value))
            },
            ItemMode::Stateless => quote! {
                let __value: #field_ty = _serde::Deserialize::deserialize(__deserializer)?;
                ::core::result::Result::Ok(#ident(__value))
            },
        }
    };

    let seq_body = if field.attrs.with.is_some() {
        let seed = with_deserialize_seed(field, explicit_state, state_bound);
        quote! {
            let state = self.state;
            let __seed = #seed;
            let __value = match _serde::de::SeqAccess::next_element_seed(&mut __seq, __seed)? {
                ::core::option::Option::Some(value) => value,
                ::core::option::Option::None =>
                    return ::core::result::Result::Err(_serde::de::Error::invalid_length(0, &self)),
            };
            if _serde::de::SeqAccess::next_element::<_serde::de::IgnoredAny>(&mut __seq)?.is_some() {
                return ::core::result::Result::Err(_serde::de::Error::invalid_length(1, &self));
            }
            ::core::result::Result::Ok(#ident(__value))
        }
    } else {
        match field_mode {
            ItemMode::Stateful => quote! {
                let state = self.state;
                let __seed = _serde_state::__private::wrap_deserialize_seed::<#field_ty, #state_tokens>(state);
                let __value = match _serde::de::SeqAccess::next_element_seed(&mut __seq, __seed)? {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::invalid_length(0, &self)),
                };
                if _serde::de::SeqAccess::next_element::<_serde::de::IgnoredAny>(&mut __seq)?.is_some() {
                    return ::core::result::Result::Err(_serde::de::Error::invalid_length(1, &self));
                }
                ::core::result::Result::Ok(#ident(__value))
            },
            ItemMode::Stateless => quote! {
                let __value = match _serde::de::SeqAccess::next_element::<#field_ty>(&mut __seq)? {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::invalid_length(0, &self)),
                };
                if _serde::de::SeqAccess::next_element::<_serde::de::IgnoredAny>(&mut __seq)?.is_some() {
                    return ::core::result::Result::Err(_serde::de::Error::invalid_length(1, &self));
                }
                ::core::result::Result::Ok(#ident(__value))
            },
        }
    };

    let visitor_struct = quote! {
        struct __Visitor #visitor_struct_generics {
            state: &'state #state_tokens,
            _marker: ::core::marker::PhantomData<#phantom_type>,
        }
    };

    let visit_body = quote! {
        fn visit_newtype_struct<__E>(
            self,
            __deserializer: __E,
        ) -> ::core::result::Result<Self::Value, __E::Error>
        where
            __E: _serde::Deserializer<'de>,
        {
            #newtype_body
        }

        fn visit_seq<__A>(
            self,
            mut __seq: __A,
        ) -> ::core::result::Result<Self::Value, __A::Error>
        where
            __A: _serde::de::SeqAccess<'de>,
        {
            #seq_body
        }
    };

    let visitor_where_clause = quote_where_clause(where_clause);
    let visitor_impl = quote! {
        impl #visitor_impl_generics _serde::de::Visitor<'de> for __Visitor #visitor_impl_type_generics #visitor_where_clause {
            type Value = #ident #ty_generics;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("newtype struct ")?;
                formatter.write_str(stringify!(#ident))
            }

            #visit_body
        }
    };

    quote! {
        #visitor_struct
        #visitor_impl

        _serde::Deserializer::deserialize_newtype_struct(
            __deserializer,
            stringify!(#ident),
            __Visitor {
                state: __state,
                _marker: ::core::marker::PhantomData,
            },
        )
    }
}

fn deserialize_tuple_struct(
    ident: &syn::Ident,
    fields: &[FieldDecl<'_>],
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let len = fields.len();
    let bindings: Vec<_> = (0..len).map(|i| format_ident!("__field_{}", i)).collect();
    let read_fields = fields.iter().enumerate().map(|(index, field)| {
        let binding = &bindings[index];
        let ty = field.ty();
        let idx = index;
        if field.attrs.with.is_some() {
            let seed = with_deserialize_seed(field, explicit_state, state_bound);
            quote! {
                let __seed = #seed;
                let #binding = match _serde::de::SeqAccess::next_element_seed(&mut __seq, __seed)? {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                };
            }
        } else {
            match field.mode() {
                ItemMode::Stateful => quote! {
                    let #binding = match _serde::de::SeqAccess::next_element_seed(
                        &mut __seq,
                        _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(state),
                    )? {
                        ::core::option::Option::Some(value) => value,
                        ::core::option::Option::None =>
                            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                    };
                },
                ItemMode::Stateless => quote! {
                    let #binding = match _serde::de::SeqAccess::next_element::<#ty>(&mut __seq)? {
                        ::core::option::Option::Some(value) => value,
                        ::core::option::Option::None =>
                            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                    };
                },
            }
        }
    });

    let construct = quote!(#ident(#(#bindings),*));
    let (visitor_struct_generics, _) =
        visitor_struct_generics_tokens(generics, include_state_param, state_bound);
    let (visitor_impl_generics, visitor_impl_type_generics) =
        visitor_impl_generics_tokens(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = generics.split_for_impl();
    let phantom_type = phantom_type(ident, generics);

    let visitor_struct = quote! {
        struct __Visitor #visitor_struct_generics {
            state: &'state #state_tokens,
            _marker: ::core::marker::PhantomData<#phantom_type>,
        }
    };

    let visit_body = quote! {
        fn visit_seq<__A>(
            self,
            mut __seq: __A,
        ) -> ::core::result::Result<Self::Value, __A::Error>
        where
            __A: _serde::de::SeqAccess<'de>,
        {
            let state = self.state;
            #(#read_fields)*
            ::core::result::Result::Ok(#construct)
        }
    };

    let visitor_where_clause = quote_where_clause(where_clause);
    let visitor_impl = quote! {
        impl #visitor_impl_generics _serde::de::Visitor<'de> for __Visitor #visitor_impl_type_generics #visitor_where_clause {
            type Value = #ident #ty_generics;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("tuple struct ")?;
                formatter.write_str(stringify!(#ident))
            }

            #visit_body
        }
    };

    quote! {
        #visitor_struct
        #visitor_impl

        _serde::Deserializer::deserialize_tuple_struct(
            __deserializer,
            stringify!(#ident),
            #len,
            __Visitor {
                state: __state,
                _marker: ::core::marker::PhantomData,
            },
        )
    }
}

fn deserialize_unit_struct(ident: &syn::Ident) -> TokenStream {
    quote! {
        struct __Visitor;
        impl<'de> _serde::de::Visitor<'de> for __Visitor {
            type Value = #ident;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("unit struct ")?;
                formatter.write_str(stringify!(#ident))
            }

            fn visit_unit<E>(self) -> ::core::result::Result<Self::Value, E>
            where
                E: _serde::de::Error,
            {
                ::core::result::Result::Ok(#ident)
            }
        }

        _serde::Deserializer::deserialize_unit_struct(
            __deserializer,
            stringify!(#ident),
            __Visitor,
        )
    }
}

fn deserialize_enum_body(
    ident: &syn::Ident,
    data: &EnumDecl<'_>,
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let variant_names: Vec<_> = data
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect();
    let variant_idents: Vec<_> = data.variants.iter().map(|variant| variant.ident).collect();

    let const_variants = {
        let names = variant_names.iter();
        quote! {
            const __VARIANTS: &'static [&'static str] = &[#(#names),*];
        }
    };

    let variant_enum = {
        let variants = variant_idents.iter();
        quote! {
            #[allow(non_camel_case_types)]
            enum __Variant { #(#variants),* }
        }
    };

    let variant_visitor = {
        let deserialize_variant =
            deserialize_identifier_or_u32(quote!(deserializer), quote!(__VariantVisitor));
        let match_arms = variant_names
            .iter()
            .zip(variant_idents.iter())
            .map(|(name, ident)| {
                quote! { #name => ::core::result::Result::Ok(__Variant::#ident) }
            });
        let index_match_arms = variant_idents.iter().enumerate().map(|(index, ident)| {
            let index = index as u64;
            quote! { #index => ::core::result::Result::Ok(__Variant::#ident) }
        });
        quote! {
            struct __VariantVisitor;
            impl<'de> _serde::de::Visitor<'de> for __VariantVisitor {
                type Value = __Variant;

                fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    formatter.write_str("variant identifier")
                }

                fn visit_str<E>(self, value: &str) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    match value {
                        #(#match_arms,)*
                        _ => ::core::result::Result::Err(_serde::de::Error::unknown_variant(value, __VARIANTS)),
                    }
                }

                fn visit_u64<E>(self, value: u64) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    match value {
                        #(#index_match_arms,)*
                        _ => ::core::result::Result::Err(_serde::de::Error::invalid_value(
                            _serde::de::Unexpected::Unsigned(value),
                            &self,
                        )),
                    }
                }

                fn visit_u32<E>(self, value: u32) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    self.visit_u64(value as u64)
                }
            }

            impl<'de> _serde::Deserialize<'de> for __Variant {
                fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
                where
                    D: _serde::Deserializer<'de>,
                {
                    #deserialize_variant
                }
            }
        }
    };

    let mut helper_tokens = Vec::new();
    let variant_match_arms = data.variants.iter().enumerate().map(|(index, variant)| {
        deserialize_enum_variant_arm(
            ident,
            variant,
            state_tokens,
            explicit_state,
            generics,
            include_state_param,
            state_bound,
            index,
            &mut helper_tokens,
            where_clause,
        )
    });

    let (visitor_struct_generics, _) =
        visitor_struct_generics_tokens(generics, include_state_param, state_bound);
    let (visitor_impl_generics, visitor_impl_type_generics) =
        visitor_impl_generics_tokens(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = generics.split_for_impl();
    let phantom_type = phantom_type(ident, generics);

    let visitor_struct = quote! {
        struct __Visitor #visitor_struct_generics {
            state: &'state #state_tokens,
            _marker: ::core::marker::PhantomData<#phantom_type>,
        }
    };

    let visitor_where_clause = quote_where_clause(where_clause);
    let visitor_impl = quote! {
        impl #visitor_impl_generics _serde::de::Visitor<'de> for __Visitor #visitor_impl_type_generics #visitor_where_clause {
            type Value = #ident #ty_generics;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("enum ")?;
                formatter.write_str(stringify!(#ident))
            }

            fn visit_enum<__E>(
                self,
                __enum: __E,
            ) -> ::core::result::Result<Self::Value, __E::Error>
            where
                __E: _serde::de::EnumAccess<'de>,
            {
                let state = self.state;
                match _serde::de::EnumAccess::variant::<__Variant>(__enum)? {
                    #(#variant_match_arms)*
                }
            }
        }
    };

    quote! {
        #const_variants
        #variant_enum
        #variant_visitor
        #(#helper_tokens)*
        #visitor_struct
        #visitor_impl

        _serde::Deserializer::deserialize_enum(
            __deserializer,
            stringify!(#ident),
            __VARIANTS,
            __Visitor {
                state: __state,
                _marker: ::core::marker::PhantomData,
            },
        )
    }
}

fn deserialize_enum_variant_arm(
    ident: &syn::Ident,
    variant: &VariantDecl<'_>,
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    index: usize,
    helpers: &mut Vec<TokenStream>,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let variant_ident = variant.ident;
    match variant.fields.style {
        FieldsStyle::Unit => {
            quote! {
                (__Variant::#variant_ident, __variant) => {
                    _serde::de::VariantAccess::unit_variant(__variant)?;
                    ::core::result::Result::Ok(#ident::#variant_ident)
                }
            }
        }
        FieldsStyle::Unnamed if variant.fields.fields.len() == 1 => {
            let field = &variant.fields.fields[0];
            let ty = field.ty();
            if field.attrs.with.is_some() {
                let seed = with_deserialize_seed(field, explicit_state, state_bound);
                quote! {
                    (__Variant::#variant_ident, __variant) => {
                        let __seed = #seed;
                        let __value = _serde::de::VariantAccess::newtype_variant_seed(__variant, __seed)?;
                        ::core::result::Result::Ok(#ident::#variant_ident(__value))
                    }
                }
            } else {
                match field.mode() {
                    ItemMode::Stateful => quote! {
                        (__Variant::#variant_ident, __variant) => {
                            let __seed = _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(state);
                            let __value = _serde::de::VariantAccess::newtype_variant_seed(__variant, __seed)?;
                            ::core::result::Result::Ok(#ident::#variant_ident(__value))
                        }
                    },
                    ItemMode::Stateless => quote! {
                        (__Variant::#variant_ident, __variant) => {
                            let __value: #ty = _serde::de::VariantAccess::newtype_variant(__variant)?;
                            ::core::result::Result::Ok(#ident::#variant_ident(__value))
                        }
                    },
                }
            }
        }
        FieldsStyle::Unnamed => {
            let visitor_ident = format_ident!("__Variant{}_TupleVisitor", index);
            helpers.push(tuple_variant_visitor(
                ident,
                variant_ident,
                &variant.fields.fields,
                state_tokens,
                explicit_state,
                generics,
                include_state_param,
                state_bound,
                &visitor_ident,
                where_clause,
            ));
            let len = variant.fields.fields.len();
            quote! {
                (__Variant::#variant_ident, __variant) => {
                    _serde::de::VariantAccess::tuple_variant(
                        __variant,
                        #len,
                        #visitor_ident {
                            state,
                            _marker: ::core::marker::PhantomData,
                        },
                    )
                }
            }
        }
        FieldsStyle::Named => {
            let visitor_ident = format_ident!("__Variant{}_StructVisitor", index);
            let field_array_ident = format_ident!("__VARIANT_FIELDS_{}", index);
            helpers.push(struct_variant_helpers(
                ident,
                variant_ident,
                &variant.fields.fields,
                state_tokens,
                explicit_state,
                generics,
                include_state_param,
                state_bound,
                &visitor_ident,
                &field_array_ident,
                where_clause,
            ));
            quote! {
                (__Variant::#variant_ident, __variant) => {
                    _serde::de::VariantAccess::struct_variant(
                        __variant,
                        #field_array_ident,
                        #visitor_ident {
                            state,
                            _marker: ::core::marker::PhantomData,
                        },
                    )
                }
            }
        }
    }
}

fn tuple_variant_visitor(
    ident: &syn::Ident,
    variant_ident: &syn::Ident,
    fields: &[FieldDecl<'_>],
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    visitor_ident: &syn::Ident,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let len = fields.len();
    let bindings: Vec<_> = (0..len)
        .map(|i| format_ident!("__variant_field_{}", i))
        .collect();
    let read_fields = fields.iter().enumerate().map(|(index, field)| {
        let binding = &bindings[index];
        let ty = field.ty();
        let idx = index;
        if field.attrs.with.is_some() {
            let seed = with_deserialize_seed(field, explicit_state, state_bound);
            quote! {
                let __seed = #seed;
                let #binding = match _serde::de::SeqAccess::next_element_seed(&mut __seq, __seed)? {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                };
            }
        } else {
            match field.mode() {
                ItemMode::Stateful => quote! {
                    let #binding = match _serde::de::SeqAccess::next_element_seed(
                        &mut __seq,
                        _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(state),
                    )? {
                        ::core::option::Option::Some(value) => value,
                        ::core::option::Option::None =>
                            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                    };
                },
                ItemMode::Stateless => quote! {
                    let #binding = match _serde::de::SeqAccess::next_element::<#ty>(&mut __seq)? {
                        ::core::option::Option::Some(value) => value,
                        ::core::option::Option::None =>
                            return ::core::result::Result::Err(_serde::de::Error::invalid_length(#idx, &self)),
                    };
                },
            }
        }
    });
    let construct = quote!(#ident::#variant_ident(#(#bindings),*));

    let (visitor_struct_generics, _) =
        visitor_struct_generics_tokens(generics, include_state_param, state_bound);
    let (visitor_impl_generics, visitor_impl_type_generics) =
        visitor_impl_generics_tokens(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = generics.split_for_impl();
    let phantom_type = phantom_type(ident, generics);

    let visitor_struct = quote! {
        #[allow(non_camel_case_types)]
        struct #visitor_ident #visitor_struct_generics {
            state: &'state #state_tokens,
            _marker: ::core::marker::PhantomData<#phantom_type>,
        }
    };

    let visit_body = quote! {
        fn visit_seq<__A>(
            self,
            mut __seq: __A,
        ) -> ::core::result::Result<Self::Value, __A::Error>
        where
            __A: _serde::de::SeqAccess<'de>,
        {
            let state = self.state;
            #(#read_fields)*
            ::core::result::Result::Ok(#construct)
        }
    };

    let visitor_where_clause = quote_where_clause(where_clause);
    let visitor_impl = quote! {
        impl #visitor_impl_generics _serde::de::Visitor<'de> for #visitor_ident #visitor_impl_type_generics #visitor_where_clause {
            type Value = #ident #ty_generics;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("tuple variant ")?;
                formatter.write_str(stringify!(#ident::#variant_ident))
            }

            #visit_body
        }
    };

    quote! {
        #visitor_struct
        #visitor_impl
    }
}

fn struct_variant_helpers(
    ident: &syn::Ident,
    variant_ident: &syn::Ident,
    fields: &[FieldDecl<'_>],
    state_tokens: &TokenStream,
    explicit_state: Option<&Type>,
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
    visitor_ident: &syn::Ident,
    field_array_ident: &syn::Ident,
    where_clause: &Option<syn::WhereClause>,
) -> TokenStream {
    let included: Vec<_> = fields.iter().filter(|field| !field.attrs.skip).collect();
    let field_idents: Vec<_> = fields.iter().map(|field| field.ident().unwrap()).collect();
    let field_names: Vec<String> = included
        .iter()
        .map(|field| field.attrs.key(field.ident().unwrap()))
        .collect();
    let field_variants: Vec<_> = included
        .iter()
        .map(|field| {
            let name = field.ident().unwrap().to_string();
            format_ident!("__variant_field_{}", name)
        })
        .collect();

    let const_fields = {
        let names = field_names.iter();
        quote! {
            const #field_array_ident: &'static [&'static str] = &[#(#names),*];
        }
    };

    let field_enum_ident = format_ident!("__VariantFieldEnum_{}", variant_ident);
    let field_enum = {
        let variants = field_variants.iter();
        quote! {
            #[allow(non_camel_case_types)]
            enum #field_enum_ident { #(#variants,)* __Ignore }
        }
    };

    let field_visitor_ident = format_ident!("__VariantFieldVisitor_{}", variant_ident);
    let field_visitor = {
        let deserialize_field =
            deserialize_identifier_or_u32(quote!(deserializer), quote!(#field_visitor_ident));
        let match_arms = field_names
            .iter()
            .zip(field_variants.iter())
            .map(|(name, variant)| {
                quote! { #name => ::core::result::Result::Ok(#field_enum_ident::#variant) }
            });
        let index_match_arms = field_variants.iter().enumerate().map(|(index, variant)| {
            let index = index as u64;
            quote! { #index => ::core::result::Result::Ok(#field_enum_ident::#variant) }
        });
        quote! {
            #[allow(non_camel_case_types)]
            struct #field_visitor_ident;
            impl<'de> _serde::de::Visitor<'de> for #field_visitor_ident {
                type Value = #field_enum_ident;

                fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    formatter.write_str("field name")
                }

                fn visit_str<E>(self, value: &str) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    match value {
                        #(#match_arms,)*
                        _ => ::core::result::Result::Ok(#field_enum_ident::__Ignore),
                    }
                }

                fn visit_u64<E>(self, value: u64) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    match value {
                        #(#index_match_arms,)*
                        _ => ::core::result::Result::Ok(#field_enum_ident::__Ignore),
                    }
                }

                fn visit_u32<E>(self, value: u32) -> ::core::result::Result<Self::Value, E>
                where
                    E: _serde::de::Error,
                {
                    self.visit_u64(value as u64)
                }
            }

            impl<'de> _serde::Deserialize<'de> for #field_enum_ident {
                fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
                where
                    D: _serde::Deserializer<'de>,
                {
                    #deserialize_field
                }
            }
        }
    };

    let init_locals = fields.iter().map(|field| {
        let ident = field.ident().unwrap();
        if field.attrs.skip {
            quote!()
        } else {
            quote!(let mut #ident = ::core::option::Option::None;)
        }
    });

    let construct = {
        let pairs = field_idents.iter().map(|ident| quote!(#ident: #ident));
        quote!(#ident::#variant_ident { #(#pairs),* })
    };

    let seq_read_fields = seq_read_fields_body(
        fields,
        &included,
        state_tokens,
        explicit_state,
        state_bound,
        construct.clone(),
    );

    let match_arms = included
        .iter()
        .zip(field_variants.iter())
        .map(|(field, variant)| {
            let ident = field.ident().unwrap();
            let ty = field.ty();
            let field_name = field.attrs.key(ident);
            let assignment = if field.attrs.with.is_some() {
                let seed = with_deserialize_seed(field, explicit_state, state_bound);
                quote! {
                    let __seed = #seed;
                    #ident = ::core::option::Option::Some(
                        _serde::de::MapAccess::next_value_seed(&mut __map, __seed)?,
                    );
                }
            } else {
                match field.mode() {
                    ItemMode::Stateful => quote! {
                        let __seed = _serde_state::__private::wrap_deserialize_seed::<#ty, #state_tokens>(state);
                        #ident = ::core::option::Option::Some(
                            _serde::de::MapAccess::next_value_seed(&mut __map, __seed)?,
                        );
                    },
                    ItemMode::Stateless => quote! {
                        #ident = ::core::option::Option::Some(
                            _serde::de::MapAccess::next_value::<#ty>(&mut __map)?,
                        );
                    },
                }
            };
            quote! {
                #field_enum_ident::#variant => {
                    if #ident.is_some() {
                        return ::core::result::Result::Err(_serde::de::Error::duplicate_field(#field_name));
                    }
                    #assignment
                }
            }
        });

    let build_fields = fields.iter().map(|field| {
        let ident = field.ident().unwrap();
        if field.attrs.skip {
            quote! {
                let #ident = ::core::default::Default::default();
            }
        } else {
            let name = field.attrs.key(ident);
            quote! {
                let #ident = match #ident {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None =>
                        return ::core::result::Result::Err(_serde::de::Error::missing_field(#name)),
                };
            }
        }
    });

    let (visitor_struct_generics, _) =
        visitor_struct_generics_tokens(generics, include_state_param, state_bound);
    let (visitor_impl_generics, visitor_impl_type_generics) =
        visitor_impl_generics_tokens(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = generics.split_for_impl();
    let phantom_type = phantom_type(ident, generics);

    let visitor_struct = quote! {
        #[allow(non_camel_case_types)]
        struct #visitor_ident #visitor_struct_generics {
            state: &'state #state_tokens,
            _marker: ::core::marker::PhantomData<#phantom_type>,
        }
    };

    let visitor_where_clause = quote_where_clause(where_clause);
    let visitor_impl = quote! {
        impl #visitor_impl_generics _serde::de::Visitor<'de> for #visitor_ident #visitor_impl_type_generics #visitor_where_clause {
            type Value = #ident #ty_generics;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str("struct variant ")?;
                formatter.write_str(stringify!(#ident::#variant_ident))
            }

            fn visit_map<__M>(
                self,
                mut __map: __M,
            ) -> ::core::result::Result<Self::Value, __M::Error>
            where
                __M: _serde::de::MapAccess<'de>,
            {
                let state = self.state;
                #(#init_locals)*
                while let ::core::option::Option::Some(key) =
                    _serde::de::MapAccess::next_key::<#field_enum_ident>(&mut __map)?
                {
                    match key {
                        #(#match_arms)*
                        #field_enum_ident::__Ignore => {
                            let _ =
                                _serde::de::MapAccess::next_value::<_serde::de::IgnoredAny>(&mut __map)?;
                        }
                    }
                }
                #(#build_fields)*
                ::core::result::Result::Ok(#construct)
            }

            fn visit_seq<__A>(self, mut __seq: __A) -> ::core::result::Result<Self::Value, __A::Error>
            where
                __A: _serde::de::SeqAccess<'de>,
            {
                #seq_read_fields
            }
        }
    };

    quote! {
        #const_fields
        #field_enum
        #field_visitor
        #visitor_struct
        #visitor_impl
    }
}

fn with_deserialize_seed(
    field: &FieldDecl<'_>,
    explicit_state: Option<&Type>,
    state_bound: Option<&Type>,
) -> TokenStream {
    let ty = field.ty();
    let with = field
        .attrs
        .with
        .as_ref()
        .expect("with_deserialize_seed used without `with`");
    match explicit_state {
        Some(state_ty) => quote! {
            {
                struct __SerdeStateWithSeed<'state> {
                    state: &'state #state_ty,
                }

                impl<'de, 'state> _serde::de::DeserializeSeed<'de>
                    for __SerdeStateWithSeed<'state>
                {
                    type Value = #ty;

                    fn deserialize<__D>(
                        self,
                        __deserializer: __D,
                    ) -> ::core::result::Result<Self::Value, __D::Error>
                    where
                        __D: _serde::Deserializer<'de>,
                    {
                        #with::deserialize_state(self.state, __deserializer)
                    }
                }

                __SerdeStateWithSeed { state }
            }
        },
        None => {
            let bound = state_bound_clause(state_bound);
            quote! {
                {
                    struct __SerdeStateWithSeed<'state, State: ?Sized #bound> {
                        state: &'state State,
                    }

                    impl<'de, 'state, State: ?Sized #bound> _serde::de::DeserializeSeed<'de>
                        for __SerdeStateWithSeed<'state, State>
                    {
                        type Value = #ty;

                        fn deserialize<__D>(
                            self,
                            __deserializer: __D,
                        ) -> ::core::result::Result<Self::Value, __D::Error>
                        where
                            __D: _serde::Deserializer<'de>,
                        {
                            #with::deserialize_state(self.state, __deserializer)
                        }
                    }

                    __SerdeStateWithSeed { state }
                }
            }
        }
    }
}

fn state_bound_clause(bound: Option<&Type>) -> TokenStream {
    match bound {
        Some(ty) => quote!(+ #ty),
        None => TokenStream::new(),
    }
}

fn add_state_param(
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
) -> Generics {
    let mut generics = generics.clone();
    let lifetime: syn::LifetimeParam = parse_quote!('de);
    generics.params.insert(0, GenericParam::Lifetime(lifetime));
    if include_state_param {
        if let Some(bound) = state_bound {
            generics.params.push(parse_quote!(__State: ?Sized + #bound));
        } else {
            generics.params.push(parse_quote!(__State: ?Sized));
        }
    }
    generics
}

struct FieldType<'a> {
    ty: &'a Type,
    mode: ItemMode,
}

impl<'a> FieldType<'a> {
    fn new(ty: &'a Type, mode: ItemMode) -> Self {
        FieldType { ty, mode }
    }
}

fn collect_field_types_from_fields<'a>(fields: &'a FieldsDecl<'a>) -> Vec<FieldType<'a>> {
    fields
        .fields
        .iter()
        .filter_map(|field| {
            if field.attrs.skip {
                return None;
            }
            Some(FieldType::new(field.ty(), field.mode()))
        })
        .collect()
}

fn collect_field_types_from_enum<'a>(data: &'a EnumDecl<'a>) -> Vec<FieldType<'a>> {
    let mut result = Vec::new();
    for variant in &data.variants {
        result.extend(collect_field_types_from_fields(&variant.fields));
    }
    result
}

fn add_deserialize_bounds_from_types(
    where_clause: &mut Option<syn::WhereClause>,
    field_types: &[FieldType<'_>],
    state_ty: &TokenStream,
) {
    if field_types.is_empty() {
        return;
    }

    let clause = where_clause.get_or_insert_with(|| syn::WhereClause {
        where_token: Default::default(),
        predicates: Default::default(),
    });

    for field in field_types {
        let ty = field.ty;
        match field.mode {
            ItemMode::Stateful => clause
                .predicates
                .push(parse_quote!(#ty: _serde_state::DeserializeState<'de, #state_ty>)),
            ItemMode::Stateless => clause
                .predicates
                .push(parse_quote!(#ty: _serde::Deserialize<'de>)),
        }
    }
}

fn add_deserialize_bounds_from_type_params(
    where_clause: &mut Option<syn::WhereClause>,
    generics: &Generics,
    state_ty: &TokenStream,
    mode: ItemMode,
) {
    let type_params: Vec<_> = generics
        .type_params()
        .map(|param| param.ident.clone())
        .collect();
    if type_params.is_empty() {
        return;
    }

    let clause = where_clause.get_or_insert_with(|| syn::WhereClause {
        where_token: Default::default(),
        predicates: Default::default(),
    });

    for ident in type_params {
        match mode {
            ItemMode::Stateful => clause
                .predicates
                .push(parse_quote!(#ident: _serde_state::DeserializeState<'de, #state_ty>)),
            ItemMode::Stateless => clause
                .predicates
                .push(parse_quote!(#ident: _serde::Deserialize<'de>)),
        }
    }
}

fn quote_where_clause(clause: &Option<syn::WhereClause>) -> TokenStream {
    match clause {
        Some(clause) => quote!(#clause),
        None => TokenStream::new(),
    }
}

fn state_type_tokens(decl: &TypeDecl) -> TokenStream {
    if let Some(ty) = decl.attrs.state.as_ref() {
        quote!(#ty)
    } else {
        quote!(__State)
    }
}

fn base_visitor_generics(
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
) -> Generics {
    let mut visitor_generics = Generics::default();
    visitor_generics.params.push(parse_quote!('state));
    if include_state_param {
        if let Some(bound) = state_bound {
            visitor_generics
                .params
                .push(parse_quote!(__State: ?Sized + #bound));
        } else {
            visitor_generics.params.push(parse_quote!(__State: ?Sized));
        }
    }
    visitor_generics
        .params
        .extend(generics.params.iter().cloned());
    visitor_generics
}

fn visitor_struct_generics_tokens(
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
) -> (TokenStream, TokenStream) {
    let visitor_generics = base_visitor_generics(generics, include_state_param, state_bound);
    let (impl_generics, ty_generics, _) = visitor_generics.split_for_impl();
    (quote!(#impl_generics), quote!(#ty_generics))
}

fn visitor_impl_generics_tokens(
    generics: &Generics,
    include_state_param: bool,
    state_bound: Option<&Type>,
) -> (TokenStream, TokenStream) {
    let struct_generics = base_visitor_generics(generics, include_state_param, state_bound);
    let (_, ty_generics, _) = struct_generics.split_for_impl();

    let mut impl_generics = base_visitor_generics(generics, include_state_param, state_bound);
    impl_generics.params.insert(0, parse_quote!('de));
    let (impl_generics_tokens, _, _) = impl_generics.split_for_impl();
    (quote!(#impl_generics_tokens), quote!(#ty_generics))
}

fn phantom_type(ident: &syn::Ident, generics: &Generics) -> TokenStream {
    let (_, ty_generics, _) = generics.split_for_impl();
    quote!(#ident #ty_generics)
}

fn add_default_bounds_for_skipped(
    fields: &FieldsDecl<'_>,
    where_clause: &mut Option<syn::WhereClause>,
) {
    for field in &fields.fields {
        if field.attrs.skip {
            push_default_bound(where_clause, field.ty());
        }
    }
}

fn push_default_bound(where_clause: &mut Option<syn::WhereClause>, ty: &Type) {
    let clause = where_clause.get_or_insert_with(|| syn::WhereClause {
        where_token: Default::default(),
        predicates: Default::default(),
    });
    clause
        .predicates
        .push(parse_quote!(#ty: ::core::default::Default));
}

fn add_default_state_bound(where_clause: &mut Option<syn::WhereClause>, state_ty: &Type) {
    let clause = where_clause.get_or_insert_with(|| syn::WhereClause {
        where_token: Default::default(),
        predicates: Default::default(),
    });
    clause
        .predicates
        .push(parse_quote!(#state_ty: ::core::default::Default));
}

fn default_deserialize_impl(decl: &TypeDecl, ident: &syn::Ident) -> Option<TokenStream> {
    let state_ty = decl.attrs.default_state.as_ref()?;
    let mut impl_generics = decl.generics.clone();
    impl_generics.params.insert(0, parse_quote!('de));
    let (impl_generics_tokens, _, _) = impl_generics.split_for_impl();
    let (_, ty_generics, _) = decl.generics.split_for_impl();
    let mut where_clause = decl.generics.where_clause.clone();
    add_default_state_bound(&mut where_clause, state_ty);
    let where_clause_tokens = quote_where_clause(&where_clause);

    Some(quote! {
        #[automatically_derived]
        impl #impl_generics_tokens _serde::Deserialize<'de> for #ident #ty_generics #where_clause_tokens {
            fn deserialize<__D>(
                __deserializer: __D,
            ) -> ::core::result::Result<Self, __D::Error>
            where
                __D: _serde::Deserializer<'de>,
            {
                let __default_state = <#state_ty as ::core::default::Default>::default();
                _serde_state::DeserializeState::deserialize_state(
                    &__default_state,
                    __deserializer,
                )
            }
        }
    })
}
