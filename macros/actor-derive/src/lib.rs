#![recursion_limit = "128"]
extern crate proc_macro;
use syn;
#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

#[proc_macro_derive(Actor)]
pub fn actor(input: TokenStream) -> TokenStream {
    // Construct a string representation of the type definition
    let s = input.to_string();

    // Parse the string representation
    let ast = syn::parse_derive_input(&s).unwrap();

    // Build the impl
    let gen = impl_actor(&ast);

    //println!("Derived code:\n{}", gen.clone().into_string());

    // Return the generated impl
    gen.parse().unwrap()
}

fn impl_actor(ast: &syn::DeriveInput) -> quote::Tokens {
    let name = &ast.ident;
    if let syn::Body::Struct(_) = ast.body {
        quote! {
            impl ActorRaw for #name {
                fn receive(&mut self, env: messaging::ReceiveEnvelope) -> () {
                    println!("Got msg, but component isn't handling any: {:?}", env);
                }
            }
        }
    } else {
        //Nope. This is an Enum. We cannot handle these!
        panic!("#[derive(Actor)] is only defined for structs, not for enums!");
    }
}
