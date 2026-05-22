//! Generates Rust bindings from `schema/*.capnp` at build time.

fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("schema")
        .file("schema/bitcoin_block.capnp")
        .file("schema/bitcoin_stats.capnp")
        .run()
        .expect("capnp schema compilation failed");
}