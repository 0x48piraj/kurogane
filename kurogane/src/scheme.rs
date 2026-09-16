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
//! - No absolute, drive-relative or rooted path injection
//! - No filesystem details leaked to clients: every request that does not
//!   name a file inside the root answers the same 404, whether the target is
//!   missing, outside the root or malformed
//!
//! Design notes:
//! - The asset root is the only allowed filesystem boundary
//! - Each decoded path segment must be one name by the filesystem
//!   capability's rules (no `.`/`..`, separator, NUL, `:`, device name or
//!   8.3 short-name shape); nothing outside the root is ever consulted
//! - Links inside the root that stay inside it keep working
//! - Focused on safe, predictable asset access within the runtime

use cef::*;
use cef::sys::cef_scheme_options_t::{
    CEF_SCHEME_OPTION_STANDARD, CEF_SCHEME_OPTION_SECURE, CEF_SCHEME_OPTION_CORS_ENABLED,
    CEF_SCHEME_OPTION_FETCH_ENABLED,
};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use percent_encoding::percent_decode_str;
use mime_guess::MimeGuess;
use url::Url;
use crate::fs::CanonicalRoot;

use crate::debug;

/// Errors returned when resolving an app:// request.
/// Each variant maps to an HTTP status code.
#[derive(Debug)]
pub enum ResolveError {
    /// The URL could not be parsed, or its scheme is not app
    InvalidUrl,
    /// The resolved path escapes the asset root (a link inside it pointing
    /// out). Answers 404, like a missing file, so the client cannot tell.
    Forbidden(PathBuf),
    /// The path names no file inside the root: missing, or not a valid
    /// sequence of names
    NotFound(PathBuf),
    /// An I/O error occurred after validation
    Io(std::io::Error),
}

impl ResolveError {
    /// The status sent to the client. An escape and a miss are both 404: a
    /// distinct answer would tell a page what exists outside the root.
    pub fn http_status(&self) -> i32 {
        match self {
            Self::InvalidUrl => 400,
            Self::Forbidden(_) | Self::NotFound(_) => 404,
            Self::Io(_) => 500,
        }
    }

    pub fn http_repr(&self) -> &'static [u8] {
        match self {
            Self::InvalidUrl => b"400 Bad Request",
            Self::Forbidden(_) | Self::NotFound(_) => b"404 Not Found",
            Self::Io(_) => b"500 Internal Server Error",
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

/// Generates a [`ResourceHandler`] for requests on a custom scheme.
///
/// Implement this trait and pass the instance to
/// [`App::register_scheme`](crate::App::register_scheme) to expose a custom
/// scheme to the frontend. The trait method mirrors
/// [`SchemeHandlerFactory::create`](cef::SchemeHandlerFactory) minus the
/// scheme name which is fixed per registration.
///
/// The handler is invoked on the browser-process IO thread and must serve the
/// response asynchronously or synchronously via a [`ResourceHandler`].
///
/// [`AppResourceHandler`] covers the common static-bytes case.
pub trait SchemeHandler: Send + Sync {
    /// Create a resource handler for `request`, or `None` to fail the request.
    fn create(
        &self,
        browser: Option<&mut Browser>,
        frame: Option<&mut Frame>,
        request: Option<&mut Request>,
    ) -> Option<ResourceHandler>;
}

/// A user-registered scheme and its handler.
///
/// Created via [`App::register_scheme`](crate::App::register_scheme).
#[derive(Clone)]
pub struct CustomScheme {
    /// Scheme name, such as `data`. The `app` scheme is reserved for the
    /// built-in asset scheme and cannot be registered.
    pub name: String,
    /// Handler invoked for every request on this scheme, regardless of host.
    pub handler: Arc<dyn SchemeHandler>,
}

/// Returns the CEF scheme option flags applied to registered custom schemes.
pub(crate) fn custom_scheme_flags() -> i32 {
    CEF_SCHEME_OPTION_STANDARD as i32
        | CEF_SCHEME_OPTION_SECURE as i32
        | CEF_SCHEME_OPTION_CORS_ENABLED as i32
        | CEF_SCHEME_OPTION_FETCH_ENABLED as i32
}

/// Validates a custom scheme name against CEF's naming rules.
///
/// Names start with a letter and may contain letters, digits, `+`, `-` and
/// `.`. The `app` scheme is reserved for the built-in asset scheme.
pub(crate) fn validate_scheme_name(name: &str) -> Result<(), &'static str> {
    if name == "app" {
        return Err("'app' is reserved for the built-in asset scheme");
    }

    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("scheme name is empty");
    };
    if !first.is_ascii_alphabetic() {
        return Err("scheme name must start with a letter");
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return Err("scheme name may only contain letters, digits, '+', '-' or '.'");
    }
    Ok(())
}

//
// SchemeHandlerFactory
//

wrap_scheme_handler_factory! {
    pub struct AppSchemeHandlerFactory {
        root: CanonicalRoot,
    }

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

            // Resolve relative to CWD
            let root = self.root.clone();

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

                    eprintln!("[kurogane] status={status} url=\"{raw_url}\" reason={e}");

                    (
                        Arc::<[u8]>::from(e.http_repr()),
                        "text/plain".to_string(),
                        status,
                    )
                }
            };

            Some(AppResourceHandler::new(
                data,
                Arc::new(AtomicUsize::new(0)),
                mime,
                status,
            ))
        }
    }
}

//
// CustomSchemeHandlerFactory
//
// CEF factory implementation that forwards requests to a user-supplied
// SchemeHandler.

wrap_scheme_handler_factory! {
    pub struct CustomSchemeHandlerFactory {
        handler: Arc<dyn SchemeHandler>,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            self.handler.create(browser, frame, request)
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
        status: i32,
    }

    impl ResourceHandler {

        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            debug!("[app://] open: {} bytes to serve", self.data.len());

            self.offset.store(0, Ordering::Release);

            if let Some(hr) = handle_request {
                *hr = 1;
            }

            1
        }

        #[expect(
            clippy::not_unsafe_ptr_arg_deref,
            reason = "signature fixed by cef-rs ImplResourceHandler; CEF owns the buffer contract"
        )]
        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: i32,
            bytes_read: Option<&mut i32>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> i32 {
            // Runs inside a CEF FFI callback; refuse rather than panic
            let Some(br) = bytes_read else {
                debug!("[app://] read: refused (no bytes_read out-parameter)");
                return 0;
            };

            // FFI safety guard (invalid pointer or non-positive length)
            if bytes_to_read <= 0 || data_out.is_null() {
                debug!("[app://] read: refused (bytes_to_read={bytes_to_read}, null={})",
                    data_out.is_null());

                *br = 0;
                return 0;
            }

            let offset = self.offset.load(Ordering::Acquire);
            let data = self.data.as_ref();

            // Runs inside a CEF FFI callback, where a slice panic could abort the process
            // Refuse the read rather than panic
            if offset > data.len() {
                debug!(
                    "[app://] read: refused (offset {offset} past {} bytes)",
                    data.len()
                );

                *br = 0;
                return 0;
            }

            let remaining = &data[offset..];
            let read = remaining.len().min(bytes_to_read as usize);

            if read > 0 {
                // SAFETY: CEF guarantees `data_out` is a writable buffer of at
                // least `bytes_to_read` bytes that does not alias `self.data`.
                // It is non-null and `bytes_to_read > 0` (checked above), and
                // `read <= bytes_to_read`, so the copy stays in bounds. The raw
                // copy never forms a reference to the possibly uninitialized
                // buffer.
                unsafe {
                    std::ptr::copy_nonoverlapping(remaining.as_ptr(), data_out, read);
                }

                self.offset.fetch_add(read, Ordering::Release);
            }

            *br = read as i32;

            debug!(
                "[app://] read: {read} bytes at offset {offset} of {} ({} requested)",
                data.len(),
                bytes_to_read
            );

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

            let data_len = self.data.len() as i64;

            response.set_status(self.status);
            response.set_mime_type(Some(&CefString::from(self.mime.as_str())));

            if let Some(len) = response_length {
                *len = data_len;
            }

            debug!(
                "[app://] response_headers: status={} mime={} length={data_len}",
                self.status, self.mime
            );
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
    let parsed = Url::parse(raw_url).map_err(|_| ResolveError::InvalidUrl)?;

    // Impose scheme rule
    if parsed.scheme() != "app" {
        return Err(ResolveError::InvalidUrl);
    }

    // Impose host rule
    if parsed.host_str() != Some("app") {
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
///
/// Each `/`-separated segment must be one valid name, so the request can
/// neither climb (`..`) nor replace the root (`C:x`, `\x`, `\\server`) when
/// joined; nothing outside the root is consulted to answer it. A link inside
/// the root is followed only while it stays inside. Every failure is 404.
pub fn safe_join(root: &CanonicalRoot, request: &str) -> Result<PathBuf, ResolveError> {
    let mut joined = root.as_path().to_path_buf();
    for segment in request.split('/') {
        if crate::capability::path::Name::parse(std::ffi::OsStr::new(segment)).is_err() {
            return Err(ResolveError::NotFound(PathBuf::from(request)));
        }
        joined.push(segment);
    }

    // Any failure to resolve is a miss to the client
    let canonical = joined
        .canonicalize()
        .map_err(|_| ResolveError::NotFound(joined))?;

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

    let bytes = std::fs::read(&path).map_err(ResolveError::Io)?;

    let mime = mime_from_path(&path);

    Ok(ResolvedAsset { path, bytes, mime })
}

/// Validates the asset root by resolving its 'index.html' entrypoint.
pub(crate) fn validate_asset_root(root: &CanonicalRoot) -> Result<(), ResolveError> {
    resolve_asset(root, "index.html")?;
    Ok(())
}

/// Returns the MIME type for a given path based on its file extension.
/// Unknown extensions fall back to 'application/octet-stream'.
fn mime_from_path(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        // App-specific overrides
        Some("js" | "mjs" | "cjs") => "application/javascript".to_string(),
        _ => MimeGuess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_owned(),
    }
}

/// Builds a [`ResourceHandler`] serving `data` once with the given MIME type
/// and status code.
///
/// Convenience wrapper around [`AppResourceHandler`] for custom scheme
/// handlers that answer with static bytes.
pub fn resource_handler_from_bytes(data: Vec<u8>, mime: &str, status: i32) -> ResourceHandler {
    AppResourceHandler::new(
        Arc::<[u8]>::from(data),
        Arc::new(AtomicUsize::new(0)),
        mime.to_string(),
        status,
    )
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
    fn rel_path_rejects_opaque_app_url_without_authority() {
        let err = extract_rel_path("app:index.html").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidUrl));
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

    #[test]
    fn safe_join_blocked_percent_encoded_traversal() {
        let parent = tmp();
        let root_path = parent.path().join("assets");
        fs::create_dir(&root_path).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();

        let root = CanonicalRoot::new(&root_path).unwrap();
        let rel = extract_rel_path("app://app/%2e%2e/secret.txt").unwrap();
        let err = safe_join(&root, &rel).unwrap_err();

        assert!(matches!(
            err,
            ResolveError::Forbidden(_) | ResolveError::NotFound(_)
        ));
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
    fn safe_join_never_consults_a_traversal_target() {
        // An existing and a missing target outside the root answer alike
        let parent = tmp();
        let root_path = parent.path().join("assets");
        fs::create_dir(&root_path).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();

        let root = CanonicalRoot::new(root_path.as_path()).unwrap();
        for request in ["../secret.txt", "../no_such_file.txt"] {
            let err = safe_join(&root, request).unwrap_err();
            assert!(matches!(err, ResolveError::NotFound(_)), "{request}");
            assert_eq!(err.http_status(), 404);
        }
    }

    #[test]
    fn every_rejected_request_is_a_uniform_404() {
        let parent = tmp();
        let root_path = parent.path().join("assets");
        fs::create_dir(&root_path).unwrap();
        fs::write(parent.path().join("secret.txt"), b"secret").unwrap();
        fs::write(root_path.join("index.html"), b"ok").unwrap();
        let root = CanonicalRoot::new(&root_path).unwrap();

        let answer =
            |url: &str| match extract_rel_path(url).and_then(|rel| resolve_asset(&root, &rel)) {
                Ok(_) => 200,
                Err(e) => e.http_status(),
            };
        assert_eq!(answer("app://app/index.html"), 200);
        for url in [
            "app://app/missing.html",
            "app://app/%2e%2e/secret.txt",
            "app://app/a%2F..%2F..%2Fsecret.txt",
            "app://app/..%5Csecret.txt",
            "app://app/%5Csecret.txt",
            "app://app/%5C%5Clocalhost%5CC$%5Csecret.txt",
            "app://app/C:secret.txt",
            "app://app/C:%5Csecret.txt",
            "app://app/sub//index.html",
            "app://app/index.html%00.png",
        ] {
            assert_eq!(answer(url), 404, "{url}");
        }
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
        let root = CanonicalRoot::new(dir.path()).unwrap();
        let err = safe_join(&root, "/etc/passwd").unwrap_err();
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
        let root_path = parent.path().join("assets");
        std::fs::create_dir(&root_path).unwrap();

        // Create a real file outside root
        let secret = parent.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();

        // Create symlink inside root pointing outside
        let link = root_path.join("escape");
        symlink(&secret, &link).unwrap();

        let root = CanonicalRoot::new(&root_path).unwrap();
        let err = safe_join(&root, "escape").unwrap_err();

        assert!(matches!(err, ResolveError::Forbidden(_)));
        assert_eq!(err.http_status(), 404, "an escape answers like a miss");
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
        assert_eq!(ResolveError::Forbidden(PathBuf::new()).http_status(), 404);
        assert_eq!(ResolveError::NotFound(PathBuf::new()).http_status(), 404);
        assert_eq!(
            ResolveError::Forbidden(PathBuf::new()).http_repr(),
            ResolveError::NotFound(PathBuf::new()).http_repr()
        );
        let io_err = std::io::Error::other("disk on fire");
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

    // Custom scheme name validation

    #[test]
    fn scheme_name_accepts_valid_names() {
        for name in ["data", "myapp", "k", "app-helper", "x.y", "a+b", "Kuro9"] {
            assert!(validate_scheme_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn scheme_name_reserves_builtin_app() {
        assert!(validate_scheme_name("app").is_err());
    }

    #[test]
    fn scheme_name_rejects_empty_and_bad_start() {
        assert!(validate_scheme_name("").is_err());
        assert!(validate_scheme_name("1data").is_err());
        assert!(validate_scheme_name("-data").is_err());
        assert!(validate_scheme_name(".data").is_err());
    }

    #[test]
    fn scheme_name_rejects_invalid_characters() {
        assert!(validate_scheme_name("da ta").is_err());
        assert!(validate_scheme_name("da/ta").is_err());
        assert!(validate_scheme_name("da*ta").is_err());
        assert!(validate_scheme_name("app:foo").is_err());
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn query_and_fragment_do_not_appear_in_output(
            path in "[a-zA-Z0-9/_\\.-]{0,64}",
            query in ".*",
            fragment in ".*",
        ) {
            let url = format!(
                "app://app/{path}?{query}#{fragment}"
            );

            if let Ok(rel) = extract_rel_path(&url) {
                prop_assert!(!rel.contains('?'));
                prop_assert!(!rel.contains('#'));
            }
        }

        #[test]
        fn safe_join_never_escapes_root(rel in ".*") {
            let dir = tempfile::tempdir().unwrap();
            let root = CanonicalRoot::new(dir.path()).unwrap();

            if let Ok(path) = safe_join(&root, &rel) {
                prop_assert!(path.starts_with(root.as_path()));
            }
        }
    }
}
