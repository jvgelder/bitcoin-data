fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("schema")
        .file("schema/light.capnp")
        .run()
        .expect("capnp light schema compilation failed");
}
