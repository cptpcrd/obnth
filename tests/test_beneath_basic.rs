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

    macro_rules! check_ok {
        ($path:expr, $flags:expr, $lookup_flags:expr, $same_path:expr $(,)?) => {
            let f = open_beneath(tmpdir_fd, $path, $flags, 0o666, $lookup_flags).unwrap();

            assert!(
                same_file_meta(&f, &tmpdir.join($same_path).symlink_metadata().unwrap()).unwrap()
            );
        };

        ($path:expr, $flags:expr, $same_path:expr $(,)?) => {
            check_ok!($path, $flags, LookupFlags::empty(), $same_path)
        };
    }

    std::os::unix::fs::symlink("a/b", tmpdir.join("c")).unwrap();
    std::os::unix::fs::symlink("/a/b", tmpdir.join("d")).unwrap();
    std::os::unix::fs::symlink("a/", tmpdir.join("e")).unwrap();
    std::os::unix::fs::symlink("/", tmpdir.join("f")).unwrap();
    std::os::unix::fs::symlink("./b", tmpdir.join("a/g")).unwrap();
    std::os::unix::fs::symlink(".", tmpdir.join("a/h")).unwrap();
    std::os::unix::fs::symlink("/escape", tmpdir.join("a/i")).unwrap();
    std::os::unix::fs::symlink("/a/", tmpdir.join("j")).unwrap();
    std::os::unix::fs::symlink("a", tmpdir.join("k")).unwrap();

    check_ok!("a", libc::O_RDONLY | libc::O_DIRECTORY, "a");
    check_ok!("a/b", libc::O_RDONLY, "a/b");
    check_ok!("a/../a", libc::O_RDONLY, "a");
    check_ok!("a/../a/b", libc::O_RDONLY, "a/b");
    check_ok!("a/sub/..", libc::O_RDONLY, "a");
    check_ok!("a/sub/../..", libc::O_RDONLY, ".");

    check_ok!("e", libc::O_RDONLY, LookupFlags::IN_ROOT, "a");
    check_ok!("a/b", libc::O_WRONLY, "a/b");
    check_ok!("c", libc::O_WRONLY, "a/b");

    check_ok!("e/", libc::O_RDONLY, "a/");
    check_ok!("e/.", libc::O_RDONLY, "a/");
    check_ok!("k/", libc::O_RDONLY, "a/");
    check_ok!("k/.", libc::O_RDONLY, "a/");

    check_ok!(".", libc::O_RDONLY, ".");
    check_ok!("./", libc::O_RDONLY, ".");
    check_ok!("/", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");

    check_ok!("c", libc::O_WRONLY, LookupFlags::IN_ROOT, "a/b");
    check_ok!("d", libc::O_WRONLY, LookupFlags::IN_ROOT, "a/b");
    check_ok!("j/", libc::O_RDONLY, LookupFlags::IN_ROOT, "a");
    check_ok!("f", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");

    check_ok!("a/g", libc::O_RDONLY, LookupFlags::IN_ROOT, "a/b");
    check_ok!("a/h", libc::O_RDONLY, LookupFlags::IN_ROOT, "a");
    check_ok!("a/..", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");
    check_ok!("a/../..", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");
    check_ok!("/a/../../", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");
    check_ok!("../..", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");
    check_ok!("../../", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");
    check_ok!("/../../", libc::O_RDONLY, LookupFlags::IN_ROOT, ".");

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        check_ok!("a", libc::O_PATH, "a");
        check_ok!("a", libc::O_PATH | libc::O_NOFOLLOW, "a");

        check_ok!("c", libc::O_PATH, "a/b");
        check_ok!("c", libc::O_PATH | libc::O_NOFOLLOW, "c");

        check_ok!("e/", libc::O_PATH, "a/");
        check_ok!("e/.", libc::O_PATH, "a/");
        check_ok!("k/", libc::O_PATH, "a/");
        check_ok!("k/.", libc::O_PATH, "a/");
    }

    // Trying to open(O_CREAT) a symlink will *not* let the OS follow the symlink and escape. So it
    // fails with EXDEV (escape detected) rather than succeeding (if running as root and able to
    // create `/escape`) or failing with EACCES (if not running as root).
    assert_eq!(
        open_beneath(
            tmpdir_fd,
            "a/i",
            libc::O_WRONLY | libc::O_CREAT,
            0o666,
            LookupFlags::empty()
        )
        .unwrap_err()
        .raw_os_error(),
        Some(libc::EXDEV),
    );

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
    std::os::unix::fs::symlink("./b", tmpdir.join("a/g")).unwrap();
    std::os::unix::fs::symlink(".", tmpdir.join("a/h")).unwrap();

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

    macro_rules! check_err {
        ($path:expr, $flags:expr, $lookup_flags:expr, $eno:expr $(,)?) => {
            assert_eq!(
                open_beneath(tmpdir_fd, $path, $flags, 0o666, $lookup_flags)
                    .unwrap_err()
                    .raw_os_error(),
                Some($eno)
            );
        };

        ($path:expr, $flags:expr, $eno:expr $(,)?) => {
            check_err!($path, $flags, LookupFlags::empty(), $eno)
        };
    }

    check_err!("", libc::O_RDONLY, libc::ENOENT);

    check_err!("NOEXIST", libc::O_RDONLY | libc::O_DIRECTORY, libc::ENOENT);
    check_err!("a/b", libc::O_RDONLY | libc::O_DIRECTORY, libc::ENOTDIR);
    check_err!("a/b/", libc::O_RDONLY, libc::ENOTDIR);
    check_err!("a/b/.", libc::O_RDONLY, libc::ENOTDIR);
    check_err!("a/b/./", libc::O_RDONLY, libc::ENOTDIR);

    check_err!("c", libc::O_RDONLY, LookupFlags::NO_SYMLINKS, libc::ELOOP);
    check_err!("d", libc::O_RDONLY, LookupFlags::NO_SYMLINKS, libc::ELOOP);
    check_err!("c", libc::O_RDONLY | libc::O_NOFOLLOW, libc::ELOOP);
    check_err!("d", libc::O_RDONLY | libc::O_NOFOLLOW, libc::ELOOP);

    check_err!("e/b", libc::O_RDONLY, LookupFlags::NO_SYMLINKS, libc::ELOOP);
    check_err!(
        "loop",
        libc::O_RDONLY,
        LookupFlags::NO_SYMLINKS,
        libc::ELOOP
    );

    check_err!("c/", libc::O_RDONLY, libc::ENOTDIR);

    check_err!("d", libc::O_RDONLY, libc::EXDEV);
    check_err!("d/", libc::O_RDONLY, libc::EXDEV);
    check_err!("f", libc::O_RDONLY, libc::EXDEV);
    check_err!("..", libc::O_RDONLY, libc::EXDEV);
    check_err!("/", libc::O_RDONLY, libc::EXDEV);
    check_err!("a/../../a/b", libc::O_RDONLY, libc::EXDEV);
    check_err!("a/../..", libc::O_RDONLY, libc::EXDEV);
    check_err!("a/sub/../../..", libc::O_RDONLY, libc::EXDEV);

    check_err!("a/h", libc::O_WRONLY, libc::EISDIR);
    check_err!("e", libc::O_WRONLY, libc::EISDIR);
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
