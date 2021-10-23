use std::ffi::{CStr, OsStr, OsString};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "linux", target_os = "dragonfly"))]
pub use libc::__errno_location as errno_ptr;

#[cfg(any(target_os = "freebsd", target_os = "macos"))]
pub use libc::__error as errno_ptr;

#[cfg(any(target_os = "android", target_os = "netbsd", target_os = "openbsd"))]
pub use libc::__errno as errno_ptr;

#[derive(Debug)]
pub struct SymlinkCounter {
    max: u16,
    cur: u16,
}

impl SymlinkCounter {
    #[inline]
    pub fn new() -> Self {
        use core::convert::TryInto;

        Self {
            max: unsafe { libc::sysconf(libc::_SC_SYMLOOP_MAX) }
                .try_into()
                .unwrap_or(crate::constants::DEFAULT_SYMLOOP_MAX),
            cur: 0,
        }
    }

    #[inline]
    pub fn nolinks() -> Self {
        Self { max: 0, cur: 0 }
    }

    #[inline]
    pub fn exhausted(&self) -> bool {
        self.cur >= self.max
    }

    #[inline]
    pub fn advance(&mut self) -> io::Result<()> {
        if self.exhausted() {
            Err(io::Error::from_raw_os_error(libc::ELOOP))
        } else {
            self.cur += 1;
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
#[inline]
pub fn renameat2(
    old_dfd: RawFd,
    old_path: &CStr,
    new_dfd: RawFd,
    new_path: &CStr,
    flags: libc::c_int,
) -> io::Result<()> {
    if unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            old_dfd,
            old_path.as_ptr(),
            new_dfd,
            new_path.as_ptr(),
            flags,
        )
    } < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    let mut stat = MaybeUninit::uninit();

    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { stat.assume_init() })
    }
}

#[inline]
pub fn fstatat(fd: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<libc::stat> {
    let mut stat = MaybeUninit::uninit();

    if unsafe { libc::fstatat(fd, path.as_ptr(), stat.as_mut_ptr(), flags) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { stat.assume_init() })
    }
}

#[inline]
pub fn samestat(st1: &libc::stat, st2: &libc::stat) -> bool {
    st1.st_ino == st2.st_ino && st1.st_dev == st2.st_dev
}

#[inline]
pub fn dup(fd: RawFd) -> io::Result<RawFd> {
    let new_fd = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };

    if new_fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(new_fd)
    }
}

#[inline]
pub fn openat_raw(
    dir_fd: RawFd,
    path: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<RawFd> {
    let fd = unsafe {
        libc::openat(
            dir_fd,
            path.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOCTTY,
            mode as libc::c_uint,
        )
    };

    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

#[inline]
pub fn openat(
    dir_fd: RawFd,
    path: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<fs::File> {
    Ok(unsafe { fs::File::from_raw_fd(openat_raw(dir_fd, path, flags, mode)?) })
}

pub fn readlinkat(dir_fd: RawFd, path: &CStr) -> io::Result<PathBuf> {
    let mut buf = [0u8; libc::PATH_MAX as usize];

    match unsafe {
        libc::readlinkat(
            dir_fd,
            path.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        )
    } {
        -1 => Err(io::Error::last_os_error()),

        len => {
            debug_assert!(len > 0);

            let len = len as usize;

            // POSIX doesn't specify whether or not the returned string is nul-terminated.

            cfg_if::cfg_if! {
                if #[cfg(any(
                    target_os = "linux",
                    target_os = "android",
                    target_os = "freebsd",
                    target_os = "dragonfly",
                    target_os = "openbsd",
                    target_os = "netbsd",
                    target_os = "macos",
                    target_os = "ios",
                ))] {
                    // On these OSes, it won't be.
                    debug_assert_ne!(buf[len - 1], 0);
                } else {
                    // On other OSes, it *might* be. Let's check.
                    let len = if buf[len - 1] == 0 { len - 1 } else { len };
                }
            }

            Ok(PathBuf::from(OsString::from_vec(buf[..len].into())))
        }
    }
}

#[inline]
pub fn mkdirat(dir_fd: RawFd, path: &CStr, mode: libc::mode_t) -> io::Result<()> {
    if unsafe { libc::mkdirat(dir_fd, path.as_ptr(), mode) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn unlinkat(dir_fd: RawFd, path: &CStr, dir: bool) -> io::Result<()> {
    if unsafe {
        libc::unlinkat(
            dir_fd,
            path.as_ptr(),
            if dir { libc::AT_REMOVEDIR } else { 0 },
        )
    } < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn symlinkat(target: &CStr, dir_fd: RawFd, path: &CStr) -> io::Result<()> {
    if unsafe { libc::symlinkat(target.as_ptr(), dir_fd, path.as_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn linkat(
    old_dfd: RawFd,
    old_path: &CStr,
    new_dfd: RawFd,
    new_path: &CStr,
    flags: libc::c_int,
) -> io::Result<()> {
    if unsafe {
        libc::linkat(
            old_dfd,
            old_path.as_ptr(),
            new_dfd,
            new_path.as_ptr(),
            flags,
        )
    } < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn renameat(
    old_dfd: RawFd,
    old_path: &CStr,
    new_dfd: RawFd,
    new_path: &CStr,
) -> io::Result<()> {
    if unsafe { libc::renameat(old_dfd, old_path.as_ptr(), new_dfd, new_path.as_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn open_dot(dir_fd: RawFd, flags: libc::c_int, mode: libc::mode_t) -> io::Result<fs::File> {
    openat(
        dir_fd,
        unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") },
        flags,
        mode,
    )
}

#[inline]
pub fn open_dotdot(dir_fd: RawFd, flags: libc::c_int, mode: libc::mode_t) -> io::Result<fs::File> {
    openat(
        dir_fd,
        unsafe { CStr::from_bytes_with_nul_unchecked(b"..\0") },
        flags,
        mode,
    )
}

pub fn path_split(path: &Path) -> Option<(Option<&OsStr>, &OsStr)> {
    if path == Path::new("/") || path.ends_with("..") {
        return None;
    }

    // Strip trailing slashes and get a byte slice
    let bytes = strip_trailing_slashes(path.as_os_str()).as_bytes();

    // We checked if it was "/" previously, so it shouldn't be empty now
    debug_assert!(!bytes.is_empty());

    // Now find the last slash that isn't at the end
    Some(match bytes.iter().rposition(|&c| c == b'/') {
        Some(i) => (
            Some(OsStr::from_bytes(&bytes[..i + 1])),
            OsStr::from_bytes(&bytes[i + 1..]),
        ),
        None => (None, OsStr::from_bytes(bytes)),
    })
}

pub fn strip_trailing_slashes(mut path: &OsStr) -> &OsStr {
    loop {
        match path.as_bytes().split_last() {
            Some((b'/', rest)) => path = OsStr::from_bytes(rest),
            _ => return path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symlink_counter() {
        let mut links = SymlinkCounter::nolinks();
        assert!(links.exhausted());
        assert_eq!(
            links.advance().unwrap_err().raw_os_error(),
            Some(libc::ELOOP)
        );

        links = SymlinkCounter::new();
        assert!(links.max > 0);
        for _ in 0..links.max {
            assert!(!links.exhausted());
            links.advance().unwrap();
        }

        assert!(links.exhausted());
        assert_eq!(
            links.advance().unwrap_err().raw_os_error(),
            Some(libc::ELOOP)
        );
    }

    #[test]
    fn test_ebadf_errors() {
        assert_eq!(fstat(-1).unwrap_err().raw_os_error(), Some(libc::EBADF));
        assert_eq!(
            fstatat(-1, CStr::from_bytes_with_nul(b"dir\0").unwrap(), 0)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EBADF)
        );
        assert_eq!(dup(-1).unwrap_err().raw_os_error(), Some(libc::EBADF));
    }

    #[test]
    fn test_mkdir_rmdir_at() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir = tmpdir.as_ref();

        let tmpdir_file = fs::File::open(tmpdir).unwrap();
        let tmpdir_fd = tmpdir_file.as_raw_fd();

        let name = CStr::from_bytes_with_nul(b"dir\0").unwrap();

        mkdirat(tmpdir_fd, name, 0o777).unwrap();
        assert_eq!(
            mkdirat(tmpdir_fd, name, 0o777).unwrap_err().raw_os_error(),
            Some(libc::EEXIST)
        );

        // unlinkat() when specifying a directory but without the AT_REMOVEDIR flag can fail with
        // either EISDIR or EPERM
        let eno = unlinkat(tmpdir_fd, name, false)
            .unwrap_err()
            .raw_os_error()
            .unwrap();
        assert!([libc::EISDIR, libc::EPERM].contains(&eno), "{}", eno);

        unlinkat(tmpdir_fd, name, true).unwrap();
        assert_eq!(
            unlinkat(tmpdir_fd, name, true).unwrap_err().raw_os_error(),
            Some(libc::ENOENT)
        );
    }

    #[test]
    fn test_unlinkat_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir = tmpdir.as_ref();

        let tmpdir_file = fs::File::open(tmpdir).unwrap();
        let tmpdir_fd = tmpdir_file.as_raw_fd();

        fs::File::create(tmpdir.join("file")).unwrap();

        let name = CStr::from_bytes_with_nul(b"file\0").unwrap();

        assert_eq!(
            unlinkat(tmpdir_fd, name, true).unwrap_err().raw_os_error(),
            Some(libc::ENOTDIR)
        );
        unlinkat(tmpdir_fd, name, false).unwrap();
        assert_eq!(
            unlinkat(tmpdir_fd, name, false).unwrap_err().raw_os_error(),
            Some(libc::ENOENT)
        );
    }

    #[test]
    fn test_path_split() {
        for path in ["/", "//", "..", "a/.."].iter() {
            assert_eq!(path_split(Path::new(path)), None);
        }

        assert_eq!(path_split(Path::new("a")), Some((None, OsStr::new("a"))));
        assert_eq!(path_split(Path::new("a/")), Some((None, OsStr::new("a"))));
        assert_eq!(path_split(Path::new("a//")), Some((None, OsStr::new("a"))));

        assert_eq!(
            path_split(Path::new("a/b")),
            Some((Some(OsStr::new("a/")), OsStr::new("b")))
        );
        assert_eq!(
            path_split(Path::new("a/b/")),
            Some((Some(OsStr::new("a/")), OsStr::new("b")))
        );
        assert_eq!(
            path_split(Path::new("a/b//")),
            Some((Some(OsStr::new("a/")), OsStr::new("b")))
        );
        assert_eq!(
            path_split(Path::new("/a//b//")),
            Some((Some(OsStr::new("/a//")), OsStr::new("b")))
        );
        assert_eq!(
            path_split(Path::new("/a")),
            Some((Some(OsStr::new("/")), OsStr::new("a")))
        );

        assert_eq!(
            path_split(Path::new("/a/.")),
            Some((Some(OsStr::new("/a/")), OsStr::new(".")))
        );

        assert_eq!(
            path_split(Path::new("/a/b/./")),
            Some((Some(OsStr::new("/a/b/")), OsStr::new(".")))
        );

        assert_eq!(
            path_split(Path::new("a/b/c")),
            Some((Some(OsStr::new("a/b/")), OsStr::new("c")))
        );
        assert_eq!(
            path_split(Path::new("a/b/c//")),
            Some((Some(OsStr::new("a/b/")), OsStr::new("c")))
        );
    }

    #[test]
    fn test_errno_ptr() {
        for eno in [0, libc::EEXIST].iter().copied() {
            unsafe {
                *errno_ptr() = eno;
            }

            assert_eq!(unsafe { *errno_ptr() }, eno);
            assert_eq!(io::Error::last_os_error().raw_os_error().unwrap(), eno);
        }
    }

    #[test]
    fn test_strip_trailing_slashes() {
        assert_eq!(strip_trailing_slashes(OsStr::new("")), OsStr::new(""));
        assert_eq!(strip_trailing_slashes(OsStr::new("/")), OsStr::new(""));
        assert_eq!(strip_trailing_slashes(OsStr::new("//")), OsStr::new(""));

        assert_eq!(strip_trailing_slashes(OsStr::new("a")), OsStr::new("a"));
        assert_eq!(strip_trailing_slashes(OsStr::new("a/")), OsStr::new("a"));
        assert_eq!(strip_trailing_slashes(OsStr::new("a//")), OsStr::new("a"));

        assert_eq!(
            strip_trailing_slashes(OsStr::new("/a/b")),
            OsStr::new("/a/b")
        );
        assert_eq!(
            strip_trailing_slashes(OsStr::new("/a/b/")),
            OsStr::new("/a/b")
        );
        assert_eq!(
            strip_trailing_slashes(OsStr::new("/a/b//")),
            OsStr::new("/a/b")
        );
    }
}
