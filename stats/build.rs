fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("schema")
        .file("schema/stats.capnp")
        .run()
        .expect("capnp stats schema compilation failed");
}
