//! Generates Rust bindings from `schema/*.capnp` at build time.

fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("schema")
        .file("schema/block.capnp")
        .run()
        .expect("capnp schema compilation failed");
}
