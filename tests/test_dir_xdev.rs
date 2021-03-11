use std::fs;
use std::os::unix::prelude::*;

use obnth::{open_beneath, LookupFlags};

#[test]
fn test_open_beneath_xdev() {
    let rootdir = fs::File::open("/").unwrap();
    let rootdir_fd = rootdir.as_raw_fd();

    macro_rules! check_ok {
        ($path:expr, $flags:expr, $lookup_flags:expr $(,)?) => {
            open_beneath(
                rootdir_fd,
                $path,
                $flags,
                0o666,
                $lookup_flags | LookupFlags::NO_XDEV | LookupFlags::IN_ROOT,
            )
            .unwrap();
        };

        ($path:expr, $flags:expr $(,)?) => {
            check_ok!($path, $flags, LookupFlags::empty())
        };
    }

    macro_rules! check_err {
        ($path:expr, $flags:expr, $lookup_flags:expr, $eno:expr $(,)?) => {
            assert_eq!(
                open_beneath(
                    rootdir_fd,
                    $path,
                    $flags,
                    0o666,
                    $lookup_flags | LookupFlags::NO_XDEV | LookupFlags::IN_ROOT,
                )
                .unwrap_err()
                .raw_os_error(),
                Some($eno)
            );
        };

        ($path:expr, $flags:expr, $eno:expr $(,)?) => {
            check_err!($path, $flags, LookupFlags::empty(), $eno)
        };
    }

    check_ok!(".", libc::O_RDONLY);
    check_ok!("bin/..", libc::O_RDONLY);

    check_ok!("bin", libc::O_RDONLY);
    check_ok!("bin/../bin", libc::O_RDONLY);
    check_ok!("bin/../../bin", libc::O_RDONLY);

    check_ok!("bin/true", libc::O_RDONLY);
    check_ok!("bin/../bin/true", libc::O_RDONLY);
    check_ok!("bin/../../bin/true", libc::O_RDONLY);

    check_err!("dev", libc::O_RDONLY, libc::EXDEV);
    check_err!("dev/fd", libc::O_RDONLY, libc::EXDEV);
    check_err!(
        "bin/../../dev",
        libc::O_RDONLY,
        LookupFlags::IN_ROOT,
        libc::EXDEV
    );

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "netbsd"))]
    {
        check_err!("proc", libc::O_RDONLY, libc::EXDEV);
        check_err!("proc/self", libc::O_RDONLY, libc::EXDEV);
    }
}
