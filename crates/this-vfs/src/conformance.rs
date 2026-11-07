//! Conformance bench of the `Vfs` contract, run by every backend's tests
//! (`vfs_conformance_<case>_<backend>`). Each case uses its own sub-tree so a backend may
//! share one instance across cases.

use super::{Precondition, Vfs, VfsEntry, VfsError, VfsPath, etag_of};

fn p(s: &str) -> VfsPath {
    VfsPath::parse(s).unwrap()
}

pub async fn read_write_roundtrip(v: &dyn Vfs) {
    let path = p("/rw/module.wasm");
    assert!(v.stat(&path).await.unwrap().is_none());
    let bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    let meta = v.write(&path, &bytes, Precondition::None).await.unwrap();
    assert_eq!(meta.size, 4096);
    assert_eq!(meta.etag, etag_of(&bytes));
    assert_eq!(v.read(&path).await.unwrap(), bytes);
    let st = v.stat(&path).await.unwrap().unwrap();
    assert_eq!(st.etag, meta.etag);
    assert_eq!(st.size, 4096);
    // Overwrite without precondition, then empty content.
    v.write(&path, b"v2", Precondition::None).await.unwrap();
    assert_eq!(v.read(&path).await.unwrap(), b"v2");
    v.write(&path, b"", Precondition::None).await.unwrap();
    assert_eq!(v.read(&path).await.unwrap(), b"");
    assert_eq!(v.stat(&path).await.unwrap().unwrap().etag, etag_of(b""));
    assert!(matches!(
        v.read(&p("/rw/absent")).await,
        Err(VfsError::NotFound(_))
    ));
}

pub async fn if_match_conflict(v: &dyn Vfs) {
    let path = p("/im/descriptor.json");
    let m1 = v.write(&path, b"one", Precondition::None).await.unwrap();
    let m2 = v
        .write(&path, b"two", Precondition::IfMatch(m1.etag.clone()))
        .await
        .unwrap();
    // Stale etag: refused, content untouched, current etag reported.
    match v
        .write(&path, b"three", Precondition::IfMatch(m1.etag.clone()))
        .await
    {
        Err(VfsError::PreconditionFailed { current, .. }) => {
            assert_eq!(current.as_deref(), Some(m2.etag.as_str()))
        }
        other => panic!("expected PreconditionFailed, got {other:?}"),
    }
    assert_eq!(v.read(&path).await.unwrap(), b"two");
    // IfMatch on a missing file fails too.
    assert!(matches!(
        v.write(
            &p("/im/missing"),
            b"x",
            Precondition::IfMatch(m2.etag.clone())
        )
        .await,
        Err(VfsError::PreconditionFailed { current: None, .. })
    ));
    // Delete with a stale etag refused, with the right one accepted.
    assert!(matches!(
        v.delete(&path, Precondition::IfMatch(m1.etag)).await,
        Err(VfsError::PreconditionFailed { .. })
    ));
    v.delete(&path, Precondition::IfMatch(m2.etag))
        .await
        .unwrap();
    assert!(v.stat(&path).await.unwrap().is_none());
}

pub async fn if_none_match_create_only(v: &dyn Vfs) {
    let path = p("/inm/manifest.json");
    v.write(&path, b"first", Precondition::IfNoneMatch)
        .await
        .unwrap();
    assert!(matches!(
        v.write(&path, b"second", Precondition::IfNoneMatch).await,
        Err(VfsError::PreconditionFailed {
            current: Some(_),
            ..
        })
    ));
    assert_eq!(v.read(&path).await.unwrap(), b"first");
}

pub async fn list_dir(v: &dyn Vfs) {
    for f in [
        "/ls/m/1.0.0/descriptor.json",
        "/ls/m/1.0.0/i18n/fr.json",
        "/ls/m/1.0.0/i18n/en.json",
        "/ls/m/2.0.0/descriptor.json",
        "/ls/top.txt",
    ] {
        v.write(&p(f), f.as_bytes(), Precondition::None)
            .await
            .unwrap();
    }
    let names = |es: Vec<VfsEntry>| {
        es.into_iter()
            .map(|e| match e {
                VfsEntry::File { path, meta } => {
                    assert_eq!(meta.size as usize, path.as_str().len());
                    format!("F {path}")
                }
                VfsEntry::Dir { path } => format!("D {path}"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(v.list(&p("/ls")).await.unwrap()),
        ["D /ls/m", "F /ls/top.txt"]
    );
    assert_eq!(
        names(v.list(&p("/ls/m")).await.unwrap()),
        ["D /ls/m/1.0.0", "D /ls/m/2.0.0"]
    );
    assert_eq!(
        names(v.list(&p("/ls/m/1.0.0")).await.unwrap()),
        ["F /ls/m/1.0.0/descriptor.json", "D /ls/m/1.0.0/i18n"]
    );
    assert_eq!(
        names(v.list(&p("/ls/m/1.0.0/i18n")).await.unwrap()),
        ["F /ls/m/1.0.0/i18n/en.json", "F /ls/m/1.0.0/i18n/fr.json"]
    );
    // Missing directory: empty, not an error.
    assert!(v.list(&p("/ls/nothing")).await.unwrap().is_empty());
    // Root lists the top-level directory.
    assert!(
        names(v.list(&p("/")).await.unwrap()).contains(&"D /ls".to_string()),
        "root must list /ls"
    );
    // A directory disappears with its last file.
    v.delete(&p("/ls/m/2.0.0/descriptor.json"), Precondition::None)
        .await
        .unwrap();
    assert_eq!(names(v.list(&p("/ls/m")).await.unwrap()), ["D /ls/m/1.0.0"]);
}

pub async fn delete_missing_not_found(v: &dyn Vfs) {
    let path = p("/del/x");
    assert!(matches!(
        v.delete(&path, Precondition::None).await,
        Err(VfsError::NotFound(_))
    ));
    v.write(&path, b"x", Precondition::None).await.unwrap();
    v.delete(&path, Precondition::None).await.unwrap();
    assert!(matches!(
        v.delete(&path, Precondition::None).await,
        Err(VfsError::NotFound(_))
    ));
    assert!(matches!(v.read(&path).await, Err(VfsError::NotFound(_))));
}

pub async fn shape_conflicts(v: &dyn Vfs) {
    v.write(&p("/shape/a/b"), b"x", Precondition::None)
        .await
        .unwrap();
    // A file cannot replace a directory, nor live below a file, nor be the root.
    assert!(matches!(
        v.write(&p("/shape/a"), b"x", Precondition::None).await,
        Err(VfsError::InvalidPath(_))
    ));
    assert!(matches!(
        v.write(&p("/shape/a/b/c"), b"x", Precondition::None).await,
        Err(VfsError::InvalidPath(_))
    ));
    assert!(matches!(
        v.write(&p("/"), b"x", Precondition::None).await,
        Err(VfsError::InvalidPath(_))
    ));
}

/// Empty content is a valid file: it is written, read back as empty, and its etag is
/// the etag of the empty content (never a path-derived or missing value). The module
/// store refuses empty *assets* at its own level; at the VFS level an empty file must
/// still be storable and readable.
pub async fn empty_file(v: &dyn Vfs) {
    let path = p("/empty/descriptor.json");
    let meta = v.write(&path, b"", Precondition::None).await.unwrap();
    assert_eq!(meta.size, 0);
    assert_eq!(meta.etag, etag_of(b""));
    assert_eq!(v.read(&path).await.unwrap(), b"");
    assert_eq!(v.stat(&path).await.unwrap().unwrap().etag, meta.etag);
}

/// Etags are content-addressed: equal bytes produce equal etags on the same backend,
/// whatever the path, and different bytes produce different etags. This is the property
/// idempotent puts (immutability) and integrity checks rely on.
pub async fn etag_is_content_addressed(v: &dyn Vfs) {
    let a = p("/ca/a");
    let b = p("/ca/b");
    let bytes = b"same content";
    let ma = v.write(&a, bytes, Precondition::None).await.unwrap();
    let mb = v.write(&b, bytes, Precondition::None).await.unwrap();
    assert_eq!(ma.etag, mb.etag, "equal content => equal etag");
    let other = v.write(&a, b"other", Precondition::None).await.unwrap();
    assert_ne!(ma.etag, other.etag, "different content => different etag");
    // Re-writing the same bytes keeps the etag (idempotence of the content address).
    let again = v.write(&a, bytes, Precondition::None).await.unwrap();
    assert_eq!(again.etag, ma.etag, "same content rewritten => same etag");
}

/// Every case, in one call (backends with an expensive setup).
pub async fn run_all(v: &dyn Vfs) {
    read_write_roundtrip(v).await;
    if_match_conflict(v).await;
    if_none_match_create_only(v).await;
    list_dir(v).await;
    delete_missing_not_found(v).await;
    shape_conflicts(v).await;
    empty_file(v).await;
    etag_is_content_addressed(v).await;
}
