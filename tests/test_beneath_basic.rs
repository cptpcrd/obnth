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
    std::os::unix::fs::symlink("./b", tmpdir.join("a/g")).unwrap();

    for (path, flags, lookup_flags, same_path) in [
        (
            "a",
            libc::O_RDONLY | libc::O_DIRECTORY,
            LookupFlags::empty(),
            "a",
        ),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        ("a", libc::O_PATH, LookupFlags::empty(), "a"),
        #[cfg(any(target_os = "linux", target_os = "android"))]
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
        #[cfg(any(target_os = "linux", target_os = "android"))]
        ("c", libc::O_PATH, LookupFlags::empty(), "a/b"),
        #[cfg(any(target_os = "linux", target_os = "android"))]
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
        ("a/g", libc::O_RDONLY, LookupFlags::IN_ROOT, "a/b"),
        ("a/..", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("a/../..", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("../../", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("/a/../..", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("/../../", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
        ("/", libc::O_RDONLY, LookupFlags::IN_ROOT, "."),
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
        (
            "c",
            libc::O_RDONLY | libc::O_NOFOLLOW,
            LookupFlags::empty(),
            libc::ELOOP,
        ),
        (
            "d",
            libc::O_RDONLY | libc::O_NOFOLLOW,
            LookupFlags::empty(),
            libc::ELOOP,
        ),
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

#[test]
fn test_open_beneath_execute() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir = tmpdir.as_ref();

    let tmpdir_file = fs::File::open(tmpdir).unwrap();
    let tmpdir_fd = tmpdir_file.as_raw_fd();

    fs::create_dir(tmpdir.join("a")).unwrap();
    fs::File::create(tmpdir.join("a/b")).unwrap();

    // 0o100 is "--x------"; i.e. execute permission but not read permission.
    // That allows us to look at files within the directory, but not list the directory (or open it
    // without O_PATH or O_SEARCH).
    std::fs::set_permissions(tmpdir.join("a"), fs::Permissions::from_mode(0o100)).unwrap();

    let res = std::panic::catch_unwind(|| {
        if obnth::has_o_search() {
            let file =
                obnth::open_beneath(tmpdir_fd, "a/b", libc::O_RDONLY, 0, LookupFlags::empty())
                    .unwrap();

            assert!(same_file_meta(&file, &fs::metadata(tmpdir.join("a/b")).unwrap()).unwrap());
        }

        if unsafe { libc::geteuid() } != 0 {
            if !obnth::has_o_search() {
                assert_eq!(
                    obnth::open_beneath(tmpdir_fd, "a/b", libc::O_RDONLY, 0, LookupFlags::empty())
                        .unwrap_err()
                        .raw_os_error(),
                    Some(libc::EACCES)
                );
            }

            assert_eq!(
                obnth::open_beneath(tmpdir_fd, "a", libc::O_RDONLY, 0, LookupFlags::empty())
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EACCES)
            );
        }
    });

    // So it can be deleted
    std::fs::set_permissions(tmpdir.join("a"), fs::Permissions::from_mode(0o755)).unwrap();
    res.unwrap();
}
