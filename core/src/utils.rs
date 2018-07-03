use std::sync::Arc;

use std::ops::{DerefMut};

use super::*;

//pub fn connect<P: Port>(c: &ProvidedRef<P>, p: &mut Required<P>) -> () {}

pub fn on_dual_definition<C1, C2, F>(c1: Arc<Component<C1>>, c2: Arc<Component<C2>>, f: F) -> ()
where
    C1: ComponentDefinition + Sized + 'static,
    C2: ComponentDefinition + Sized + 'static,
    F: FnOnce(&mut C1, &mut C2) -> (),
{
    //c1.on_definition(|cd1| c2.on_definition(|cd2| f(cd1, cd2)))
    let mut cd1 = c1.definition().lock().unwrap();
    let mut cd2 = c2.definition().lock().unwrap();
    f(cd1.deref_mut(), cd2.deref_mut());
}

pub fn biconnect<P, C1, C2>(prov: &mut ProvidedPort<P, C1>, req: &mut RequiredPort<P, C2>) -> ()
where
    P: Port,
    C1: ComponentDefinition + Sized + 'static + Provide<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P>,
{
    let prov_share = prov.share();
    let req_share = req.share();
    prov.connect(req_share);
    req.connect(prov_share);
}
