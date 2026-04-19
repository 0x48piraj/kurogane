use cargo_metadata::MetadataCommand;

fn main() {
    // Ask Cargo for resolved dependency graph
    let metadata = MetadataCommand::new()
        .exec()
        .expect("failed to read cargo metadata");

    // Find cef-dll-sys package
    let pkg = metadata.packages.iter()
        .find(|p| p.name == "cef-dll-sys")
        .expect("cef crate not found in dependency graph");

    let version = pkg.version.to_string();

    // Extract Chromium version (after "+")
    let cef_version = version
        .split('+')
        .nth(1)
        .expect("invalid cef version format");

    println!("cargo:rustc-env=KUROGANE_CEF_VERSION={}", cef_version);

    // Re-run if dependency graph changes
    println!("cargo:rerun-if-changed=Cargo.lock");
}
