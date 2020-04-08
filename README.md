Kompact
=======

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/kompics/kompact)
[![Cargo](https://img.shields.io/crates/v/kompact.svg)](https://crates.io/crates/kompact)
[![Documentation](https://docs.rs/kompact/badge.svg)](https://docs.rs/kompact)
[![Build Status](https://travis-ci.org/kompics/kompact.svg?branch=master)](https://travis-ci.org/kompics/kompact)

Kompact is an in-development message-passing component system like [Kompics](https://kompics.github.io/docs/current/) in the Rust language, with performance and static typing in mind. It merges both Kompics' component model and the actor model as found in [Erlang](http://www.erlang.se/) or [Akka](https://akka.io/).

Kompact has [shown](https://kompics.github.io/kompicsbenches/) itself to vastly outperform many of its peers on a wide range of message-passing tasks, providing the capability of handling up to 400mio messages per second on 36 cores.

Kompact comes with its own network library built-in, providing easy connection maintenance and efficient serialisation for distributed deployments.

## Documentation

For reference and short examples check the [API Docs](https://docs.rs/kompact).

A thorough introduction with many longer examples can be found in the [tutorial](https://kompics.github.io/kompact/). The examples themselves can be found in the [docs/examples](docs/examples) folder.

#### Rust Version

Kompact before `v0.9.0` requires Rust `nightly`.

Since `v0.9.0` Kompact can be built on stable Rust, with a slightly different API.

## License

Licensed under the terms of MIT license.

See [LICENSE](LICENSE) for details.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Kompact by you shall be licensed as MIT, without any additional terms or conditions.
