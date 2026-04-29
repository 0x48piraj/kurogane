//! app:// scheme support for local assets.
//!
//! This is a constrained asset-serving boundary layer designed specifically for exposing
//! bundled application resources to the browser runtime.
//!
//! Converts app:// URLs into file reads while enforcing a sandbox rooted at
//! the configured asset directory.
//!
//! What it guarantees:
//! - No path traversal or root escape
//! - No symlink-based escapes
//! - No absolute path injection
//! - No filesystem details leaked to clients
//!
//! Design notes:
//! - The asset root is the only allowed filesystem boundary
//! - Focused on safe, predictable asset access within the runtime

use cef::*;
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use mime_guess::MimeGuess;
use url::Url;

use crate::debug;

/// Errors returned when resolving an app:// request.
/// Each variant maps to an HTTP status code.
#[derive(Debug)]
pub enum ResolveError {
    /// The URL could not be parsed, or its scheme is not app
    InvalidUrl,
    /// The resolved path escapes the asset root (path-traversal attempt)
    Forbidden(PathBuf),
    /// The path is inside the root but the file does not exist
    NotFound(PathBuf),
    /// An I/O error occurred after validation
    Io(std::io::Error),
}

impl ResolveError {
    pub fn http_status(&self) -> i32 {
        match self {
            Self::InvalidUrl => 400,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Io(_) => 500,
        }
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "Invalid URL"),
            Self::Forbidden(p) => write!(f, "Forbidden: {}", p.display()),
            Self::NotFound(p) => write!(f, "Not found: {}", p.display()),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

/// A successfully resolved file asset.
#[derive(Debug)]
pub struct ResolvedAsset {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub mime: String,
}

//
// SchemeHandlerFactory
//

wrap_scheme_handler_factory! {
    pub struct AppSchemeHandlerFactory;

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {

            Some(AppResourceHandler::new(
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(0usize)),
                Arc::new(Mutex::new(String::from("text/html"))),
                Arc::new(AtomicI32::new(200)),
            ))
        }
    }
}

//
// ResourceHandler
//

wrap_resource_handler! {
    pub struct AppResourceHandler {
        data: Arc<Mutex<Vec<u8>>>,
        offset: Arc<Mutex<usize>>,
        mime: Arc<Mutex<String>>,
        status: Arc<AtomicI32>,
    }

    impl ResourceHandler {

        /// Resolves and loads an app:// resource for the request.
        ///
        /// Populates response data and status code.
        fn open(
            &self,
            request: Option<&mut Request>,
            handle_request: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            let request = request.unwrap();
            let raw_url = CefString::from(&request.url()).to_string();

            // Resolve relative to CWD (set by resolver)
            let root = crate::runtime::Runtime::asset_root();

            let result = extract_rel_path(&raw_url)
                .and_then(|rel| resolve_asset(&root, &rel));

            match result {
                Ok(asset) => {
                    debug!(
                        "[kurogane] status=200 url=\"{}\" path=\"{}\" bytes={} mime={}",
                        raw_url,
                        asset.path.display(),
                        asset.bytes.len(),
                        asset.mime
                    );

                    *self.data.lock().unwrap() = asset.bytes;
                    *self.offset.lock().unwrap() = 0;
                    *self.mime.lock().unwrap() = asset.mime;
                    self.status.store(200, Ordering::Release);
                }
                Err(e) => {
                    let status = e.http_status();

                    match &e {
                        ResolveError::Forbidden(path) |
                        ResolveError::NotFound(path) => {
                            eprintln!(
                                "[kurogane] status={} url=\"{}\" path=\"{}\" reason={:?}",
                                status,
                                raw_url,
                                path.display(),
                                e
                            );
                        }
                        _ => {
                            eprintln!(
                                "[kurogane] status={} url=\"{}\" reason={:?}",
                                status,
                                raw_url,
                                e
                            );
                        }
                    }

                    let body = match e {
                        ResolveError::InvalidUrl => b"400 Bad Request".to_vec(),
                        ResolveError::Forbidden(_) => b"403 Forbidden".to_vec(),
                        ResolveError::NotFound(_) => b"404 Not Found".to_vec(),
                        ResolveError::Io(_) => b"500 Internal Server Error".to_vec(),
                    };

                    self.status.store(status, Ordering::Release);
                    *self.data.lock().unwrap() = body;
                    *self.offset.lock().unwrap() = 0;
                    *self.mime.lock().unwrap() = "text/plain".into();
                }
            }

            if let Some(hr) = handle_request {
                *hr = 1;
            }

            1
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: i32,
            bytes_read: Option<&mut i32>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> i32 {
            let br = bytes_read.unwrap();
 
            // Avoid invalid cast guard
            if bytes_to_read <= 0 {
                *br = 0;
                return 0;
            }

            let mut offset = self.offset.lock().unwrap();
            let data = self.data.lock().unwrap();

            debug_assert!(*offset <= data.len(), "offset invariant broken");

            let remaining = &data[*offset..];
            let read = remaining.len().min(bytes_to_read as usize);

            if read > 0 {
                // Safety: writes at most bytes_to_read into valid CEF buffer
                unsafe {
                    std::ptr::copy_nonoverlapping(remaining.as_ptr(), data_out, read);
                }
                *offset += read;
                debug_assert!(*offset <= data.len(), "offset exceeded buffer length");
            }

            *br = read as i32;

            if read == 0 {
                return 0; // EOF
            }

            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let response = response.unwrap();

            let status = self.status.load(Ordering::Acquire);
            let data_len = self.data.lock().unwrap().len() as i64;
            let mime = self.mime.lock().unwrap().clone();

            response.set_status(status);
            response.set_mime_type(Some(&CefString::from(mime.as_str())));

            if let Some(len) = response_length {
                *len = data_len;
            }
        }
    }
}

//
// Helpers
//

/// Extracts a relative path from an app:// URL.
/// Defaults to "index.html" for empty paths.
/// Query strings and fragments are intentionally ignored.
pub fn extract_rel_path(raw_url: &str) -> Result<String, ResolveError> {
    let parsed = Url::parse(raw_url)
        .map_err(|_| ResolveError::InvalidUrl)?;

    if parsed.scheme() != "app" {
        return Err(ResolveError::InvalidUrl);
    }

    let rel = parsed.path().trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    Ok(rel.to_owned())
}

/// Resolves a request path relative to root and returns a canonical path
/// inside the allowed filesystem boundary.
pub fn safe_join(root: &Path, request: &str) -> Result<PathBuf, ResolveError> {
    // Canonical root defines the sandbox boundary
    let root = root
        .canonicalize()
        .map_err(ResolveError::Io)?;

    let joined = root.join(request);

    // Canonicalize and distinguish 404 (file missing) from 403 (path escapes root)
    let canonical = joined
        .canonicalize()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ResolveError::NotFound(joined)
            } else {
                ResolveError::Io(e)
            }
        })?;

    if !canonical.starts_with(&root) {
        return Err(ResolveError::Forbidden(canonical));
    }

    Ok(canonical)
}

/// Loads a file under root and returns its bytes and MIME type.
pub fn resolve_asset(root: &Path, rel_path: &str) -> Result<ResolvedAsset, ResolveError> {
    let path = safe_join(root, rel_path)?;

    let bytes = std::fs::read(&path).map_err(|e| ResolveError::Io(e))?;

    let mime = mime_from_path(&path);

    debug!(
        "[app://] 200  {}  ({}, {} bytes)",
        path.display(),
        mime,
        bytes.len()
    );

    Ok(ResolvedAsset {
        path,
        bytes,
        mime,
    })
}

/// Returns the MIME type for a given path based on its file extension.
/// Unknown extensions fall back to 'application/octet-stream'.
fn mime_from_path(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        // App-specific overrides
        Some("js") | Some("mjs") | Some("cjs") => {
            "application/javascript".to_string()
        }
        // Note: MIME resolution depends on mime_guess crate. Dependency updates can be fatal.
        _ => MimeGuess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_owned(),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    // URL parsing and normalization tests

    #[test]
    fn rel_path_standard_file() {
        assert_eq!(
            extract_rel_path("app://app/index.html").unwrap(),
            "index.html"
        );
    }

    #[test]
    fn rel_path_nested() {
        assert_eq!(
            extract_rel_path("app://app/static/app.js").unwrap(),
            "static/app.js"
        );
    }

    #[test]
    fn rel_path_root_slash_defaults_to_index() {
        assert_eq!(extract_rel_path("app://app/").unwrap(), "index.html");
    }

    #[test]
    fn rel_path_bare_host_defaults_to_index() {
        assert_eq!(extract_rel_path("app://app").unwrap(), "index.html");
    }

    #[test]
    fn rel_path_query_string_is_stripped() {
        // Query params are irrelevant for static file serving
        assert_eq!(
            extract_rel_path("app://app/page.html?v=2").unwrap(),
            "page.html"
        );
    }

    #[test]
    fn rel_path_fragment_is_stripped() {
        assert_eq!(
            extract_rel_path("app://app/page.html#section").unwrap(),
            "page.html"
        );
    }

    #[test]
    fn rel_path_rejects_wrong_scheme() {
        let err = extract_rel_path("https://example.com/foo").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidUrl));
        assert_eq!(err.http_status(), 400);
    }

    #[test]
    fn rel_path_rejects_malformed_url() {
        let err = extract_rel_path("not a url at all").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidUrl));
    }

    // Path safety and traversal checks

    #[test]
    fn safe_join_resolves_existing_file() {
        let dir = tmp();
        fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
        let path = safe_join(dir.path(), "hello.txt").unwrap();
        assert!(path.is_file());
        assert!(path.ends_with("hello.txt"));
    }

    #[test]
    fn safe_join_resolves_nested_file() {
        let dir = tmp();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/page.html"), b"<h1>hi</h1>").unwrap();
        let path = safe_join(dir.path(), "sub/page.html").unwrap();
        assert!(path.ends_with("page.html"));
    }

    #[test]
    fn safe_join_not_found_for_missing_file() {
        let dir = tmp();
        let err = safe_join(dir.path(), "missing.txt").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
        assert_eq!(err.http_status(), 404);
    }

    #[test]
    fn safe_join_forbidden_for_traversal_to_existing_file() {
        // Traversal escapes root to an existing file; must be rejected (403)
        let parent = tmp();
        let root = parent.path().join("assets");
        fs::create_dir(&root).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();
 
        let err = safe_join(&root, "../secret.txt").unwrap_err();
        assert!(matches!(err, ResolveError::Forbidden(_)));
        assert_eq!(err.http_status(), 403);
    }

    #[test]
    fn safe_join_not_found_for_traversal_to_missing_file() {
        // Traversal to non-existent target is indistinguishable from in-root miss without
        // an exists() check (which would be TOCTOU).
        let dir = tmp();
        let err = safe_join(dir.path(), "../no_such_file.txt").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[test]
    fn safe_join_rejects_buried_traversal() {
        let parent = tmp();
        let root = parent.path().join("assets");
        fs::create_dir(&root).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();
 
        let err = safe_join(&root, "a/b/../../../../secret.txt").unwrap_err();
        assert!(matches!(
            err,
            ResolveError::Forbidden(_) | ResolveError::NotFound(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn safe_join_denied_for_absolute_path_injection() {
        let dir = tmp();
        let err = safe_join(dir.path(), "/etc/passwd").unwrap_err();
        assert!(matches!(
            err,
            ResolveError::Forbidden(_) | ResolveError::NotFound(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn safe_join_forbidden_for_symlink_escaping_root() {
        use std::os::unix::fs::symlink;
        let parent = tmp();
        let root = parent.path().join("assets");
        fs::create_dir(&root).unwrap();
        // External file targeted via symlink inside root
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();
        symlink(parent.path().join("secret.txt"), root.join("escape")).unwrap();
 
        let err = safe_join(&root, "escape").unwrap_err();
        assert!(matches!(err, ResolveError::Forbidden(_)));
        assert_eq!(err.http_status(), 403);
    }

    // MIME detection tests

    #[test]
    fn mime_common_web_types() {
        let cases = [
            ("index.html", "text/html"),
            ("style.css", "text/css"),
            ("data.json", "application/json"),
            ("image.png", "image/png"),
            ("font.woff2", "font/woff2"),
        ];
        for (file, expected) in cases {
            assert_eq!(
                mime_from_path(Path::new(file)),
                expected,
                "failed for {file}"
            );
        }
    }

    #[test]
    fn mime_all_js_module_variants() {
        for ext in ["js", "mjs", "cjs"] {
            assert_eq!(
                mime_from_path(Path::new(&format!("module.{ext}"))),
                "application/javascript",
                ".{ext} must be application/javascript"
            );
        }
    }

    #[test]
    fn mime_unknown_extension_is_octet_stream() {
        assert_eq!(
            mime_from_path(Path::new("file.unknownext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn mime_double_extension_uses_last() {
        assert_eq!(
            mime_from_path(Path::new("archive.tar.gz")),
            "application/gzip"
        );
    }

    #[test]
    fn mime_no_extension_is_octet_stream() {
        assert_eq!(
            mime_from_path(Path::new("Makefile")),
            "application/octet-stream"
        );
    }

    // File loading tests

    #[test]
    fn resolve_asset_returns_correct_bytes_and_mime() {
        let dir = tmp();
        fs::write(dir.path().join("app.js"), b"console.log('hi')").unwrap();
        let asset = resolve_asset(dir.path(), "app.js").unwrap();
        assert_eq!(asset.mime, "application/javascript");
        assert_eq!(asset.bytes, b"console.log('hi')");
    }

    #[test]
    fn resolve_asset_404_propagates() {
        let dir = tmp();
        let err = resolve_asset(dir.path(), "nope.html").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
        assert_eq!(err.http_status(), 404);
    }

    #[test]
    fn resolve_asset_empty_file_is_ok() {
        let dir = tmp();
        fs::write(dir.path().join("empty.js"), b"").unwrap();
        let asset = resolve_asset(dir.path(), "empty.js").unwrap();
        assert!(asset.bytes.is_empty());
        assert_eq!(asset.mime, "application/javascript");
    }

    // Status mapping and formatting tests

    #[test]
    fn error_http_status_codes() {
        assert_eq!(ResolveError::InvalidUrl.http_status(), 400);
        assert_eq!(ResolveError::Forbidden(PathBuf::new()).http_status(), 403);
        assert_eq!(ResolveError::NotFound(PathBuf::new()).http_status(), 404);
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "disk on fire");
        assert_eq!(ResolveError::Io(io_err).http_status(), 500);
    }

    #[test]
    fn error_display_is_human_readable() {
        let s = ResolveError::InvalidUrl.to_string();
        assert!(s.contains("Invalid URL"));

        let s = ResolveError::Forbidden(PathBuf::from("/etc/passwd")).to_string();
        assert!(s.contains("Forbidden"));
        assert!(s.contains("passwd"));
    }
}
