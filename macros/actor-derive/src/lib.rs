#![recursion_limit = "128"]
extern crate proc_macro;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

#[proc_macro_derive(Actor)]
pub fn actor(input: TokenStream) -> TokenStream {
    // Parse the input stream
    let ast = parse_macro_input!(input as DeriveInput);

    // Build the impl
    let gen = impl_actor(&ast);

    //println!("Derived code:\n{}", gen.clone().into_string());

    // Return the generated impl
    gen.into()
}

fn impl_actor(ast: &syn::DeriveInput) -> TokenStream2 {
    let name = &ast.ident;
    if let syn::Data::Struct(_) = ast.data {
        let generics = &ast.generics;
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        quote! {
            impl #impl_generics ActorRaw for #name #ty_generics #where_clause {

                type Message = Never;

                fn receive(&mut self, env: MsgEnvelope<Self::Message>) -> () {
                    println!("Got msg, but component isn't handling any: {:?}", env);
                }
            }
        }
    } else {
        //Nope. This is an Enum. We cannot handle these!
        panic!("#[derive(Actor)] is only defined for structs, not for enums!");
    }
}
