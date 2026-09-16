//! The `fs.*` IPC command surface.
//!
//! Each [`FsCommand`] has an async handler backed by a shared [`Filesystem`].
//! Authorization uses `IpcContext.origin` and [`Filesystem::authorize`]; the
//! origin is derived from the frame URL and never accepted from the payload.
//!
//! Operations run on a bounded `kurogane-fs` worker thread, spawned on first
//! use, so file I/O does not block the CEF UI thread. Requests are processed
//! in submission order; a full queue is rejected with "filesystem is busy".
//!
//! A request is admitted before its payload is copied or queued: an origin
//! whose grants cannot perform the command gets [`ErrorCode::Capability`],
//! a `write_file` over the transfer limit gets [`ErrorCode::TooLarge`] from
//! its header alone, and an oversized path or request gets
//! [`ErrorCode::Buffer`]. Admitted requests hold a share of a queued-bytes
//! budget, and one origin may have at most [`ORIGIN_IN_FLIGHT`] operations
//! queued or running, so no origin can occupy the worker for the others.
//!
//! File contents use the binary channel; all other data uses the string
//! channel. `fs.read_file` takes a UTF-8 path and returns its contents.
//! `fs.write_file` takes `[path length: u32 LE][path: UTF-8][contents]` and
//! returns an empty response. Other operations use `{"path"}` or
//! `{"src", "dst"}` and return their operation-specific JSON result or
//! `null`.
//!
//! [`FsError`] maps to [`ErrorCode::Capability`] (-5),
//! [`ErrorCode::PathDenied`] (-6), [`ErrorCode::PathInvalid`] (-7) and
//! [`ErrorCode::Handler`] (0). Malformed JSON or binary frames return
//! [`ErrorCode::Buffer`] (-2).

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::acl::Origin;
use crate::capability::authorized::{AuthorizedFs, Filesystem};
use crate::capability::error::FsError;
use crate::capability::policy::FsCommand;
use crate::capability::safe::EntryKind;
use crate::debug;
use crate::ipc::{AsyncHandler, BinaryResponder, ErrorCode, IpcError};

/// How many operations may wait for the worker before new ones are refused.
const QUEUE_DEPTH: usize = 64;

/// Operations one origin may have queued or running at once.
const ORIGIN_IN_FLIGHT: usize = 16;

/// The longest path a request may carry, in bytes.
const MAX_PATH_BYTES: usize = 32 * 1024;

/// The largest request other than `write_file`, in bytes.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// One handler per `fs.*` command, all feeding one worker.
pub(crate) fn handlers(filesystem: Filesystem) -> impl Iterator<Item = (FsCommand, AsyncHandler)> {
    let budget = Arc::new(Budget::for_limit(filesystem.max_file_size()));
    let worker = Arc::new(Worker {
        filesystem: Arc::new(filesystem),
        queue: OnceLock::new(),
        budget,
    });
    FsCommand::ALL
        .into_iter()
        .map(move |command| (command, handler(Arc::clone(&worker), command)))
}

fn handler(worker: Arc<Worker>, command: FsCommand) -> AsyncHandler {
    Box::new(move |data, responder, ctx| {
        // Refused requests are answered here, before anything is copied
        let permit = match worker.admit(command, data, &ctx.origin) {
            Ok(permit) => permit,
            Err(refusal) => return responder.resolve(Err(refusal)),
        };
        worker.submit(Job {
            command,
            data: data.to_vec(),
            origin: ctx.origin,
            responder,
            _permit: permit,
        });
    })
}

/// One queued operation. Plain data, so a refused job still reaches its
/// responder; its permit returns its share of the budget when dropped.
struct Job {
    command: FsCommand,
    data: Vec<u8>,
    origin: Origin,
    responder: BinaryResponder,
    _permit: Permit,
}

impl Job {
    fn run(self, filesystem: &Filesystem) {
        // Cancelled by the page or by navigation while queued: nobody waits
        if self.responder.is_cancelled() {
            return;
        }
        let result = match filesystem.authorize(&self.origin) {
            Some(auth) => run(&auth, self.command, &self.data),
            None => Err(FsError::CapabilityDenied.into()),
        };
        self.responder.resolve(result);
    }
}

struct Worker {
    filesystem: Arc<Filesystem>,
    /// `None` when the thread could not be spawned; jobs then run inline
    queue: OnceLock<Option<SyncSender<Job>>>,
    budget: Arc<Budget>,
}

impl Worker {
    /// Decides from the origin's grants and the request's size alone whether
    /// the request may be queued.
    fn admit(&self, command: FsCommand, data: &[u8], origin: &Origin) -> Result<Permit, IpcError> {
        if !command.admits(self.filesystem.access_of(origin)) {
            return Err(FsError::CapabilityDenied.into());
        }
        match command {
            FsCommand::WriteFile => {
                let (path, contents) = write_frame(data)?;
                if path.as_os_str().len() > MAX_PATH_BYTES {
                    return Err(IpcError::with_code(
                        "the path is too long",
                        ErrorCode::Buffer,
                    ));
                }
                let limit = self.filesystem.max_file_size();
                if u64::try_from(contents.len()).map_or(true, |len| len > limit) {
                    return Err(FsError::TooLarge { limit }.into());
                }
            }
            FsCommand::ReadFile if data.len() > MAX_PATH_BYTES => {
                return Err(IpcError::with_code(
                    "the path is too long",
                    ErrorCode::Buffer,
                ));
            }
            _ if data.len() > MAX_REQUEST_BYTES => {
                return Err(IpcError::with_code(
                    "the request is too large",
                    ErrorCode::Buffer,
                ));
            }
            _ => {}
        }
        self.budget
            .acquire(origin, data.len())
            .ok_or_else(|| IpcError::new("filesystem is busy"))
    }

    fn submit(&self, job: Job) {
        let Some(queue) = self.queue.get_or_init(|| self.spawn()) else {
            job.run(&self.filesystem);
            return;
        };
        let (job, reason) = match queue.try_send(job) {
            Ok(()) => return,
            Err(TrySendError::Full(job)) => (job, "filesystem is busy"),
            Err(TrySendError::Disconnected(job)) => (job, "filesystem worker stopped"),
        };
        job.responder.resolve(Err(IpcError::new(reason)));
    }

    fn spawn(&self) -> Option<SyncSender<Job>> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(QUEUE_DEPTH);
        let filesystem = Arc::clone(&self.filesystem);
        let spawned = std::thread::Builder::new()
            .name("kurogane-fs".to_owned())
            .spawn(move || {
                for job in jobs {
                    // A panicking job drops its responder, which rejects the
                    // request; the worker keeps serving the others
                    let _ = catch_unwind(AssertUnwindSafe(|| job.run(&filesystem)));
                }
            });
        match spawned {
            Ok(_) => Some(sender),
            Err(e) => {
                debug!("[fs] cannot spawn the worker thread ({e}); running operations inline");
                None
            }
        }
    }
}

/// Bounds what queued requests may hold: their payload bytes in total, and
/// how many one origin may have queued or running.
struct Budget {
    capacity: usize,
    state: Mutex<BudgetState>,
}

#[derive(Default)]
struct BudgetState {
    used: usize,
    in_flight: HashMap<Origin, usize>,
}

impl Budget {
    /// Room for two maximal transfers plus small requests.
    fn for_limit(max_file_size: u64) -> Budget {
        let capacity = max_file_size
            .saturating_mul(2)
            .saturating_add(1 << 20)
            .min(usize::MAX as u64) as usize;
        Budget {
            capacity,
            state: Mutex::new(BudgetState::default()),
        }
    }

    fn acquire(self: &Arc<Self>, origin: &Origin, bytes: usize) -> Option<Permit> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let used = state
            .used
            .checked_add(bytes)
            .filter(|&used| used <= self.capacity)?;
        let count = state.in_flight.entry(origin.clone()).or_default();
        if *count >= ORIGIN_IN_FLIGHT {
            return None;
        }
        *count += 1;
        state.used = used;
        Some(Permit {
            budget: Arc::clone(self),
            origin: origin.clone(),
            bytes,
        })
    }
}

/// A request's share of the [`Budget`], returned when the request ends.
struct Permit {
    budget: Arc<Budget>,
    origin: Origin,
    bytes: usize,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self
            .budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.used -= self.bytes;
        if let Some(count) = state.in_flight.get_mut(&self.origin) {
            *count -= 1;
            if *count == 0 {
                state.in_flight.remove(&self.origin);
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArg {
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TwoPaths {
    src: PathBuf,
    dst: PathBuf,
}

fn run(auth: &AuthorizedFs<'_>, command: FsCommand, data: &[u8]) -> Result<Vec<u8>, IpcError> {
    let response = match command {
        // File contents are raw bytes both ways
        FsCommand::ReadFile => return Ok(auth.read_file(&binary_path(data)?)?),
        FsCommand::WriteFile => {
            let (path, contents) = write_frame(data)?;
            auth.write_file(&path, contents)?;
            return Ok(Vec::new());
        }
        FsCommand::ReadDir => {
            let PathArg { path } = parse(data)?;
            let entries: Vec<Value> = auth
                .list_dir(&path)?
                .iter()
                .map(|entry| json!({ "name": entry.name().to_string_lossy(), "kind": kind_name(entry.kind()) }))
                .collect();
            json!({ "entries": entries })
        }
        FsCommand::Size => json!({ "size": auth.size(&parse::<PathArg>(data)?.path)? }),
        FsCommand::Exists => json!({ "exists": auth.exists(&parse::<PathArg>(data)?.path)? }),
        FsCommand::CreateDir => {
            auth.create_dir(&parse::<PathArg>(data)?.path)?;
            Value::Null
        }
        FsCommand::RemoveFile => {
            auth.remove_file(&parse::<PathArg>(data)?.path)?;
            Value::Null
        }
        FsCommand::RemoveDir => {
            auth.remove_dir(&parse::<PathArg>(data)?.path)?;
            Value::Null
        }
        FsCommand::CopyFile => {
            let TwoPaths { src, dst } = parse(data)?;
            json!({ "bytes": auth.copy_file(&src, &dst)? })
        }
        FsCommand::RenameFile => {
            let TwoPaths { src, dst } = parse(data)?;
            auth.rename_file(&src, &dst)?;
            Value::Null
        }
    };
    serde_json::to_vec(&response).map_err(IpcError::from)
}

/// The path of a binary request: its UTF-8 bytes.
fn binary_path(bytes: &[u8]) -> Result<PathBuf, IpcError> {
    std::str::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| IpcError::with_code("the path is not UTF-8", ErrorCode::Buffer))
}

/// Splits a `write_file` frame: `[path length: u32 LE][path: UTF-8][contents]`.
fn write_frame(frame: &[u8]) -> Result<(PathBuf, &[u8]), IpcError> {
    let malformed = || IpcError::with_code("malformed write_file frame", ErrorCode::Buffer);
    let (length, rest) = frame.split_first_chunk::<4>().ok_or_else(malformed)?;
    let length = usize::try_from(u32::from_le_bytes(*length)).map_err(|_| malformed())?;
    if length > rest.len() {
        return Err(malformed());
    }
    let (path, contents) = rest.split_at(length);
    Ok((binary_path(path)?, contents))
}

fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Other => "other",
    }
}

fn parse<T: DeserializeOwned>(data: &[u8]) -> Result<T, IpcError> {
    serde_json::from_slice(data).map_err(IpcError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `write_file` frame, built the way the bridge builds it.
    fn frame(path: &str, contents: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(path.len()).unwrap().to_le_bytes().to_vec();
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(contents);
        out
    }

    #[test]
    fn binary_frames_round_trip() {
        let text = frame("dir/ä.txt", "hé".as_bytes());
        let (path, contents) = write_frame(&text).unwrap();
        assert_eq!(path, PathBuf::from("dir/ä.txt"));
        assert_eq!(contents, "hé".as_bytes());

        let empty = frame("empty.txt", b"");
        let (path, contents) = write_frame(&empty).unwrap();
        assert_eq!(path, PathBuf::from("empty.txt"));
        assert!(contents.is_empty());

        assert_eq!(binary_path(b"a/b.txt").unwrap(), PathBuf::from("a/b.txt"));
    }

    #[test]
    fn malformed_binary_frames_are_buffer_errors() {
        let too_long = {
            let mut f = frame("abc", b"x");
            f[0] = 200;
            f
        };
        let mut not_utf8 = 2u32.to_le_bytes().to_vec();
        not_utf8.extend_from_slice(b"\xff\xfe");
        for bad in [&b""[..], &b"\x03\x00\x00"[..], &too_long[..], &not_utf8[..]] {
            assert_eq!(
                write_frame(bad).unwrap_err().code(),
                ErrorCode::Buffer,
                "{bad:?}"
            );
        }
        assert_eq!(binary_path(b"\xff").unwrap_err().code(), ErrorCode::Buffer);
    }

    #[test]
    fn the_budget_bounds_bytes_and_per_origin_requests() {
        let budget = Arc::new(Budget::for_limit(0));
        let a = Origin::parse("app://a").unwrap();
        let b = Origin::parse("app://b").unwrap();
        assert!(
            budget.acquire(&a, budget.capacity + 1).is_none(),
            "over the byte budget"
        );
        let held: Vec<_> = (0..ORIGIN_IN_FLIGHT)
            .map(|_| budget.acquire(&a, 1).unwrap())
            .collect();
        assert!(budget.acquire(&a, 1).is_none(), "over the per-origin cap");
        assert!(
            budget.acquire(&b, 1).is_some(),
            "another origin is unaffected"
        );
        drop(held);
        assert!(
            budget.acquire(&a, 1).is_some(),
            "permits return their share"
        );
    }

    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    mod with_backend {
        use super::super::*;
        use super::frame;
        use std::time::Duration;

        use crate::acl::Origin;
        use crate::capability::policy::FsAccess;
        use crate::ipc::{BinaryResponder, FrameId, IpcContext};

        fn notes(access: FsAccess) -> (tempfile::TempDir, PathBuf, Vec<(FsCommand, AsyncHandler)>) {
            let tmp = tempfile::tempdir().unwrap();
            let notes = tmp.path().join("notes");
            std::fs::create_dir_all(notes.join("secrets")).unwrap();
            std::fs::write(notes.join("secrets/secret.txt"), b"top secret").unwrap();
            std::fs::write(notes.join("note.txt"), b"hello").unwrap();
            let mut builder = Filesystem::builder();
            let scope = builder.scope("notes", |s| {
                s.allow_directory_recursive(&notes);
                s.deny_path(notes.join("secrets"));
            });
            builder.grant(Origin::parse("app://notes").unwrap(), scope, access);
            (tmp, notes, handlers(builder.build().unwrap()).collect())
        }

        fn ctx(origin: &str) -> IpcContext {
            IpcContext {
                browser_id: None,
                frame: FrameId::new("test-frame"),
                origin: Origin::from_url(origin),
                url_origin: Origin::from_url(origin),
            }
        }

        /// Sends `request` to the handler for `command`; returns the answer
        /// and the name of the thread that produced it.
        fn send_on(
            handlers: &[(FsCommand, AsyncHandler)],
            command: FsCommand,
            origin: &str,
            request: &[u8],
        ) -> (Option<String>, Result<Vec<u8>, IpcError>) {
            let (_, handler) = handlers.iter().find(|(c, _)| *c == command).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let responder = BinaryResponder::new(Box::new(move |r| {
                let _ = tx.send((std::thread::current().name().map(str::to_owned), r));
            }));
            handler(request, responder, ctx(origin));
            rx.recv_timeout(Duration::from_secs(10))
                .expect("handler resolved")
        }

        fn send(
            handlers: &[(FsCommand, AsyncHandler)],
            command: FsCommand,
            origin: &str,
            request: &[u8],
        ) -> Result<Vec<u8>, IpcError> {
            send_on(handlers, command, origin, request).1
        }

        fn send_json(
            handlers: &[(FsCommand, AsyncHandler)],
            command: FsCommand,
            request: Value,
        ) -> Result<Value, IpcError> {
            let bytes = send(
                handlers,
                command,
                "app://notes",
                &serde_json::to_vec(&request).unwrap(),
            )?;
            Ok(serde_json::from_slice(&bytes).unwrap())
        }

        #[test]
        fn every_command_gets_a_handler() {
            let (_tmp, _notes, handlers) = notes(FsAccess::ALL);
            assert_eq!(handlers.len(), FsCommand::ALL.len());
        }

        #[test]
        fn file_contents_travel_as_raw_bytes() {
            let (_tmp, notes, handlers) = notes(FsAccess::ALL);
            let bytes: Vec<u8> = (0..=255).collect();
            let written = send(
                &handlers,
                FsCommand::WriteFile,
                "app://notes",
                &frame("fresh.bin", &bytes),
            )
            .unwrap();
            assert!(written.is_empty());
            assert_eq!(std::fs::read(notes.join("fresh.bin")).unwrap(), bytes);
            let read = send(&handlers, FsCommand::ReadFile, "app://notes", b"fresh.bin").unwrap();
            assert_eq!(read, bytes);
        }

        #[test]
        fn operations_run_in_order_on_the_worker_thread() {
            let (_tmp, _notes, handlers) = notes(FsAccess::ALL);
            let find = |command| &handlers.iter().find(|(c, _)| *c == command).unwrap().1;
            let (tx, rx) = std::sync::mpsc::channel();
            let requests = [
                (FsCommand::WriteFile, frame("order.txt", b"first")),
                (FsCommand::ReadFile, b"order.txt".to_vec()),
            ];
            for (command, request) in requests {
                let tx = tx.clone();
                let responder = BinaryResponder::new(Box::new(move |r| {
                    let _ = tx.send((std::thread::current().name().map(str::to_owned), r));
                }));
                find(command)(&request, responder, ctx("app://notes"));
            }
            let (thread, written) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(written.is_ok());
            assert_eq!(thread.as_deref(), Some("kurogane-fs"));
            let (_, read) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(read.unwrap(), b"first");
        }

        #[test]
        fn errors_carry_their_class_codes() {
            let cases: [(&str, &[u8], ErrorCode); 5] = [
                ("app://attacker", &b"note.txt"[..], ErrorCode::Capability),
                (
                    "app://notes",
                    &b"secrets/secret.txt"[..],
                    ErrorCode::PathDenied,
                ),
                ("app://notes", &b""[..], ErrorCode::PathInvalid),
                ("app://notes", &b"missing.txt"[..], ErrorCode::Handler),
                ("app://notes", &b"\xff\xfe"[..], ErrorCode::Buffer),
            ];
            let (_tmp, _notes, handlers) = notes(FsAccess::READ);
            for (origin, request, code) in cases {
                let err = send(&handlers, FsCommand::ReadFile, origin, request).unwrap_err();
                assert_eq!(err.code(), code, "{origin} {request:?}");
            }
        }

        #[test]
        fn ungranted_origins_are_refused_before_queueing() {
            let (_tmp, notes, handlers) = notes(FsAccess::READ);
            let big = frame("big.bin", &vec![0; 1 << 20]);
            // No grant at all, and a grant without the command's bit
            for (origin, command, request) in [
                ("https://attacker.example", FsCommand::WriteFile, &big[..]),
                (
                    "https://attacker.example",
                    FsCommand::ReadFile,
                    &b"note.txt"[..],
                ),
                ("app://notes", FsCommand::WriteFile, &big[..]),
            ] {
                let (thread, result) = send_on(&handlers, command, origin, request);
                assert_eq!(result.unwrap_err().code(), ErrorCode::Capability);
                assert_ne!(
                    thread.as_deref(),
                    Some("kurogane-fs"),
                    "{origin} was queued"
                );
            }
            assert!(!notes.join("big.bin").exists());
        }

        #[test]
        fn oversized_writes_are_refused_before_queueing() {
            let tmp = tempfile::tempdir().unwrap();
            let mut builder = Filesystem::builder();
            let scope = builder.scope("data", |s| {
                s.allow_directory(tmp.path());
            });
            builder
                .grant(Origin::parse("app://notes").unwrap(), scope, FsAccess::ALL)
                .max_file_size(4);
            let handlers: Vec<_> = handlers(builder.build().unwrap()).collect();
            let (thread, result) = send_on(
                &handlers,
                FsCommand::WriteFile,
                "app://notes",
                &frame("new.txt", b"12345"),
            );
            assert_eq!(result.unwrap_err().code(), ErrorCode::TooLarge);
            assert_ne!(thread.as_deref(), Some("kurogane-fs"));
            let long = vec![b'a'; MAX_PATH_BYTES + 1];
            let (_, result) = send_on(&handlers, FsCommand::ReadFile, "app://notes", &long);
            assert_eq!(result.unwrap_err().code(), ErrorCode::Buffer);
            assert!(!tmp.path().join("new.txt").exists());
        }

        #[test]
        fn transfers_over_the_limit_are_too_large() {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(tmp.path().join("five.txt"), b"hello").unwrap();
            let mut builder = Filesystem::builder();
            let scope = builder.scope("data", |s| {
                s.allow_directory(tmp.path());
            });
            builder
                .grant(Origin::parse("app://notes").unwrap(), scope, FsAccess::ALL)
                .max_file_size(4);
            let handlers: Vec<_> = handlers(builder.build().unwrap()).collect();

            let read =
                send(&handlers, FsCommand::ReadFile, "app://notes", b"five.txt").unwrap_err();
            assert_eq!(read.code(), ErrorCode::TooLarge);
            let write = send(
                &handlers,
                FsCommand::WriteFile,
                "app://notes",
                &frame("new.txt", b"12345"),
            );
            assert_eq!(write.unwrap_err().code(), ErrorCode::TooLarge);
            assert!(!tmp.path().join("new.txt").exists());
        }

        #[test]
        fn metadata_commands_stay_json() {
            let (_tmp, _notes, handlers) = notes(FsAccess::LIST | FsAccess::METADATA);
            let out = send_json(&handlers, FsCommand::ReadDir, json!({ "path": "." })).unwrap();
            let entries = out["entries"].as_array().unwrap();
            assert!(
                entries
                    .iter()
                    .any(|e| e["name"] == "note.txt" && e["kind"] == "file")
            );
            assert!(!entries.iter().any(|e| e["name"] == "secrets"));
            assert_eq!(
                send_json(&handlers, FsCommand::Size, json!({ "path": "note.txt" })).unwrap()["size"],
                5
            );
            let malformed =
                send_json(&handlers, FsCommand::Size, json!({ "file": "note.txt" })).unwrap_err();
            assert_eq!(malformed.code(), ErrorCode::Buffer);
        }
    }
}
