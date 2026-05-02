use std::fs;
use anyhow::Result;
use std::path::Path;
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
    // Ensure the destination directory exists
    fs::create_dir_all(dest)?;

    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                // Grab just the dir name
                let dir_name = subdir.path().file_name().expect("Directories should have valid names");
                let new_dest = dest.join(dir_name);

                // Recurse into this subdirectory with the new destination
                copy_embedded_dir(subdir, &new_dest)?;
            }

            include_dir::DirEntry::File(file) => {
                // Grab just the filename
                let file_name = file.path().file_name().expect("Files should have valid names");
                let file_path = dest.join(file_name);

                // Write the file
                fs::write(file_path, file.contents())?;
            }
        }
    }

    Ok(())
}
