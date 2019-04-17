#![recursion_limit = "128"]
extern crate proc_macro;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

use std::iter::Iterator;

#[proc_macro_derive(ComponentDefinition)]
pub fn component_definition(input: TokenStream) -> TokenStream {
    // Parse the input stream
    let ast = parse_macro_input!(input as DeriveInput);

    // Build the impl
    let gen = impl_component_definition(&ast);

    //println!("Derived code:\n{}", gen.clone().into_string());

    // Return the generated impl
    gen.into()
}

fn impl_component_definition(ast: &syn::DeriveInput) -> TokenStream2 {
    let name = &ast.ident;
    let name_str = format!("{}", name);
    if let syn::Data::Struct(ref vdata) = ast.data {
        let fields = &vdata.fields;
        let mut ports: Vec<(&syn::Field, PortField)> = Vec::new();
        let mut ctx_field: Option<&syn::Field> = None;
        for field in fields.iter() {
            let cf = identify_field(field);
            match cf {
                ComponentField::Ctx => {
                    ctx_field = Some(field);
                }
                ComponentField::Port(pf) => ports.push((field, pf)),
                ComponentField::Other => (),
            }
        }
        let (ctx_setup, ctx_access) = match ctx_field {
            Some(f) => {
                let ref id = f.ident;
                let setup = quote! { self.#id.initialise(self_component.clone()); };
                let access = quote! { self.#id };
                (setup, access)
            }
            None => panic!("No ComponentContext found for {:?}!", name),
        };
        let port_setup = ports
            .iter()
            .map(|&(f, _)| {
                let ref id = f.ident;
                quote! { self.#id.set_parent(self_component.clone()); }
            })
            .collect::<Vec<_>>();
        let port_handles_skip = ports
            .iter()
            .enumerate()
            .map(|(i, &(f, ref t))| {
                let ref id = f.ident;
                //let ref ty = f.ty;
                let handle = t.as_handle();
                quote! {
                    if skip <= #i {
                        if count >= max_events {
                            return ExecuteResult::new(count, #i);
                        }
                        if let Some(event) = self.#id.dequeue() {
                            #handle
                            count += 1;
                            done_work = true;
                        }
                    }
                }
            })
            .collect::<Vec<_>>();
        let port_handles = ports
            .iter()
            .enumerate()
            .map(|(i, &(f, ref t))| {
                let ref id = f.ident;
                //let ref ty = f.ty;
                let handle = t.as_handle();
                quote! {
                    if count >= max_events {
                        return ExecuteResult::new(count, #i);
                    }
                    if let Some(event) = self.#id.dequeue() {
                        #handle
                        count += 1;
                        done_work = true;
                    }
                }
            })
            .collect::<Vec<_>>();
        let exec = if port_handles.is_empty() {
            quote! {
                fn execute(&mut self, _max_events: usize, _skip: usize) -> ExecuteResult {
                    ExecuteResult::new(0, 0)
                }
            }
        } else {
            quote! {
                fn execute(&mut self, max_events: usize, skip: usize) -> ExecuteResult {
                    let mut count: usize = 0;
                    let mut done_work = true; // might skip queues that have work
                    #(#port_handles_skip)*
                    while done_work {
                        done_work = false;
                        #(#port_handles)*
                    }
                    ExecuteResult::new(count, 0)
                }
            }
        };
        quote! {
            impl ComponentDefinition for #name {
                fn setup(&mut self, self_component: ::std::sync::Arc<Component<Self>>) -> () {
                    #ctx_setup
                    //println!("Setting up ports");
                    #(#port_setup)*
                }
                #exec
                fn ctx_mut(&mut self) -> &mut ComponentContext<Self> {
                    &mut #ctx_access
                }
                fn ctx(&self) -> &ComponentContext<Self> {
                    &#ctx_access
                }
                fn type_name() -> &'static str {
                    #name_str
                }
            }
        }
    } else {
        //Nope. This is an Enum. We cannot handle these!
        panic!("#[derive(ComponentDefinition)] is only defined for structs, not for enums!");
    }
}

#[derive(Debug)]
enum ComponentField {
    Ctx,
    Port(PortField),
    Other,
}

#[derive(Debug)]
enum PortField {
    Required(syn::Type),
    Provided(syn::Type),
}

impl PortField {
    fn as_handle(&self) -> TokenStream2 {
        match self {
            &PortField::Provided(ref ty) => quote! { Provide::<#ty>::handle(self, event); },
            &PortField::Required(ref ty) => quote! { Require::<#ty>::handle(self, event); },
        }
    }
}

const REQP: &'static str = "RequiredPort";
const PROVP: &'static str = "ProvidedPort";
const CTX: &'static str = "ComponentContext";
const KOMPICS: &'static str = "kompact";

fn identify_field(f: &syn::Field) -> ComponentField {
    if let syn::Type::Path(ref patht) = f.ty {
        let path = &patht.path;
        let port_seg_opt = if path.segments.len() == 1 {
            Some(&path.segments[0])
        } else if path.segments.len() == 2 {
            if path.segments[0].ident == KOMPICS {
                Some(&path.segments[1])
            } else {
                //println!("Module is not 'kompact': {:?}", path);
                None
            }
        } else {
            //println!("Path too long for port: {:?}", path);
            None
        };
        if let Some(seg) = port_seg_opt {
            if seg.ident == REQP {
                ComponentField::Port(PortField::Required(extract_port_type(seg)))
            } else if seg.ident == PROVP {
                ComponentField::Port(PortField::Provided(extract_port_type(seg)))
            } else if seg.ident == CTX {
                ComponentField::Ctx
            } else {
                //println!("Not a port: {:?}", path);
                ComponentField::Other
            }
        } else {
            ComponentField::Other
        }
    } else {
        ComponentField::Other
    }
}

fn extract_port_type(seg: &syn::PathSegment) -> syn::Type {
    match seg.arguments {
        syn::PathArguments::AngleBracketed(ref abppd) => {
            match abppd.args.first().expect("Invalid type argument!").value() {
                syn::GenericArgument::Type(ty) => ty.clone(),
                _ => panic!("Wrong generic argument type in {:?}", seg),
            }
        }
        _ => panic!("Wrong path parameter type! {:?}", seg),
    }
}
