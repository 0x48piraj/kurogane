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

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};

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

/// One handler per `fs.*` command, all feeding one worker.
pub(crate) fn handlers(filesystem: Filesystem) -> impl Iterator<Item = (FsCommand, AsyncHandler)> {
    let worker = Arc::new(Worker {
        filesystem: Arc::new(filesystem),
        queue: OnceLock::new(),
    });
    FsCommand::ALL
        .into_iter()
        .map(move |command| (command, handler(Arc::clone(&worker), command)))
}

fn handler(worker: Arc<Worker>, command: FsCommand) -> AsyncHandler {
    Box::new(move |data, responder, ctx| {
        worker.submit(Job {
            command,
            data: data.to_vec(),
            origin: ctx.origin,
            responder,
        });
    })
}

/// One queued operation. Plain data, so a refused job still reaches its
/// responder.
struct Job {
    command: FsCommand,
    data: Vec<u8>,
    origin: Origin,
    responder: BinaryResponder,
}

impl Job {
    fn run(self, filesystem: &Filesystem) {
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
}

impl Worker {
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

    #[cfg(any(target_os = "linux", windows))]
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
            }
        }

        /// Sends `request` to the handler for `command` and waits for the answer.
        fn send(
            handlers: &[(FsCommand, AsyncHandler)],
            command: FsCommand,
            origin: &str,
            request: &[u8],
        ) -> Result<Vec<u8>, IpcError> {
            let (_, handler) = handlers.iter().find(|(c, _)| *c == command).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let responder = BinaryResponder::new(Box::new(move |r| {
                let _ = tx.send(r);
            }));
            handler(request, responder, ctx(origin));
            rx.recv_timeout(Duration::from_secs(10))
                .expect("handler resolved")
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
