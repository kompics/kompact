use kompact::prelude::*;

#[derive(Debug)]
struct TestPort;
impl Port for TestPort {
    type Indication = String;
    type Request = usize;
}

#[derive(ComponentDefinition)] //~ ERROR: Ambiguous port type: There are multiple fields with type TestPort (["provided_test_port1", "provided_test_port2"]). You cannot derive ComponentDefinition in these cases, as you must resolve the ambiguity manually.
struct DuplicatePortComponent {
    ctx: ComponentContext<Self>,
    provided_test_port1: ProvidedPort<TestPort>,
    provided_test_port2: ProvidedPort<TestPort>,
    required_test_port1: RequiredPort<TestPort>,
    required_test_port2: RequiredPort<TestPort>,
}

fn main() { 
    // unused 
}
