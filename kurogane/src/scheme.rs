//! app:// scheme support for local assets.

use cef::*;
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use mime_guess::MimeGuess;

use crate::debug;

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

        // Synchronous open
        fn open(
            &self,
            request: Option<&mut Request>,
            handle_request: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            let request = request.unwrap();
            let url: CefString = (&request.url()).into();
            let url = url.to_string();

            // Strip scheme and handle trailing slashes
            let path = url
                .strip_prefix("app://app/")
                .unwrap_or("index.html")
                .trim_start_matches('/')
                .trim_end_matches('/');

            // Handle empty path (app:// or app:///)
            let path = if path.is_empty() { "index.html" } else { path };

            debug!("Resolved path: {}", path);

            // Resolve relative to CWD (set by resolver)
            let root = crate::runtime::Runtime::asset_root();

            let result = safe_join(&root, path)
                .and_then(|p| std::fs::read(&p).ok().map(|b| (p, b)));

            match result {
                Some((full_path, bytes)) => {
                    *self.data.lock().unwrap() = bytes;
                    *self.offset.lock().unwrap() = 0;
                    *self.mime.lock().unwrap() = mime_from_path(&full_path);
                    self.status.store(200, Ordering::Release);
                }
                None => {
                    eprintln!("[app://] 404 {}", path);
                    self.status.store(404, Ordering::Release);
                    *self.data.lock().unwrap() = b"404 Not Found".to_vec();
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

            let mut offset = self.offset.lock().unwrap();
            let data = self.data.lock().unwrap();

            let remaining = &data[*offset..];
            let read = remaining.len().min(bytes_to_read as usize);

            if read > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(remaining.as_ptr(), data_out, read);
                }
                *offset += read;
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

fn safe_join(root: &Path, request: &str) -> Option<PathBuf> {
    let canonical = root.join(request).canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_types() {
        assert_eq!(mime_from_path("index.html".as_ref()), "text/html");
        assert_eq!(mime_from_path("app.js".as_ref()), "application/javascript");
        assert_eq!(mime_from_path("style.css".as_ref()), "text/css");
        assert_eq!(mime_from_path("data.json".as_ref()), "application/json");
    }

    #[test]
    fn detects_modern_edge_cases() {
        assert_eq!(mime_from_path("module.mjs".as_ref()), "application/javascript");
        assert_eq!(mime_from_path("config.cjs".as_ref()), "application/javascript"); // mime_guess create misses
    }

    #[test]
    fn handles_unknown() {
        assert_eq!(
            mime_from_path("file.unknownext".as_ref()),
            "application/octet-stream"
        );
    }

    #[test]
    fn handles_double_extensions() {
        assert_eq!(
            mime_from_path("archive.tar.gz".as_ref()),
            "application/gzip"
        );
    }
}
