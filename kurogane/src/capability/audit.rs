//! Cross-layer invariants of the capability layer.
//!
//! 1. The command ACL and filesystem grants are disjoint; no ACL rule yields
//!    native access and an ACL rejection never revokes a grant.
//! 2. [`Filesystem::authorize`] is the only origin-dependent decision: equal
//!    grants under different origins behave identically.
//! 3. Every denial is class-level text that never contains the request.

use std::path::{Path, PathBuf};

use crate::acl::{CommandAcl, Origin};
use crate::capability::test_support::link_dir;
use crate::capability::{Filesystem, FsAccess, FsError};

/// Class-level messages for every capability denial. Each
/// [`Denial`](crate::capability::Denial) variant and
/// [`FsError::CapabilityDenied`] maps to one of these messages.
const CLASS_LEVEL_DENIALS: [&str; 4] = [
    "capability not granted for this origin",
    "path is not within any allowed root",
    "path is denied by the scope's deny patterns",
    "filesystem access denied",
];

fn assert_class_level(err: &FsError, request: &Path) {
    let message = err.to_string();
    assert!(
        CLASS_LEVEL_DENIALS.contains(&message.as_str()),
        "not a class-level denial: {message:?}"
    );
    for component in request.components() {
        let text = component.as_os_str().to_string_lossy();
        if text.len() > 2 {
            assert!(
                !message.contains(&*text),
                "denial echoes {text:?}: {message:?}"
            );
        }
    }
}

fn notes_for(origin: &str) -> (tempfile::TempDir, PathBuf, Filesystem) {
    let tmp = tempfile::tempdir().unwrap();
    let notes = tmp.path().join("notes");
    std::fs::create_dir_all(notes.join("secrets")).unwrap();
    std::fs::write(notes.join("secrets/foo.txt"), b"hidden").unwrap();
    std::fs::write(notes.join("note.txt"), b"hello").unwrap();
    std::fs::write(notes.join("ok.txt"), b"1").unwrap();
    let mut builder = Filesystem::builder();
    let scope = builder.scope("notes", |s| {
        s.allow_directory_recursive(&notes);
        s.deny_path(notes.join("secrets"));
    });
    builder.grant(Origin::parse(origin).unwrap(), scope, FsAccess::ALL);
    (tmp, notes, builder.build().unwrap())
}

/// Rule 1

#[test]
fn acl_permission_never_implies_native_access() {
    let attacker = Origin::parse("https://attacker.example").unwrap();
    let mut acl = CommandAcl::new();
    acl.allow_all("ping").unwrap();
    acl.deny_unlisted();
    assert!(acl.allows("ping", &attacker));

    let (_tmp, _notes, fs) = notes_for("app://notes");
    assert!(
        fs.authorize(&attacker).is_none(),
        "an ACL rule is not a grant"
    );
}

#[test]
fn acl_rejection_never_revokes_a_grant() {
    let granted = Origin::parse("app://notes").unwrap();
    let mut acl = CommandAcl::new();
    acl.deny_unlisted();
    assert!(!acl.allows("ping", &granted));

    let (_tmp, _notes, fs) = notes_for("app://notes");
    assert!(fs.authorize(&granted).is_some());
}

/// Rule 2

#[test]
fn equal_grants_under_different_origins_behave_identically() {
    let (_ta, a_dir, a_fs) = notes_for("app://notes");
    let (_tb, b_dir, b_fs) = notes_for("https://other.example");
    link_dir(&a_dir.join("secrets"), &a_dir.join("alias"));
    link_dir(&b_dir.join("secrets"), &b_dir.join("alias"));
    let a = a_fs
        .authorize(&Origin::parse("app://notes").unwrap())
        .unwrap();
    let b = b_fs
        .authorize(&Origin::parse("https://other.example").unwrap())
        .unwrap();
    assert_eq!(a.access(), b.access());

    assert_eq!(
        a.read_file(&a_dir.join("note.txt")).unwrap(),
        b.read_file(&b_dir.join("note.txt")).unwrap()
    );
    for rel in ["secrets/foo.txt", "alias/foo.txt", "../escape.txt"] {
        let ea = a.read_file(&a_dir.join(rel)).unwrap_err().to_string();
        let eb = b.read_file(&b_dir.join(rel)).unwrap_err().to_string();
        assert_eq!(ea, eb, "{rel}");
    }
}

/// Rule 3

#[test]
fn denials_are_class_level_and_never_echo_the_request() {
    let (_tmp, notes, fs) = notes_for("app://notes");
    link_dir(&notes.join("secrets"), &notes.join("alias"));
    let auth = fs
        .authorize(&Origin::parse("app://notes").unwrap())
        .unwrap();

    let requests = [
        notes.join("secrets/foo.txt"),
        notes.join("alias/foo.txt"),
        notes.join("../../outside/passwd"),
    ];
    for request in &requests {
        assert_class_level(&auth.read_file(request).unwrap_err(), request);
        assert_class_level(&auth.exists(request).unwrap_err(), request);
    }
    let into = notes.join("alias/new.txt");
    assert_class_level(&auth.write_file(&into, b"x").unwrap_err(), &into);
    let moved = notes.join("secrets/stolen.txt");
    assert_class_level(
        &auth.rename_file(&notes.join("ok.txt"), &moved).unwrap_err(),
        &moved,
    );

    let mut builder = Filesystem::builder();
    let scope = builder.scope("notes", |s| {
        s.allow_directory_recursive(&notes);
    });
    builder.grant(Origin::parse("app://notes").unwrap(), scope, FsAccess::READ);
    let read_only = builder.build().unwrap();
    let ro = read_only
        .authorize(&Origin::parse("app://notes").unwrap())
        .unwrap();
    let target = notes.join("note.txt");
    assert_class_level(&ro.write_file(&target, b"x").unwrap_err(), &target);
}
