fn main() {
    protobuf_codegen::Codegen::new()
        .pure()
        .cargo_out_dir("generated")
        .input("src/protos/example.proto")
        .include("src/protos")
        .run_from_script();
}
