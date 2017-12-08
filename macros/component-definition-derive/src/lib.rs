#![recursion_limit="128"]
extern crate proc_macro;
extern crate syn;
#[macro_use]
extern crate quote;

use proc_macro::TokenStream;
use std::iter::Iterator;

#[proc_macro_derive(ComponentDefinition)]
pub fn component_definition(input: TokenStream) -> TokenStream {
    // Construct a string representation of the type definition
    let s = input.to_string();

    // Parse the string representation
    let ast = syn::parse_derive_input(&s).unwrap();

    // Build the impl
    let gen = impl_component_definition(&ast);

    //println!("Derived code:\n{}", gen.clone().into_string());

    // Return the generated impl
    gen.parse().unwrap()
}

fn impl_component_definition(ast: &syn::DeriveInput) -> quote::Tokens {
    let name = &ast.ident;
    if let syn::Body::Struct(ref vdata) = ast.body {
        let fields = vdata.fields();
        let mut ports: Vec<(&syn::Field, PortField)> = Vec::new();
        for field in fields {
            let pf = identify_field(field);
//            println!(
//                "Field {:?} ({:?}) with type {:?}",
//                field.ident,
//                pf,
//                field.ty
//            );
            match pf {
                PortField::Provided(_) => ports.push((field, pf)),
                PortField::Required(_) => ports.push((field, pf)),
                PortField::None => (),
            }
        }
        let port_setup = ports
            .iter()
            .map(|&(f, _)| {
                //let (ref f, _) = t;
                let ref id = f.ident;
                quote! { self.#id.set_parent(self_component.clone()); }
            })
            .collect::<Vec<_>>();
        let port_handles = ports
            .iter()
            .map(|&(f, ref t)| {
                let ref id = f.ident;
                //let ref ty = f.ty;
                let handle = match t {
                    &PortField::Provided(ref ty) => quote! { Provide::<#ty>::handle(self, event); },
                    &PortField::Required(ref ty) => quote! { Require::<#ty>::handle(self, event); },
                    &PortField::None => unreachable!(),
                };
                quote! {
                if let Some(event) = self.#id.dequeue() {
                    #handle
                    count += 1;
                    done_work = true;
                }
                if count >= max_events {
                    break;
                }
            }
            })
            .collect::<Vec<_>>();
        let num_ports = ports.len();
        quote! {
            impl ComponentDefinition for #name {
                fn setup_ports(&mut self, self_component: Arc<Component<Self>>) -> () {
                    //println!("Setting up ports");
                    #(#port_setup)*
                }
                fn execute(&mut self, core: &ComponentCore, max_events: usize) -> () {
                    //println!("Executing");
                    if #num_ports > max_events {
                        panic!("Throughput must be larger than the number of ports, to prevent starvation. ({} < {})", #num_ports, max_events);
                    }
                    let mut count:usize = 0;
                    let mut done_work = true;
                    while (count < max_events) && done_work {
                        done_work = false;
                        #(#port_handles)*
                    }
                    //println!("Executed {}", count);
                    match core.decrement_work(count) {
                        SchedulingDecision::Schedule => {
                            //println!("Rescheduling!");
                            let system = core.system();
                            let cc = core.component();
                            system.schedule(cc);
                        }
                        _ => (), // ignore
                    }
                }
            }
        }
    } else {
        //Nope. This is an Enum. We cannot handle these!
        panic!("#[derive(ComponentDefinition)] is only defined for structs, not for enums!");
    }
}

#[derive(Debug)]
enum PortField {
    Required(syn::Ty),
    Provided(syn::Ty),
    None,
}

const REQP: &'static str = "RequiredPort";
const PROVP: &'static str = "ProvidedPort";
const KOMPICS: &'static str = "kompics";

fn identify_field(f: &syn::Field) -> PortField {
    if let syn::Ty::Path(_, ref path) = f.ty {
        if !path.global {
            let port_seg_opt = if path.segments.len() == 1 {
                Some(&path.segments[0])
            } else if path.segments.len() == 2 {
                if path.segments[0].ident == KOMPICS {
                    Some(&path.segments[1])
                } else {
                    println!("Module is not 'kompics': {:?}", path);
                    None
                }
            } else {
                println!("Path too long for port: {:?}", path);
                None
            };
            if let Some(seg) = port_seg_opt {
                if seg.ident == REQP {
                    PortField::Required(extract_port_type(seg))
                } else if seg.ident == PROVP {
                    PortField::Provided(extract_port_type(seg))
                } else {
                    println!("Not a port: {:?}", path);
                    PortField::None
                }
            } else {
                PortField::None
            }
        } else {
            PortField::None
        }
    } else {
        PortField::None
    }
}

fn extract_port_type(seg: &syn::PathSegment) -> syn::Ty {
    match seg.parameters {
        syn::PathParameters::AngleBracketed(ref abppd) => abppd.types[0].clone(),
        _ => panic!("Wrong path parameter type! {:?}", seg),
    }
}
