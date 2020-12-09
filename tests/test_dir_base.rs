use std::fs;
use std::os::unix::prelude::*;
use std::path::Path;

use obnth::{Dir, LookupFlags};

#[test]
fn test_parent() {
    let temp_dir = std::env::temp_dir();
    assert!(Dir::open(temp_dir).unwrap().parent().unwrap().is_some());

    assert!(Dir::open("/").unwrap().parent().unwrap().is_none());
}

#[test]
fn test_into_from_raw_fd() {
    let temp_dir = Dir::open(std::env::temp_dir()).unwrap();
    let meta1 = temp_dir.self_metadata().unwrap();

    let temp_dir = unsafe { Dir::from_raw_fd(temp_dir.into_raw_fd()) };
    let meta2 = temp_dir.self_metadata().unwrap();

    assert_eq!(meta1.stat().st_ino, meta2.stat().st_ino);
    assert_eq!(meta1.stat().st_dev, meta2.stat().st_dev);
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
        ("./", LookupFlags::empty(), libc::EEXIST),
        (".//", LookupFlags::empty(), libc::EEXIST),
        ("/", LookupFlags::IN_ROOT, libc::EEXIST),
        ("//", LookupFlags::IN_ROOT, libc::EEXIST),
        ("..", LookupFlags::IN_ROOT, libc::EEXIST),
        ("../", LookupFlags::IN_ROOT, libc::EEXIST),
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
fn test_open_file_lookup_flags() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    tmpdir
        .symlink("link", "target", LookupFlags::empty())
        .unwrap();

    assert_eq!(
        tmpdir
            .open_file()
            .read(true)
            .lookup_flags(LookupFlags::NO_SYMLINKS)
            .open("link")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ELOOP),
    );

    assert_eq!(
        tmpdir
            .open_file()
            .read(true)
            .lookup_flags(LookupFlags::empty())
            .open("/link")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EXDEV),
    );

    assert_eq!(
        tmpdir
            .open_file()
            .read(true)
            .lookup_flags(LookupFlags::IN_ROOT)
            .open("/link")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOENT),
    );
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

    for (path, lookup_flags, allow_enos) in [
        (
            ".",
            LookupFlags::empty(),
            [libc::EISDIR, libc::EPERM].as_ref(),
        ),
        (
            "/",
            LookupFlags::IN_ROOT,
            [libc::EISDIR, libc::EPERM].as_ref(),
        ),
        (
            "..",
            LookupFlags::IN_ROOT,
            [libc::EISDIR, libc::EPERM].as_ref(),
        ),
        (
            "dir",
            LookupFlags::empty(),
            [libc::EISDIR, libc::EPERM].as_ref(),
        ),
        (
            "dir/subfile/..",
            LookupFlags::empty(),
            [libc::ENOTDIR].as_ref(),
        ),
    ]
    .iter()
    {
        let eno = tmpdir
            .remove_file(path, *lookup_flags)
            .unwrap_err()
            .raw_os_error()
            .unwrap();

        assert!(allow_enos.contains(&eno), "{}", eno);
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

#[test]
fn test_change_cwd_to() {
    // No-op... unfortunately we can't test much more without messing up other threads
    Dir::open(".").unwrap().change_cwd_to().unwrap();

    assert_eq!(
        unsafe { Dir::from_raw_fd(-1) }
            .change_cwd_to()
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EBADF)
    );

    assert_eq!(
        unsafe {
            Dir::from_raw_fd(
                fs::File::open(std::env::current_exe().unwrap())
                    .unwrap()
                    .into_raw_fd(),
            )
        }
        .change_cwd_to()
        .unwrap_err()
        .raw_os_error(),
        Some(libc::ENOTDIR)
    );
}
