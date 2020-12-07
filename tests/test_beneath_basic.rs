use std::fs;
use std::io;
use std::os::unix::prelude::*;

use obnth::{open_beneath, LookupFlags};

fn same_file_meta(f1: &fs::File, m2: &fs::Metadata) -> io::Result<bool> {
    let m1 = f1.metadata()?;

    Ok(m1.ino() == m2.ino() && m1.dev() == m2.dev())
}

#[test]
fn test_open_beneath_success() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir = tmpdir.as_ref();

    let tmpdir_file = fs::File::open(tmpdir).unwrap();
    let tmpdir_fd = tmpdir_file.as_raw_fd();

    fs::create_dir(tmpdir.join("a")).unwrap();
    fs::File::create(tmpdir.join("a/b")).unwrap();
    fs::create_dir(tmpdir.join("a/sub")).unwrap();

    std::os::unix::fs::symlink("a/b", tmpdir.join("c")).unwrap();
    std::os::unix::fs::symlink("/a/b", tmpdir.join("d")).unwrap();
    std::os::unix::fs::symlink("a/", tmpdir.join("e")).unwrap();
    std::os::unix::fs::symlink("/", tmpdir.join("f")).unwrap();

    for (path, flags, lookup_flags, same_path) in [
        (
            "a",
            libc::O_RDONLY | libc::O_DIRECTORY,
            LookupFlags::empty(),
            "a",
        ),
        ("a", libc::O_PATH, LookupFlags::empty(), "a"),
        (
            "a",
            libc::O_PATH | libc::O_NOFOLLOW,
            LookupFlags::empty(),
            "a",
        ),
        ("a/b", libc::O_RDONLY, LookupFlags::empty(), "a/b"),
        ("a/../a", libc::O_RDONLY, LookupFlags::empty(), "a"),
        ("a/../a/b", libc::O_RDONLY, LookupFlags::empty(), "a/b"),
        ("a/sub/..", libc::O_RDONLY, LookupFlags::empty(), "a"),
        ("a/sub/../..", libc::O_RDONLY, LookupFlags::empty(), "."),
        ("e", libc::O_RDONLY, LookupFlags::IN_ROOT, "a"),
        ("a/b", libc::O_WRONLY, LookupFlags::empty(), "a/b"),
        ("c", libc::O_WRONLY, LookupFlags::empty(), "a/b"),
        ("c", libc::O_PATH, LookupFlags::empty(), "a/b"),
        (
            "c",
            libc::O_PATH | libc::O_NOFOLLOW,
            LookupFlags::empty(),
            "c",
        ),
        (".", libc::O_RDONLY, LookupFlags::empty(), "."),
        ("./", libc::O_RDONLY, LookupFlags::empty(), "."),
        ("c", libc::O_WRONLY, LookupFlags::IN_ROOT, "a/b"),
        ("d", libc::O_WRONLY, LookupFlags::IN_ROOT, "a/b"),
        ("f", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("a/..", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("a/../..", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("../../", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("/a/../..", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("/../../", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
    ]
    .iter()
    {
        let f = open_beneath(tmpdir_fd, path, *flags, 0o666, *lookup_flags).unwrap();

        assert!(same_file_meta(&f, &tmpdir.join(same_path).symlink_metadata().unwrap()).unwrap());
    }

    open_beneath(
        tmpdir_fd,
        "a/sub/file",
        libc::O_WRONLY | libc::O_CREAT,
        0o600,
        LookupFlags::empty(),
    )
    .unwrap();
    assert_eq!(
        tmpdir.join("a/sub/file").metadata().unwrap().mode(),
        0o100600
    );
}

#[test]
fn test_open_beneath_error() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir = tmpdir.as_ref();

    let tmpdir_file = fs::File::open(tmpdir).unwrap();
    let tmpdir_fd = tmpdir_file.as_raw_fd();

    fs::create_dir(tmpdir.join("a")).unwrap();
    fs::File::create(tmpdir.join("a/b")).unwrap();
    fs::create_dir(tmpdir.join("a/sub")).unwrap();

    std::os::unix::fs::symlink("a/b", tmpdir.join("c")).unwrap();
    std::os::unix::fs::symlink("/a/b", tmpdir.join("d")).unwrap();
    std::os::unix::fs::symlink("a/", tmpdir.join("e")).unwrap();
    std::os::unix::fs::symlink("/", tmpdir.join("f")).unwrap();

    std::os::unix::fs::symlink("loop", tmpdir.join("loop")).unwrap();

    assert_eq!(
        open_beneath(
            libc::AT_FDCWD,
            ".",
            libc::O_RDONLY,
            0o666,
            LookupFlags::empty()
        )
        .unwrap_err()
        .raw_os_error(),
        Some(libc::EBADF)
    );

    assert_eq!(
        open_beneath(
            fs::File::open(tmpdir.join("a/b")).unwrap().as_raw_fd(),
            ".",
            libc::O_RDONLY,
            0o666,
            LookupFlags::empty()
        )
        .unwrap_err()
        .raw_os_error(),
        Some(libc::ENOTDIR)
    );

    for (path, flags, lookup_flags, eno) in [
        (
            "NOEXIST",
            libc::O_RDONLY | libc::O_DIRECTORY,
            LookupFlags::empty(),
            libc::ENOENT,
        ),
        (
            "a/b",
            libc::O_RDONLY | libc::O_DIRECTORY,
            LookupFlags::empty(),
            libc::ENOTDIR,
        ),
        ("a/b/", libc::O_RDONLY, LookupFlags::empty(), libc::ENOTDIR),
        ("a/b/.", libc::O_RDONLY, LookupFlags::empty(), libc::ENOTDIR),
        ("c", libc::O_RDONLY, LookupFlags::NO_SYMLINKS, libc::ELOOP),
        ("d", libc::O_RDONLY, LookupFlags::NO_SYMLINKS, libc::ELOOP),
        ("loop", libc::O_RDONLY, LookupFlags::empty(), libc::ELOOP),
        ("d", libc::O_RDONLY, LookupFlags::empty(), libc::EXDEV),
        ("e", libc::O_WRONLY, LookupFlags::empty(), libc::EISDIR),
        ("f", libc::O_RDONLY, LookupFlags::empty(), libc::EXDEV),
        ("..", libc::O_RDONLY, LookupFlags::empty(), libc::EXDEV),
        ("", libc::O_RDONLY, LookupFlags::empty(), libc::ENOENT),
        ("/", libc::O_RDONLY, LookupFlags::empty(), libc::EXDEV),
        (
            "a/../../a/b",
            libc::O_RDONLY,
            LookupFlags::empty(),
            libc::EXDEV,
        ),
        ("a/../..", libc::O_RDONLY, LookupFlags::empty(), libc::EXDEV),
        (
            "a/sub/../../..",
            libc::O_RDONLY,
            LookupFlags::empty(),
            libc::EXDEV,
        ),
    ]
    .iter()
    {
        assert_eq!(
            open_beneath(tmpdir_fd, path, *flags, 0o666, *lookup_flags)
                .unwrap_err()
                .raw_os_error(),
            Some(*eno)
        );
    }
}
