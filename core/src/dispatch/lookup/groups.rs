//! Support for routing groups and policies

use crate::{
    actors::{ActorPath, DynActorRef},
    messaging::{NetMessage, PathResolvable},
};
use uuid::Uuid;

pub struct RoutingGroup {
    members: Vec<DynActorRef>,
    policy: Box<dyn RoutingPolicy<DynActorRef, NetMessage>>,
}

pub trait RoutingPolicy<Ref, M> {
    fn route(&mut self, members: &[Ref], msg: M);
}
