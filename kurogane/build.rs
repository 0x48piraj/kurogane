use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/bridge/runtime.js");

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by Cargo");
    let bridge = fs::read_to_string("src/bridge/runtime.js")
        .expect("failed to read src/bridge/runtime.js")
        .replace("__KUROGANE_VERSION__", &version);

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    fs::write(out.join("kurogane-bridge.js"), bridge).expect("failed to write generated bridge");
}
