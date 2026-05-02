use anyhow::Result;
use std::{fs, path::Path};
use include_dir::{include_dir, Dir, DirEntry};

// Embed templates into the binary
pub static TEMPLATES: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// Extracts an embedded template into the destination directory.
pub fn extract_template(name: &str, dest: &Path) -> Result<()> {
    let dir = TEMPLATES
        .get_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Template '{}' not found", name))?;

    copy_embedded_dir(dir, dest)
}

/// Copy embedded directory recursively
/// Assumes 'dest' exists or is handled by the caller.
fn copy_embedded_dir(dir: &Dir, dest: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(subdir) => {
                // Grab just the dir name
                let dir_name = subdir
                    .path()
                    .file_name()
                    .expect("Directory should have valid name");
                let new_dest = dest.join(dir_name);

                // Create dir only when encountered during traversal
                fs::create_dir_all(&new_dest)?;
                // Recurse into this subdirectory with the new destination
                copy_embedded_dir(subdir, &new_dest)?;
            }

            DirEntry::File(file) => {
                // Grab just the filename
                let file_name = file
                    .path()
                    .file_name()
                    .expect("File should have valid name");

                let file_path = dest.join(file_name);

                // Write the file
                fs::write(file_path, file.contents())?;
            }
        }
    }

    Ok(())
}
