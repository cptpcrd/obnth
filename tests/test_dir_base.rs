use std::fs;
use std::path::Path;

use obnth::{Dir, LookupFlags};

#[test]
fn test_parent() {
    let temp_dir = std::env::temp_dir();
    assert!(Dir::open(temp_dir).unwrap().parent().unwrap().is_some());

    assert!(Dir::open("/").unwrap().parent().unwrap().is_none());
}

#[test]
fn test_create_remove_dir() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    tmpdir
        .create_dir("dir", 0o777, LookupFlags::empty())
        .unwrap();
    tmpdir
        .create_dir("dir/subdir", 0o777, LookupFlags::empty())
        .unwrap();

    for (path, lookup_flags, eno) in [
        ("dir", LookupFlags::empty(), libc::EEXIST),
        ("dir/subdir", LookupFlags::empty(), libc::EEXIST),
        (".", LookupFlags::empty(), libc::EEXIST),
        ("/", LookupFlags::IN_ROOT, libc::EEXIST),
        ("..", LookupFlags::IN_ROOT, libc::EEXIST),
        ("dir/subdir/..", LookupFlags::empty(), libc::EEXIST),
    ]
    .iter()
    {
        assert_eq!(
            tmpdir
                .create_dir(path, 0o777, *lookup_flags)
                .unwrap_err()
                .raw_os_error(),
            Some(*eno)
        );
    }

    for (path, lookup_flags, eno) in [
        (".", LookupFlags::empty(), libc::EBUSY),
        ("/", LookupFlags::IN_ROOT, libc::EBUSY),
        ("..", LookupFlags::IN_ROOT, libc::EBUSY),
        ("dir", LookupFlags::empty(), libc::ENOTEMPTY),
        ("dir/subdir/..", LookupFlags::empty(), libc::ENOTEMPTY),
    ]
    .iter()
    {
        assert_eq!(
            tmpdir
                .remove_dir(path, *lookup_flags)
                .unwrap_err()
                .raw_os_error(),
            Some(*eno)
        );
    }

    tmpdir
        .remove_dir("dir/subdir", LookupFlags::empty())
        .unwrap();
    tmpdir.remove_dir("dir", LookupFlags::empty()).unwrap();
}

#[test]
fn test_remove_file() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    fs::File::create(&tmpdir_path.join("file")).unwrap();

    tmpdir
        .create_dir("dir", 0o777, LookupFlags::empty())
        .unwrap();

    fs::File::create(&tmpdir_path.join("dir/subfile")).unwrap();

    for (path, lookup_flags, eno) in [
        (".", LookupFlags::empty(), libc::EISDIR),
        ("/", LookupFlags::IN_ROOT, libc::EISDIR),
        ("..", LookupFlags::IN_ROOT, libc::EISDIR),
        ("dir", LookupFlags::empty(), libc::EISDIR),
        ("dir/subfile/..", LookupFlags::empty(), libc::ENOTDIR),
    ]
    .iter()
    {
        assert_eq!(
            tmpdir
                .remove_file(path, *lookup_flags)
                .unwrap_err()
                .raw_os_error(),
            Some(*eno)
        );
    }

    tmpdir
        .remove_file("dir/subfile", LookupFlags::empty())
        .unwrap();
    tmpdir.remove_file("file", LookupFlags::empty()).unwrap();
}

#[test]
fn test_symlinks() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    fs::File::create(&tmpdir_path.join("file")).unwrap();

    tmpdir
        .symlink("link", "target", LookupFlags::empty())
        .unwrap();

    tmpdir
        .create_dir("dir", 0o777, LookupFlags::empty())
        .unwrap();

    tmpdir
        .symlink("dir/sublink", "subtarget", LookupFlags::empty())
        .unwrap();

    for (path, lookup_flags, eno) in [
        (".", LookupFlags::empty(), libc::EEXIST),
        ("/", LookupFlags::IN_ROOT, libc::EEXIST),
        ("..", LookupFlags::IN_ROOT, libc::EEXIST),
        ("dir", LookupFlags::empty(), libc::EEXIST),
        ("dir/sublink/..", LookupFlags::empty(), libc::ENOENT),
    ]
    .iter()
    {
        assert_eq!(
            tmpdir
                .symlink(path, "target", *lookup_flags)
                .unwrap_err()
                .raw_os_error(),
            Some(*eno)
        );
    }

    assert_eq!(
        tmpdir.read_link("link", LookupFlags::empty()).unwrap(),
        Path::new("target"),
    );
    assert_eq!(
        tmpdir.read_link("/link", LookupFlags::IN_ROOT).unwrap(),
        Path::new("target"),
    );

    assert_eq!(
        tmpdir
            .read_link("dir/../link", LookupFlags::IN_ROOT)
            .unwrap(),
        Path::new("target"),
    );

    assert_eq!(
        tmpdir
            .read_link("dir/sublink", LookupFlags::empty())
            .unwrap(),
        Path::new("subtarget"),
    );

    for (path, lookup_flags, eno) in [
        (".", LookupFlags::empty(), libc::EINVAL),
        ("/", LookupFlags::IN_ROOT, libc::EINVAL),
        ("..", LookupFlags::IN_ROOT, libc::EINVAL),
        ("dir", LookupFlags::empty(), libc::EINVAL),
        ("file", LookupFlags::empty(), libc::EINVAL),
        ("dir/sublink/..", LookupFlags::empty(), libc::ENOENT),
    ]
    .iter()
    {
        assert_eq!(
            tmpdir
                .read_link(path, *lookup_flags)
                .unwrap_err()
                .raw_os_error(),
            Some(*eno),
        );
    }
}
