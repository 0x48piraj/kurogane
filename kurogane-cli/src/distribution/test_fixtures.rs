//! Distribution fixtures for tests.

use std::path::Path;

pub use kurogane_layout::test_fixtures::{cef_runtime, tmp_dir};

use crate::distribution::{AppMetadata, Executable, ResolvedDistribution, ResolvedResource};

/// Creates a valid sample resolved distribution.
pub fn sample_distribution(dir: &Path) -> ResolvedDistribution {
    #[cfg(target_os = "windows")]
    let exe_name = "myapp.exe";
    #[cfg(not(target_os = "windows"))]
    let exe_name = "myapp";

    let exe = dir.join(exe_name);
    std::fs::write(&exe, "binary").unwrap();

    let frontend = dir.join("frontend");
    std::fs::create_dir_all(&frontend).unwrap();
    write(frontend.join("index.html"), "<html></html>");

    let cef = cef_runtime(&dir.join("cef"));
    for notice in crate::install::NOTICES {
        write(cef.join(notice), "notice");
    }

    let resource = dir.join("extra.txt");
    write(&resource, "data");

    let destination = resource
        .file_name()
        .map(Into::into)
        .unwrap_or_else(|| "extra.txt".into());

    ResolvedDistribution {
        metadata: AppMetadata {
            name: "myapp".to_string(),
            version: "1.0.0".to_string(),
            exe_name: exe_name.to_string(),
            ..Default::default()
        },
        executable: Executable::Application(exe),
        frontend: Some(frontend),
        cef_runtime: cef,
        extra_resources: vec![ResolvedResource {
            source: resource,
            destination,
        }],
    }
}

/// Creates a distribution shaped like a sandboxed Windows application:
/// CEF's bootstrap under the application's name, loading its library.
pub fn sandboxed_distribution(dir: &Path) -> ResolvedDistribution {
    let mut dist = sample_distribution(dir);

    let bootstrap = dir.join("bootstrap.exe");
    write(&bootstrap, "bootstrap");

    let library = dir.join("myapp_lib.dll");
    write(&library, "application");

    dist.metadata.exe_name = "myapp.exe".to_string();
    dist.executable = Executable::Bootstrap { bootstrap, library };

    dist
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent");
    }
    std::fs::write(path, contents).unwrap();
}
