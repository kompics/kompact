#![allow(clippy::unused_unit)]
use kompact::{prelude::*, serde_serialisers::*};
use lru::LruCache;
use rand::{distributions::Alphanumeric, rngs::SmallRng, thread_rng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

// ANCHOR: messages
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Query {
    id: Uuid,
    pattern: String,
}
impl SerialisationId for Query {
    const SER_ID: SerId = 4242;
}
#[derive(Serialize, Deserialize, Debug, Clone)]
struct QueryResponse {
    id: Uuid,
    pattern: String,
    matches: Vec<String>,
}
impl SerialisationId for QueryResponse {
    const SER_ID: SerId = 4243;
}
// ANCHOR_END: messages

#[derive(ComponentDefinition)]
struct QueryServer {
    ctx: ComponentContext<Self>,
    database: Arc<[String]>,
    handled_requests: usize,
}
impl QueryServer {
    fn new(database: Arc<[String]>) -> Self {
        QueryServer {
            ctx: ComponentContext::uninitialised(),
            database,
            handled_requests: 0,
        }
    }

    fn find_matches(&self, pattern: &str) -> Vec<String> {
        self.database
            .iter()
            .filter(|e| e.contains(pattern))
            .cloned()
            .collect()
    }
}

impl ComponentLifecycle for QueryServer {
    fn on_kill(&mut self) -> Handled {
        info!(
            self.log(),
            "Shutting down a Server that handled {} requests", self.handled_requests
        );
        Handled::Ok
    }
}
impl Actor for QueryServer {
    type Message = Never;

    fn receive_local(&mut self, _msg: Self::Message) -> Handled {
        unreachable!("Can't instantiate Never type");
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        let sender = msg.sender;

        match_deser!(msg.data; {
            query: Query [Serde] => {
                let matches = self.find_matches(&query.pattern);
                let response = QueryResponse { id: query.id, pattern: query.pattern, matches };
                sender.tell((response, Serde), self);
                self.handled_requests += 1;
            },
        });
        Handled::Ok
    }
}

#[derive(ComponentDefinition)]
struct Client {
    ctx: ComponentContext<Self>,
    server_path: ActorPath,
    broadcast_path: ActorPath,
    request_count: usize,
    cache_hits: usize,
    cache: LruCache<String, Vec<String>>,
    current_query: Option<Query>,
    rng: SmallRng,
}
impl Client {
    fn new(server_path: ActorPath, broadcast_path: ActorPath) -> Self {
        Client {
            ctx: ComponentContext::uninitialised(),
            server_path,
            broadcast_path,
            request_count: 0,
            cache_hits: 0,
            cache: LruCache::new(20),
            current_query: None,
            rng: SmallRng::from_entropy(),
        }
    }

    fn send_request(&mut self) -> () {
        while self.current_query.is_none() {
            let pattern = generate_string(&mut self.rng, PATTERN_LENGTH);
            self.request_count += 1;
            let res = self.cache.get(&pattern).map(|result| result.len());
            if let Some(result) = res {
                self.cache_hits += 1;
                debug!(
                    self.log(),
                    "Answered query #{} ({}) with {} matches from cache.",
                    self.request_count,
                    pattern,
                    result
                );
            } else {
                let id = Uuid::new_v4();
                trace!(
                    self.log(),
                    "Sending query #{} ({}) with id={}",
                    self.request_count,
                    pattern,
                    id
                );
                let query = Query { id, pattern };
                self.current_query = Some(query.clone());
                // ANCHOR: client_send
                self.server_path
                    .tell((query, Serde), &self.broadcast_path.using_dispatcher(self));
                // ANCHOR_END: client_send
            }
        }
    }
}

impl ComponentLifecycle for Client {
    fn on_start(&mut self) -> Handled {
        self.send_request();
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        let hit_ratio = (self.cache_hits as f64) / (self.request_count as f64);
        info!(
            self.log(),
            "Shutting down a Client that ran {} requests with {} cache hits ({}%)",
            self.request_count,
            self.cache_hits,
            hit_ratio
        );
        Handled::Ok
    }
}

impl Actor for Client {
    type Message = Never;

    fn receive_local(&mut self, _msg: Self::Message) -> Handled {
        unreachable!("Can't instantiate Never type");
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        match_deser!(msg; {
            response: QueryResponse [Serde] => {
                trace!(self.log(), "Got response for query id={}: {:?}", response.id, response.matches);
                if let Some(current_query) = self.current_query.take() {
                    if current_query.id == response.id {
                        debug!(self.log(), "Got response with {} matches for query: {}", response.matches.len(), current_query.pattern);
                        self.send_request();
                    } else {
                        // wrong id, put it back
                        self.current_query = Some(current_query);
                    }
                }
                // in any case, put it in the cache
                self.cache.put(response.pattern, response.matches);
            },
        });
        Handled::Ok
    }
}

// ANCHOR: constants
const ENTRY_LENGTH: usize = 20;
const PATTERN_LENGTH: usize = 2;

const BALANCER_PATH: &str = "server";
const CLIENT_PATH: &str = "client";

const NUM_SERVERS: usize = 3;
const NUM_CLIENTS: usize = 12;
const DATABASE_SIZE: usize = 10000;

const TIMEOUT: Duration = Duration::from_millis(100);
// ANCHOR_END: constants

fn generate_string<R: Rng>(rng: &mut R, length: usize) -> String {
    std::iter::repeat(())
        .map(|_| rng.sample(Alphanumeric))
        .take(length)
        .collect()
}

fn generate_database(size: usize) -> Arc<[String]> {
    let mut data: Vec<String> = Vec::with_capacity(size);
    let mut rng = thread_rng();
    for _i in 0..size {
        let entry = generate_string(&mut rng, ENTRY_LENGTH);
        data.push(entry);
    }
    data.into()
}

pub fn main() {
    let mut cfg = KompactConfig::new();
    cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let system = cfg.build().expect("KompactSystem");

    // ANCHOR: policies_setup
    // use implicit policy
    let broadcast_path: ActorPath = system
        .system_path()
        .into_named_with_string("client/*")
        .expect("path")
        .into();

    // set explicit policy
    let balancer_path = system
        .set_routing_policy(
            kompact::routing::groups::RoundRobinRouting::default(),
            BALANCER_PATH,
            false,
        )
        .wait_expect(TIMEOUT, "balancing policy");
    // ANCHOR_END: policies_setup

    let database = generate_database(DATABASE_SIZE);

    // ANCHOR: server_setup
    let servers: Vec<Arc<Component<QueryServer>>> = (0..NUM_SERVERS)
        .map(|_| {
            let db = database.clone();
            system.create(move || QueryServer::new(db))
        })
        .collect();

    let registration_futures: Vec<KFuture<RegistrationResult>> = servers
        .iter()
        .enumerate()
        .map(|(index, server)| {
            system.register_by_alias(server, format!("{}/server-{}", BALANCER_PATH, index))
        })
        .collect();
    // We don't actually need the paths,
    // just need to be sure they finished registering
    registration_futures.expect_ok(TIMEOUT, "server path");
    // ANCHOR_END: server_setup

    // ANCHOR: client_setup
    let clients: Vec<Arc<Component<Client>>> = (0..NUM_CLIENTS)
        .map(|_| {
            let server_path = balancer_path.clone();
            let client_path = broadcast_path.clone();
            system.create(move || Client::new(server_path, client_path))
        })
        .collect();
    let registration_futures: Vec<KFuture<RegistrationResult>> = clients
        .iter()
        .enumerate()
        .map(|(index, client)| {
            system.register_by_alias(client, format!("{}/client-{}", CLIENT_PATH, index))
        })
        .collect();
    // We don't actually need the paths,
    // just need to be sure they finished registering
    registration_futures.expect_ok(TIMEOUT, "client path");
    // ANCHOR_END: client_setup

    // ANCHOR: running
    // Start everything
    servers
        .iter()
        .map(|s| system.start_notify(&s))
        .expect_completion(TIMEOUT, "server start");
    clients
        .iter()
        .map(|c| system.start_notify(&c))
        .expect_completion(TIMEOUT, "client start");

    // Let them work for a while
    std::thread::sleep(Duration::from_secs(5));

    // Shut down clients nicely.
    clients
        .into_iter()
        .map(|c| system.kill_notify(c))
        .collect::<Vec<_>>()
        .expect_completion(TIMEOUT, "client kill");

    // Shut down servers nicely.
    servers
        .into_iter()
        .map(|s| system.kill_notify(s))
        .collect::<Vec<_>>()
        .expect_completion(TIMEOUT, "server kill");

    system.shutdown().expect("shutdown");
    // ANCHOR_END: running
    // Wait a bit longer, so all output is logged (asynchronously) before shutting down
    std::thread::sleep(Duration::from_millis(10));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_balancer() {
        main();
    }
}
