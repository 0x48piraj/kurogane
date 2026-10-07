use cargo_metadata::MetadataCommand;

fn main() {
    // Ask Cargo for resolved dependency graph
    let metadata = MetadataCommand::new()
        .exec()
        .expect("failed to read cargo metadata");

    // `links = "tetsu"` admits one tetsu-sys in a dependency graph
    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == "tetsu-sys")
        .expect("tetsu-sys not found in dependency graph");

    // The build metadata is the CEF version, `154.0.33` in `154.4.0+154.0.33`
    let cef_version = pkg.version.build.as_str();
    assert!(
        !cef_version.is_empty(),
        "tetsu-sys {} names no CEF version",
        pkg.version
    );

    println!("cargo::rustc-env=KUROGANE_CEF_VERSION={cef_version}");

    // Re-run if dependency graph changes; the lock file sits at the workspace root
    println!(
        "cargo::rerun-if-changed={}",
        metadata.workspace_root.join("Cargo.lock")
    );
}
