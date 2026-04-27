#![allow(clippy::unused_unit)]
use async_std_resolver::{AsyncStdResolver, config, proto::rr::record_type::RecordType, resolver};
use dialoguer::Input;
use kompact::prelude::*;

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
    fn on_start(&mut self) -> HandlerResult {
        debug!(self.log(), "Starting...");
        Handled::block_on(self, async move |mut async_self| {
            let resolver = resolver(
                config::ResolverConfig::default(),
                config::ResolverOpts::default(),
            )
            .await;
            async_self.resolver = Some(resolver);
            debug!(async_self.log(), "Started!");
            Handled::OK
        })
    }

    fn on_stop(&mut self) -> HandlerResult {
        drop(self.resolver.take());
        Handled::OK
    }

    fn on_kill(&mut self) -> HandlerResult {
        self.on_stop()
    }
}
// ANCHOR_END: lifecycle

// ANCHOR: actor
impl Actor for DnsComponent {
    type Message = Ask<DnsRequest, DnsResponse>;

    fn receive_local(&mut self, msg: Self::Message) -> HandlerResult {
        debug!(self.log(), "Got request for domain: {}", msg.request().0);
        if let Some(resolver) = self.resolver.clone() {
            let domain = msg.request().0.clone();
            self.spawn_local(async move |async_self| {
                let query_result = resolver
                    .lookup(domain.clone(), RecordType::A)
                    .await
                    .expect("dns query result");
                debug!(async_self.log(), "Got reply for domain: {}", domain);
                let mut results: Vec<String> = Vec::new();
                for (index, ip) in query_result.iter().enumerate() {
                    results.push(format!("{}. {:?}", index, ip));
                }
                let result_string = format!("{}:\n   {}", domain, results.join("\n    "));
                msg.reply(DnsResponse(result_string)).expect("reply");
                Handled::OK
            });
            Handled::OK
        } else {
            panic!("Component should have been initialised first!")
        }
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
