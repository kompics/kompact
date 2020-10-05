extern crate protoc_rust;


fn main() {
	protoc_rust::Codegen::new()
        .out_dir("src/messages")
        .inputs(&["./proto/example.proto"])
        .include("./proto")
        .run()
        .expect("Running protoc failed.");
}
