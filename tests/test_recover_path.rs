use std::fs;
use std::path::Path;

use obnth::Dir;

#[test]
fn test_recover_path_tmpdir() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    assert_eq!(
        tmpdir.recover_path().unwrap(),
        tmpdir_path.canonicalize().unwrap()
    );
}

#[test]
fn test_recover_path_remove() {
    // Create the directory, open it, and *then* remove it.
    let tmpdir_path = tempfile::tempdir().unwrap().into_path();
    let tmpdir = Dir::open(&tmpdir_path).unwrap();
    fs::remove_dir(&tmpdir_path).unwrap();

    assert_eq!(
        tmpdir.recover_path().unwrap_err().raw_os_error(),
        Some(libc::ENOENT)
    );
}

#[test]
fn test_recover_path_rename() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();

    fs::create_dir(tmpdir_path.join("a")).unwrap();
    let tmpdir_a = Dir::open(tmpdir_path.join("a")).unwrap();

    // The name is correct at first
    assert_eq!(tmpdir_a.recover_path().unwrap(), tmpdir_path.join("a"));

    // Rename it...
    fs::rename(tmpdir_path.join("a"), tmpdir_path.join("b")).unwrap();

    // And the name updates
    assert_eq!(tmpdir_a.recover_path().unwrap(), tmpdir_path.join("b"));
}

#[test]
fn test_recover_path_deleted_name() {
    // On Linux, adding a " (deleted)" suffix to the file name will make it look like it's been
    // deleted and trigger the fallback path recovery code.

    let tmpdir = tempfile::Builder::new()
        .suffix(" (deleted)")
        .tempdir()
        .unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    assert_eq!(
        tmpdir.recover_path().unwrap(),
        tmpdir_path.canonicalize().unwrap()
    );
}

#[test]
fn test_recover_path_root() {
    assert_eq!(
        Dir::open("/").unwrap().recover_path().unwrap(),
        Path::new("/"),
    );
}

#[test]
fn test_recover_path_tmp() {
    let temp_dir = std::env::temp_dir();

    assert_eq!(
        Dir::open(&temp_dir).unwrap().recover_path().unwrap(),
        temp_dir.canonicalize().unwrap(),
    );
}
