#![recursion_limit = "128"]
extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

use std::{collections::HashMap, iter::Iterator};

/// A macro to derive fair [ComponentDefinition](ComponentDefinition) implementations
///
/// Implementations will set up ports and the component context correctly, and
/// during execution check ports in a fair round-robin manner.
///
/// Using this macro will also derive implementations of [ProvideRef](ProvideRef)
/// or [RequireRef](RequireRef) for each declared port.
#[proc_macro_derive(ComponentDefinition)]
pub fn component_definition(input: TokenStream) -> TokenStream {
    // Parse the input stream
    let ast = parse_macro_input!(input as DeriveInput);

    // Build the impl
    let gen = impl_component_definition(&ast);

    //println!("Derived code:\n{}", gen);

    // Return the generated impl
    gen.into()
}

type PortEntry<'a> = (&'a syn::Field, PortField);

#[allow(clippy::map_entry)]
fn impl_component_definition(ast: &syn::DeriveInput) -> TokenStream2 {
    let name = &ast.ident;
    let name_str = format!("{}", name);
    if let syn::Data::Struct(ref vdata) = ast.data {
        let generics = &ast.generics;
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let fields = &vdata.fields;
        let mut ports: Vec<PortEntry> = Vec::new();
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
                let id = &f.ident;
                let setup = quote! { self.#id.initialise(self_component.clone()); };
                let access = quote! { self.#id };
                (setup, access)
            }
            None => panic!("No ComponentContext found for {:?}!", name),
        };
        let port_setup = ports
            .iter()
            .map(|&(f, _)| {
                let id = &f.ident;
                quote! { self.#id.set_parent(self_component.clone()); }
            })
            .collect::<Vec<_>>();
        let port_handles_skip = ports
            .iter()
            .enumerate()
            .map(|(i, &(f, ref t))| {
                let id = &f.ident;
                //let ref ty = f.ty;
                let handle = t.as_handle();
                quote! {
                    if skip <= #i {
                        if count >= max_events {
                            return ExecuteResult::new(false, count, #i);
                        }
                        #[allow(unreachable_code)]
                        { // can be Never type, which is ok, so just suppress this warning
                            if let Some(event) = self.#id.dequeue() {
                                let res = #handle
                                count += 1;
                                done_work = true;
                                if let Handled::BlockOn(blocking_future) = res {
                                    self.ctx_mut().set_blocking(blocking_future);
                                    return ExecuteResult::new(true, count, #i);
                                }
                            }
                        }
                    }
                }
            })
            .collect::<Vec<_>>();
        let port_handles = ports
            .iter()
            .enumerate()
            .map(|(i, &(f, ref t))| {
                let id = &f.ident;
                //let ref ty = f.ty;
                let handle = t.as_handle();
                quote! {
                    if count >= max_events {
                        return ExecuteResult::new(false, count, #i);
                    }
                    #[allow(unreachable_code)]
                    { // can be Never type, which is ok, so just suppress this warning
                        if let Some(event) = self.#id.dequeue() {
                            let res = #handle
                            count += 1;
                            done_work = true;
                            if let Handled::BlockOn(blocking_future) = res {
                                self.ctx_mut().set_blocking(blocking_future);
                                return ExecuteResult::new(true, count, #i);
                            }
                        }
                    }
                }
            })
            .collect::<Vec<_>>();
        let exec = if port_handles.is_empty() {
            quote! {
                fn execute(&mut self, _max_events: usize, _skip: usize) -> ExecuteResult {
                    ExecuteResult::new(false, 0, 0)
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
                    ExecuteResult::new(false, count, 0)
                }
            }
        };

        let mut provided_ports_unique: HashMap<syn::Type, PortEntry> = HashMap::new();
        let mut provided_ports_non_unique: HashMap<syn::Type, Vec<PortEntry>> = HashMap::new();
        let mut required_ports_unique: HashMap<syn::Type, PortEntry> = HashMap::new();
        let mut required_ports_non_unique: HashMap<syn::Type, Vec<PortEntry>> = HashMap::new();
        for port in ports {
            let port_field = port.1.clone();
            match port_field {
                PortField::Required(ty) => {
                    if let Some(port_list) = required_ports_non_unique.get_mut(&ty) {
                        port_list.push(port);
                    } else if required_ports_unique.contains_key(&ty) {
                        let other_entry = required_ports_unique.remove(&ty).unwrap();
                        required_ports_non_unique.insert(ty, vec![other_entry, port]);
                    } else {
                        required_ports_unique.insert(ty, port);
                    }
                }
                PortField::Provided(ty) => {
                    if let Some(port_list) = provided_ports_non_unique.get_mut(&ty) {
                        port_list.push(port);
                    } else if provided_ports_unique.contains_key(&ty) {
                        let other_entry = provided_ports_unique.remove(&ty).unwrap();
                        provided_ports_non_unique.insert(ty, vec![other_entry, port]);
                    } else {
                        provided_ports_unique.insert(ty, port);
                    }
                }
            }
        }

        let generate_provided_ref_impl = |p: &PortEntry| {
            let (field, port_field) = p;
            let id = &field.ident;
            let ty = port_field.port_type();
            quote! {
                impl #impl_generics ProvideRef< #ty > for #name #ty_generics #where_clause {
                    fn provided_ref(&mut self) -> ProvidedRef< #ty > {
                        self.#id.share()
                    }
                    fn connect_to_required(&mut self, req: RequiredRef< #ty >) -> () {
                        self.#id.connect(req);
                    }
                    fn disconnect(&mut self, req: RequiredRef< #ty >) -> () {
                        self.#id.disconnect_port(req);
                    }
                }
            }
        };
        let generate_required_ref_impl = |p: &PortEntry| {
            let (field, port_field) = p;
            let id = &field.ident;
            let ty = port_field.port_type();
            quote! {
                impl #impl_generics RequireRef< #ty > for #name #ty_generics #where_clause {
                    fn required_ref(&mut self) -> RequiredRef< #ty > {
                        self.#id.share()
                    }
                    fn connect_to_provided(&mut self, prov: ProvidedRef< #ty >) -> () {
                        self.#id.connect(prov);
                    }
                    fn disconnect(&mut self, prov: ProvidedRef< #ty >) -> () {
                        self.#id.disconnect_port(prov);
                    }
                }
            }
        };
        let generate_ambiguous_provided_ref_impl = |ty: &syn::Type, port_entries: &[PortEntry]| {
            let ids: Vec<String> = port_entries
                .iter()
                .map(|(field, _port_field)| {
                    let id = field.ident.as_ref().unwrap();
                    format!("{}", quote! {#id})
                })
                .collect();
            let error_msg = format!("Ambiguous port type: There are multiple fields with type {} ({:?}). You cannot derive ComponentDefinition in these cases, as you must resolve the ambiguity manually.", quote!{#ty}, ids);
            quote! {
                impl #impl_generics ProvideRef< #ty > for #name #ty_generics #where_clause {
                    fn provided_ref(&mut self) -> ProvidedRef< #ty > {
                        compile_error!(#error_msg);
                    }
                    fn connect_to_required(&mut self, req: RequiredRef< #ty >) -> () {
                        compile_error!(#error_msg);
                    }
                    fn disconnect(&mut self, req: RequiredRef< #ty >) -> () {
                        compile_error!(#error_msg);
                    }
                }
            }
        };
        let generate_ambiguous_required_ref_impl = |ty: &syn::Type, port_entries: &[PortEntry]| {
            let ids: Vec<String> = port_entries
                .iter()
                .map(|(field, _port_field)| {
                    let id = field.ident.as_ref().unwrap();
                    format!("{}", quote! {#id})
                })
                .collect();
            let error_msg = format!("Ambiguous port type: There are multiple fields with type {} ({:?}). You cannot derive ComponentDefinition in these cases, as you must resolve the ambiguity manually.", quote!{#ty}, ids);
            quote! {
                impl #impl_generics RequireRef< #ty > for #name #ty_generics #where_clause {
                    fn provided_ref(&mut self) -> ProvidedRef< #ty > {
                        compile_error!(#error_msg);
                    }
                    fn connect_to_required(&mut self, req: RequiredRef< #ty >) -> () {
                        compile_error!(#error_msg);
                    }
                    fn disconnect(&mut self, req: RequiredRef< #ty >) -> () {
                        compile_error!(#error_msg);
                    }
                }
            }
        };

        let port_ref_impls = provided_ports_unique
            .values()
            .map(generate_provided_ref_impl)
            .chain(
                required_ports_unique
                    .values()
                    .map(generate_required_ref_impl),
            )
            .chain(
                provided_ports_non_unique
                    .iter()
                    .map(|pair| generate_ambiguous_provided_ref_impl(pair.0, pair.1)),
            )
            .chain(
                required_ports_non_unique
                    .iter()
                    .map(|pair| generate_ambiguous_required_ref_impl(pair.0, pair.1)),
            )
            .collect::<Vec<_>>();

        fn make_match(f: &syn::Field, t: &syn::Type) -> TokenStream2 {
            let f = &f.ident;
            quote! {
                id if id == ::std::any::TypeId::of::<#t>() =>
                    Some(&mut self.#f as &mut dyn ::std::any::Any),
            }
        }

        let provided_matches: Vec<_> = provided_ports_unique
            .iter()
            .map(|(t, p)| make_match(p.0, t))
            .collect();

        let required_matches: Vec<_> = required_ports_unique
            .iter()
            .map(|(t, p)| make_match(p.0, t))
            .collect();

        quote! {
            impl #impl_generics ComponentDefinition for #name #ty_generics #where_clause {
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
            impl #impl_generics DynamicPortAccess for #name #ty_generics #where_clause {
                fn get_provided_port_as_any(&mut self, port_id: ::std::any::TypeId) -> Option<&mut dyn ::std::any::Any> {
                    match port_id {
                        #(#provided_matches)*
                        _ => None,
                    }
                }

                fn get_required_port_as_any(&mut self, port_id: ::std::any::TypeId) -> Option<&mut dyn ::std::any::Any> {
                    match port_id {
                        #(#required_matches)*
                        _ => None,
                    }
                }
            }
            #(#port_ref_impls)*
        }
    } else {
        //Nope. This is an Enum. We cannot handle these!
        panic!("#[derive(ComponentDefinition)] is only defined for structs, not for enums!");
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum ComponentField {
    Ctx,
    Port(PortField),
    Other,
}

#[derive(Debug, Clone)]
enum PortField {
    Required(syn::Type),
    Provided(syn::Type),
}

impl PortField {
    fn as_handle(&self) -> TokenStream2 {
        match *self {
            PortField::Provided(ref ty) => quote! { Provide::<#ty>::handle(self, event); },
            PortField::Required(ref ty) => quote! { Require::<#ty>::handle(self, event); },
        }
    }

    fn port_type(&self) -> &syn::Type {
        match self {
            PortField::Provided(ref ty) => ty,
            PortField::Required(ref ty) => ty,
        }
    }
}

const REQP: &str = "RequiredPort";
const PROVP: &str = "ProvidedPort";
const CTX: &str = "ComponentContext";
const KOMPICS: &str = "kompact";

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
            match abppd.args.first().expect("Invalid type argument!") {
                syn::GenericArgument::Type(ty) => ty.clone(),
                _ => panic!("Wrong generic argument type in {:?}", seg),
            }
        }
        _ => panic!("Wrong path parameter type! {:?}", seg),
    }
}
