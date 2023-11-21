use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Field, Path};

#[proc_macro_derive(RuntimeExposed)]
pub fn derive_runtime_exposed(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let struct_name = &ast.ident;
    let (impl_generics, type_generics, where_clause) = &ast.generics.split_for_impl();

    let field_defs = match ast.data {
        syn::Data::Struct(s) => s
            .fields
            .into_iter()
            .map(|value| {
                let field_name = value.ident.unwrap();
                let field_type = value.ty;
                let type_name = syn::Ident::new(
                    &format!("{}{}Accessor", struct_name, field_name),
                    struct_name.span(),
                );
                let string_field = format!("{}", field_name);
                let fragment = quote! {
                    struct #type_name;
                    impl FieldAccessor<#struct_name> for #type_name {
                        type FieldResult = #field_type;
                        fn name() -> &'static str {
                            #string_field
                        }
                        fn get(p: &BoundObj, _scope: &mut HandleScope) -> Self::FieldResult {
                            p.#field_name
                        }
                    }
                };
                (type_name, fragment)
            })
            .collect::<Vec<_>>(),
        syn::Data::Enum(e) => panic!(),
        syn::Data::Union(u) => panic!(),
    };

    let field_names = field_defs
        .iter()
        .map(|i| {
            let name = i.0.clone();
            quote! {
                template.set_accessor::<#name>();
            }
        })
        .collect::<Vec<_>>();

    let field_accessors = field_defs.iter().map(|i| i.1.clone()).collect::<Vec<_>>();

    TokenStream::from(quote! {
        impl #impl_generics crate::logic::expose::RuntimeExposed for #struct_name #type_generics #where_clause {
            fn setup_template(template: &mut ExposedTypeBinder<Self>) {
                #(#field_names)*
            }
        }
        #(#field_accessors)*
    })
}
