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
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use percent_encoding::percent_decode_str;
use mime_guess::MimeGuess;
use url::Url;

use crate::debug;

/// Errors returned when resolving an app:// request.
/// Each variant maps to an HTTP status code.
#[derive(Debug)]
pub enum ResolveError {
    /// The URL could not be parsed, or its scheme is not app
    InvalidUrl,
    /// The configured asset root is invalid (exists but is not a directory)
    InvalidRoot(PathBuf),
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
            Self::InvalidRoot(_) => 500,
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
            Self::InvalidRoot(p) => write!(f, "Invalid asset root: {}", p.display()),
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

#[derive(Clone, Debug)]
pub struct CanonicalRoot(PathBuf);

impl CanonicalRoot {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, ResolveError> {
        let canonical = path.as_ref()
            .canonicalize()
            .map_err(ResolveError::Io)?;

        if !canonical.is_dir() {
            return Err(ResolveError::InvalidRoot(canonical));
        }

        Ok(Self(canonical))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for CanonicalRoot {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

//
// SchemeHandlerFactory
//

wrap_scheme_handler_factory! {
    pub struct AppSchemeHandlerFactory;

    impl SchemeHandlerFactory {
        /// Resolves and loads an app:// resource for the request.
        ///
        /// Populates response data and status code.
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {

            let request = request.unwrap();
            let raw_url = CefString::from(&request.url()).to_string();

            // Resolve relative to CWD (set by resolver)
            let root = crate::runtime::Runtime::asset_root();

            let (data, mime, status) = match extract_rel_path(&raw_url)
                .and_then(|rel| resolve_asset(&root, &rel))
            {
                Ok(asset) => {
                    debug!(
                        "[kurogane] status=200 url=\"{}\" path=\"{}\" bytes={} mime={}",
                        raw_url,
                        asset.path.display(),
                        asset.bytes.len(),
                        asset.mime
                    );

                    (
                        Arc::<[u8]>::from(asset.bytes),
                        asset.mime,
                        200,
                    )
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

                    let body: Vec<u8> = match status {
                        400 => b"400 Bad Request".to_vec(),
                        403 => b"403 Forbidden".to_vec(),
                        404 => b"404 Not Found".to_vec(),
                        _ => b"500 Internal Server Error".to_vec(),
                    };

                    (
                        Arc::<[u8]>::from(body),
                        "text/plain".to_string(),
                        status,
                    )
                }
            };

            Some(AppResourceHandler::new(
                data,
                Arc::new(AtomicUsize::new(0)),
                mime,
                Arc::new(AtomicI32::new(status)),
            ))
        }
    }
}

//
// ResourceHandler
//

wrap_resource_handler! {
    pub struct AppResourceHandler {
        data: Arc<[u8]>,
        offset: Arc<AtomicUsize>,
        mime: String,
        status: Arc<AtomicI32>,
    }

    impl ResourceHandler {

        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            self.offset.store(0, Ordering::Release);

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

            // FFI safety guard (invalid pointer or non-positive length)
            if bytes_to_read <= 0 || data_out.is_null() {
                *br = 0;
                return 0;
            }

            let offset = self.offset.load(Ordering::Acquire);
            let data = self.data.as_ref();

            debug_assert!(offset <= data.len(), "offset invariant broken");

            let remaining = &data[offset..];
            let read = remaining.len().min(bytes_to_read as usize);

            if read > 0 {
                // Safety: writes at most bytes_to_read into valid CEF buffer
                unsafe {
                    std::ptr::copy_nonoverlapping(remaining.as_ptr(), data_out, read);
                }

                self.offset.fetch_add(read, Ordering::AcqRel);

                debug_assert!(offset + read <= data.len(), "offset exceeded buffer length");
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
            let data_len = self.data.len() as i64;

            response.set_status(status);
            response.set_mime_type(Some(&CefString::from(self.mime.as_str())));

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

    // Impose scheme rule
    if parsed.scheme() != "app" {
        return Err(ResolveError::InvalidUrl);
    }

    // Impose host rule
    if parsed.host_str().is_some_and(|h| h != "app") {
        return Err(ResolveError::InvalidUrl);
    }

    // Percent-decode
    let decoded = percent_decode_str(parsed.path())
        .decode_utf8()
        .map_err(|_| ResolveError::InvalidUrl)?; // reject invalid UTF-8

    let rel = decoded.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    Ok(rel.to_owned())
}

/// Resolves a request path relative to root and returns a canonical path
/// inside the allowed filesystem boundary.
pub fn safe_join(root: &CanonicalRoot, request: &str) -> Result<PathBuf, ResolveError> {
    let joined = root.as_path().join(request);

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

    if !canonical.starts_with(root.as_path()) {
        return Err(ResolveError::Forbidden(canonical));
    }

    Ok(canonical)
}

/// Loads a file under root and returns its bytes and MIME type.
pub fn resolve_asset(root: &CanonicalRoot, rel_path: &str) -> Result<ResolvedAsset, ResolveError> {
    let path = safe_join(root, rel_path)?;

    // Treat directories as not found
    if !path.is_file() {
        return Err(ResolveError::NotFound(path));
    }

    let bytes = std::fs::read(&path).map_err(|e| ResolveError::Io(e))?;

    let mime = mime_from_path(&path);

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

/// Computes a deterministic FNV-1a 64-bit hash of a filesystem path.
/// Intended for identity stability, not cryptographic use.
pub fn fnv1a_64(path: &Path) -> String {
    let mut hash: u64 = 14695981039346656037;

    for byte in path.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }

    format!("{:016x}", hash)
}

/// Sanitizes a user-provided name into a filesystem-safe identifier.
/// Returns "default" when the input cannot be reduced to a valid name.
pub fn sanitize_name(name: &str) -> String {
    // Windows reserved names
    const WINDOWS_RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    // Replace forbidden/control chars with _
    let replaced = name.chars().map(|c| match c {
        '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
        _ if c.is_control() => '_',
        _ => c,
    }).collect::<String>();

    // Collapse consecutive _ for aesthetics
    let mut sanitized = replaced
        .chars()
        .fold(String::new(), |mut acc, c| {
            if c == '_' && acc.ends_with('_') {
                acc
            } else {
                acc.push(c);
                acc
            }
        });

    // Trim Windows-invalid endings
    sanitized = sanitized.trim_end_matches(['.', ' ']).to_string();

    // Trim leading dots
    sanitized = sanitized.trim_start_matches('.').to_string();

    let stem = sanitized.split('.').next().unwrap();

    if WINDOWS_RESERVED.iter().any(|&r| r.eq_ignore_ascii_case(stem)) {
        sanitized = format!("_{sanitized}");
    }

    // Character length limit
    const MAX_LEN: usize = 64;
    sanitized = sanitized
        .chars()
        .take(MAX_LEN)
        .collect();

    // Fallback if empty, or _ for aesthetics, again
    if sanitized.is_empty() || sanitized.chars().all(|c| c == '_') {
        return "kurogane-app".to_string();
    }

    sanitized
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
    fn rel_path_rejects_wrong_host() {
        let err = extract_rel_path("app://evil/foo").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidUrl));
        assert_eq!(err.http_status(), 400);
    }

    #[test]
    fn rel_path_rejects_malformed_url() {
        let err = extract_rel_path("not a url at all").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidUrl));
    }

    #[test]
    fn rel_path_decodes_percent_encoding() {
        assert_eq!(
            extract_rel_path("app://app/My%20File.html").unwrap(),
            "My File.html"
        );
    }

    // Path safety and traversal checks

    #[test]
    fn safe_join_resolves_existing_file() {
        let dir = tmp();
        fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let path = safe_join(&root, "hello.txt").unwrap();
        assert!(path.is_file());
        assert!(path.ends_with("hello.txt"));
    }

    #[test]
    fn safe_join_resolves_nested_file() {
        let dir = tmp();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/page.html"), b"<h1>hi</h1>").unwrap();
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let path = safe_join(&root, "sub/page.html").unwrap();
        assert!(path.ends_with("page.html"));
    }

    #[test]
    fn safe_join_not_found_for_missing_file() {
        let dir = tmp();
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let err = safe_join(&root, "missing.txt").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
        assert_eq!(err.http_status(), 404);
    }

    #[test]
    fn safe_join_forbidden_for_traversal_to_existing_file() {
        // Traversal escapes root to an existing file; must be rejected (403)
        let parent = tmp();
        let root_path = parent.path().join("assets");
        fs::create_dir(&root_path).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();

        let root = CanonicalRoot::new(root_path.as_path()).unwrap();
        let err = safe_join(&root, "../secret.txt").unwrap_err();
        assert!(matches!(err, ResolveError::Forbidden(_)));
        assert_eq!(err.http_status(), 403);
    }

    #[test]
    fn safe_join_not_found_for_traversal_to_missing_file() {
        // Traversal to non-existent target is indistinguishable from in-root miss without
        // an exists() check (which would be TOCTOU).
        let dir = tmp();
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let err = safe_join(&root, "../no_such_file.txt").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[test]
    fn safe_join_rejects_buried_traversal() {
        let parent = tmp();
        let root_path = parent.path().join("assets");
        fs::create_dir(&root_path).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();

        let root = CanonicalRoot::new(root_path.as_path()).unwrap();
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
        std::fs::create_dir(&root).unwrap();

        // Create a real file outside root
        let secret = parent.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();

        // Create symlink inside root pointing outside
        let link = root.join("escape");
        symlink(&secret, &link).unwrap();

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
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let asset = resolve_asset(&root, "app.js").unwrap();
        assert_eq!(asset.mime, "application/javascript");
        assert_eq!(asset.bytes, b"console.log('hi')");
    }

    #[test]
    fn resolve_asset_404_propagates() {
        let dir = tmp();
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let err = resolve_asset(&root, "nope.html").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
        assert_eq!(err.http_status(), 404);
    }

    #[test]
    fn resolve_asset_empty_file_is_ok() {
        let dir = tmp();
        fs::write(dir.path().join("empty.js"), b"").unwrap();
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let asset = resolve_asset(&root, "empty.js").unwrap();
        assert!(asset.bytes.is_empty());
        assert_eq!(asset.mime, "application/javascript");
    }

    #[test]
    fn resolve_asset_returns_404_for_directory() {
        let dir = tmp();
        fs::create_dir(dir.path().join("sub")).unwrap();

        let root = CanonicalRoot::new(dir.path()).unwrap();
        let err = resolve_asset(&root, "sub").unwrap_err();

        assert!(matches!(err, ResolveError::NotFound(_)));
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

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn sanitize_name_never_panics(s in ".*") {
            // never panics on any input
            let _ = sanitize_name(&s);
        }

        #[test]
        fn sanitize_name_output_is_valid_utf8(s in ".*") {
            let result = sanitize_name(&s);
            prop_assert!(result.chars().count() <= 64);
            assert!(!result.is_empty());
        }

        #[test]
        fn sanitize_name_no_forbidden_chars(s in ".*") {
            let result = sanitize_name(&s);
            for c in result.chars() {
                prop_assert!(!matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'));
            }
        }

        #[test]
        fn extract_rel_path_never_produces_absolute(url in "app://app/.*") {
            if let Ok(rel) = extract_rel_path(&url) {
                prop_assert!(!rel.starts_with('/'),
                    "rel path must be relative, got: {:?}", rel);
                prop_assert!(!Path::new(&rel).is_absolute());
            }
        }

        #[test]
        fn extract_rel_path_no_traversal_components(url in "app://app/.*") {
            if let Ok(rel) = extract_rel_path(&url) {
                // Note: safe_join handles the actual traversal prevention,
                // but we can verify decoded output doesn't contain obvious markers
                for component in Path::new(&rel).components() {
                    prop_assert!(
                        !matches!(component, std::path::Component::ParentDir),
                        "decoded path contains .. component: {:?}", rel
                    );
                }
            }
        }
    }
}
