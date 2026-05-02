use std::fs;
use anyhow::Result;
use std::path::{Path, PathBuf};
use include_dir::{include_dir, Dir};

// Embed templates into the binary
pub static TEMPLATES: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates");

//
// Extract template from embedded dir
//
pub fn extract_template(name: &str, dest: &Path) -> Result<()> {
    let dir = TEMPLATES
        .get_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Template '{}' not found", name))?;

    copy_embedded_dir(dir, dest)
}

//
// Copy embedded directory recursively
//
fn copy_embedded_dir(dir: &Dir, dest: &Path) -> Result<()> {
    for file in dir.files() {
        let rel_path = file.path();

        let stripped = rel_path
            .components()
            .skip(1) // remove template root
            .collect::<PathBuf>();

        let path = dest.join(stripped);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, file.contents())?;
    }

    for subdir in dir.dirs() {
        copy_embedded_dir(subdir, dest)?;
    }

    Ok(())
}
