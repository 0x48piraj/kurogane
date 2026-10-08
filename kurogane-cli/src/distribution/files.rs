//! Directory copying and linking for bundles.

use std::path::Path;

/// Mirrors `src` into `dst`, reusing files where possible.
///
/// A staged CEF runtime is hundreds of megabytes of read-only files that are
/// already on disk, so it is hard linked rather than copied. Linking is
/// refused across volumes and on filesystems without hard links and each
/// file falls back to a copy.
///
/// `keep` is called for every entry by name at each directory level.
pub fn link_dir(src: &Path, dst: &Path, keep: &dyn Fn(&str) -> bool) -> std::io::Result<()> {
    mirror_dir(src, dst, keep, &link_file)
}

pub fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    copy_dir_filtered(src, dst, &|_| true)
}

/// Copies `src` into `dst`, leaving out every entry `keep` refuses.
///
/// `keep` is asked about every entry by name, at every level.
pub(crate) fn copy_dir_filtered(
    src: &Path,
    dst: &Path,
    keep: &dyn Fn(&str) -> bool,
) -> std::io::Result<()> {
    mirror_dir(src, dst, keep, &|src, dst| {
        std::fs::copy(src, dst).map(drop)
    })
}

/// Mirrors the directory structure from `src` into `dst`.
///
/// Kept files are passed to `place` for copying or linking.
fn mirror_dir(
    src: &Path,
    dst: &Path,
    keep: &dyn Fn(&str) -> bool,
    place: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();

        if !keep(&name.to_string_lossy()) {
            continue;
        }

        let path = entry.path();
        let dest = dst.join(&name);

        if path.is_dir() {
            mirror_dir(&path, &dest, keep, place)?;
        } else {
            place(&path, &dest)?;
        }
    }

    Ok(())
}

/// Links one file into place, leaving an up-to-date destination alone.
///
/// Reuses the destination when its metadata matches the source.
fn link_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    let source = std::fs::metadata(src)?;

    if is_same_file(&source, dst) {
        return Ok(());
    }

    // A stale link has to go before a fresh one can take its name
    match std::fs::remove_file(dst) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    if std::fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }

    std::fs::copy(src, dst)?;

    Ok(())
}

/// Returns whether `dst` already holds what `source` describes.
fn is_same_file(source: &std::fs::Metadata, dst: &Path) -> bool {
    let Ok(existing) = std::fs::metadata(dst) else {
        return false;
    };

    if existing.len() != source.len() {
        return false;
    }

    match (existing.modified(), source.modified()) {
        (Ok(existing), Ok(source)) => existing == source,
        _ => false,
    }
}
