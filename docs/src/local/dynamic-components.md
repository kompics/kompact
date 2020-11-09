# Dynamic Components

Kompact is a very strictly and statically typed framework. Sometimes, however, it is beneficial to be a little more dynamic.
There are many reasons you might want to introduce some dynamism into your component system: modularity, ease of modeling,
or sometimes even performance (more static -> more monomorphisation -> code bloat -> cache misses).

Because of this, we introduced a way to deal with components with a little bit of dynamic typing. Namely, you're able
to create components from type-erased definitions with `{System,SystemHandle}::create_erased` (nightly only), and query 
type-erased components for ports they may provide and/or require with `on_dyn_definition` and `get_{provided,required}_port`.

> **Note:** while creating type-erased components from **type-erased definitions** _is_ nightly-only, you can create component
> just normally and then cast it to a type-erased component on stable.

Let's create a dynamic interactive system showcasing these features. We'll build a little REPL which the user can use
to spawn some components, set their settings, and send them some data to process.

First some basic components:
```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/dynamic_components.rs:simple_components}}
```

Our components perform simple arithmetic operations on the incoming message and log the results (as well as their 
lifecycle). The internal state of the components can be set via `Set{Offset,Scale}` ports. So far we just have components
with _either_ a scale _or_ an offset. Let's add something slightly more interesting, which uses both.

 ```rust,edition2018,no_run,noplaypen
 {{#rustdoc_include ../../examples/src/bin/dynamic_components.rs:linear}}
 ```

Now let's write a manager component, which will take care of creating the components described above, killing them, 
modifying their settings, and sending them data to process. In this case we have just three different types of worker
components, but imagine we had tens (still sharing the same message type and some subsets of "settings"). In that case
it would be **very** tedious to manage all these component types explicitly.

 ```rust,edition2018,no_run,noplaypen
 {{#rustdoc_include ../../examples/src/bin/dynamic_components.rs:storage}}
 ```

Using `Arc<dyn AbstractComponent<Message=M>>` we can mix different components that take the same type of message in one
collection. Now to fill that `Vec` with something useful. We'll define some messages for the manager and start creating 
some components.

 ```rust,edition2018,no_run,noplaypen
 {{#rustdoc_include ../../examples/src/bin/dynamic_components.rs:creation}}
 ```

As we don't want the manager type to know about the concrete component types _at all_, the `Spawn` message above contains a
boxed, type-erased component definition, which we then turn into a component using `create_erased`.

Normally, after creating the components we would connect the ports to each other using `connect_to_required`, or maybe
`on_definition` and direct port access. However, all of those require concrete types, like `Arc<Component<Adder>>`
or `Arc<Component<Linear>>`, which is not what we get here (`Arc<dyn AbstractComponent<Message=f32>`). Instead we can use
`on_dyn_definition` together with the `Option`-returning `get_{provided,required}_port` to dynamically check if a given
port exists on the abstract component and, if so, fetch it. 

 ```rust,edition2018,no_run,noplaypen
 {{#rustdoc_include ../../examples/src/bin/dynamic_components.rs:ports}}
 ```

Now that we have the dynamic component part done, we can write a very simple repl. We'll start the Kompact system in the
main thread, create the manager there, and await system termination. In a separate thread we'll continuously read `stdin`
and interpret the lines as commands to send to the manager.

 ```rust,edition2018,no_run,noplaypen
 {{#rustdoc_include ../../examples/src/bin/dynamic_components.rs:repl}}
 ```

When ran, it looks something like this:
```
❯ cargo run --features=type_erasure,silent_logging --bin dynamic_components
   Compiling kompact-examples v0.10.0 (/home/mrobakowski/projects/kompact/docs/examples)
    Finished dev [unoptimized + debuginfo] target(s) in 2.56s
     Running `/home/mrobakowski/projects/kompact/target/debug/dynamic_components`
compute 1
spawn adder
Nov 09 00:55:59.917 INFO Starting..., ctype: Adder, cid: 79bd396b-de75-4284-bc57-e0cf8193f72f, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:39
set offset 5
compute 1
Nov 09 00:56:43.465 INFO Adder result = 6, ctype: Adder, cid: 79bd396b-de75-4284-bc57-e0cf8193f72f, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:46
spawn multiplier
Nov 09 00:56:55.518 INFO Starting..., ctype: Multiplier, cid: 47dd4827-8d35-4351-a717-344ec7fe70fe, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:85
set scale 2
compute 2
Nov 09 00:57:09.684 INFO Adder result = 7, ctype: Adder, cid: 79bd396b-de75-4284-bc57-e0cf8193f72f, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:46
Nov 09 00:57:09.684 INFO Multiplier result = 4, ctype: Multiplier, cid: 47dd4827-8d35-4351-a717-344ec7fe70fe, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:92
kill all
Nov 09 00:57:17.769 INFO Killing..., ctype: Adder, cid: 79bd396b-de75-4284-bc57-e0cf8193f72f, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:39
Nov 09 00:57:17.769 INFO Killing..., ctype: Multiplier, cid: 47dd4827-8d35-4351-a717-344ec7fe70fe, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:85
spawn linear
Nov 09 00:57:24.840 INFO Starting..., ctype: Linear, cid: d0f01d1a-b448-4b5f-bddd-701d764992ea, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:135
spawn adder
Nov 09 00:57:32.136 INFO Starting..., ctype: Adder, cid: c3b9a0c5-875d-4e1d-8c70-3b414fe2a7bb, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:39
set offset 2
set scale 3
compute 4
Nov 09 00:57:41.558 INFO Linear result = 14, ctype: Linear, cid: d0f01d1a-b448-4b5f-bddd-701d764992ea, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:142
Nov 09 00:57:41.558 INFO Adder result = 6, ctype: Adder, cid: c3b9a0c5-875d-4e1d-8c70-3b414fe2a7bb, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:46
quit
Nov 09 00:57:51.351 INFO Killing..., ctype: Linear, cid: d0f01d1a-b448-4b5f-bddd-701d764992ea, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:135
Nov 09 00:57:51.352 INFO Killing..., ctype: Adder, cid: c3b9a0c5-875d-4e1d-8c70-3b414fe2a7bb, system: kompact-runtime-1, location: docs/examples/src/bin/dynamic_components.rs:39

```
