#![allow(clippy::unused_unit)]
use async_std_resolver::{config, resolver, AsyncStdResolver};
use dialoguer::Input;
use kompact::prelude::*;
use trust_dns_proto::{rr::record_type::RecordType, xfer::dns_request::DnsRequestOptions};

// ANCHOR: messages
#[derive(Debug)]
struct DnsRequest(String);
#[derive(Debug)]
struct DnsResponse(String);
// ANCHOR_END: messages

// ANCHOR: state
#[derive(ComponentDefinition)]
struct DnsComponent {
    ctx: ComponentContext<Self>,
    resolver: Option<AsyncStdResolver>,
}
impl DnsComponent {
    pub fn new() -> Self {
        DnsComponent {
            ctx: ComponentContext::uninitialised(),
            resolver: None,
        }
    }
}
// ANCHOR_END: state

// ANCHOR: lifecycle
impl ComponentLifecycle for DnsComponent {
    fn on_start(&mut self) -> Handled {
        debug!(self.log(), "Starting...");
        Handled::block_on(self, move |mut async_self| async move {
            let resolver = resolver(
                config::ResolverConfig::default(),
                config::ResolverOpts::default(),
            )
            .await
            .expect("failed to connect resolver");
            async_self.resolver = Some(resolver);
            debug!(async_self.log(), "Started!");
        })
    }

    fn on_stop(&mut self) -> Handled {
        drop(self.resolver.take());
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        self.on_stop()
    }
}
// ANCHOR_END: lifecycle

// ANCHOR: actor
impl Actor for DnsComponent {
    type Message = Ask<DnsRequest, DnsResponse>;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        debug!(self.log(), "Got request for domain: {}", msg.request().0);
        if let Some(ref resolver) = self.resolver {
            let query_result_future = resolver.lookup(
                msg.request().0.clone(),
                RecordType::A,
                DnsRequestOptions::default(),
            );
            self.spawn_local(move |async_self| async move {
                let query_result = query_result_future.await.expect("dns query result");
                debug!(
                    async_self.log(),
                    "Got reply for domain: {}",
                    msg.request().0
                );
                let mut results: Vec<String> = Vec::new();
                for (index, ip) in query_result.iter().enumerate() {
                    results.push(format!("{}. {:?}", index, ip));
                }
                let result_string = format!("{}:\n   {}", msg.request().0, results.join("\n    "));
                msg.reply(DnsResponse(result_string)).expect("reply");
                Handled::Ok
            });
            Handled::Ok
        } else {
            panic!("Component should have been initialised first!")
        }
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!("ignore networking");
    }
}
// ANCHOR_END: actor

// ANCHOR: main
fn main() {
    let system = KompactConfig::default().build().expect("system");
    let dns_comp = system.create(DnsComponent::new);
    let dns_comp_ref = dns_comp.actor_ref().hold().expect("live");
    system.start_notify(&dns_comp).wait();
    println!("System is ready, enter your queries.");
    loop {
        let command = Input::<String>::new().with_prompt(">").interact();
        match command {
            Ok(s) => match s.as_ref() {
                "stop" => break,
                _ => {
                    let mut outstanding = Vec::new();
                    for domain in s.split(',') {
                        let domain = domain.trim();
                        info!(system.logger(), "Sending request for {}", domain);
                        let query_f = dns_comp_ref.ask(DnsRequest(domain.to_string()));
                        outstanding.push(query_f);
                    }
                    for query_f in outstanding {
                        let result = query_f.wait();
                        info!(system.logger(), "Got:\n    {}\n", result.0);
                    }
                }
            },
            Err(e) => error!(system.logger(), "Error with input: {}", e),
        }
    }
    system.kill_notify(dns_comp).wait();
    system.shutdown().expect("shutdown");
}
// ANCHOR_END: main
