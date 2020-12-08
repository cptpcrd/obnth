use std::ffi;
use std::fs;
use std::io;
use std::os::unix::prelude::*;
use std::path::Path;

#[cfg(target_os = "linux")]
pub use libc::__errno_location as errno_ptr;

#[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "macos"))]
pub use libc::__error as errno_ptr;

#[cfg(any(target_os = "android", target_os = "netbsd", target_os = "openbsd"))]
pub use libc::__errno as errno_ptr;

#[inline]
pub fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    let mut stat = unsafe { std::mem::zeroed() };

    if unsafe { libc::fstat(fd, &mut stat) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(stat)
    }
}

#[inline]
pub fn fstatat(fd: RawFd, path: &ffi::CStr, flags: libc::c_int) -> io::Result<libc::stat> {
    let mut stat = unsafe { std::mem::zeroed() };

    if unsafe { libc::fstatat(fd, path.as_ptr(), &mut stat, flags) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(stat)
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
    path: &ffi::CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<RawFd> {
    let fd = unsafe {
        libc::openat(
            dir_fd,
            path.as_ptr(),
            flags | libc::O_CLOEXEC,
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
    path: &ffi::CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<fs::File> {
    Ok(unsafe { fs::File::from_raw_fd(openat_raw(dir_fd, path, flags, mode)?) })
}

pub fn readlinkat(dir_fd: RawFd, path: &ffi::CStr) -> io::Result<ffi::CString> {
    let mut buf = [0u8; libc::PATH_MAX as usize];

    if unsafe {
        libc::readlinkat(
            dir_fd,
            path.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        )
    } < 0
    {
        Err(io::Error::last_os_error())
    } else {
        let len = buf
            .iter()
            .position(|c| *c == 0)
            .unwrap_or_else(|| buf.len());

        // SAFETY: we cut off the portion after the first nul character
        Ok(unsafe { ffi::CString::from_vec_unchecked(buf[..len].into()) })
    }
}

pub fn mkdirat(dir_fd: RawFd, path: &ffi::CStr, mode: libc::mode_t) -> io::Result<()> {
    if unsafe { libc::mkdirat(dir_fd, path.as_ptr(), mode) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn unlinkat(dir_fd: RawFd, path: &ffi::CStr, dir: bool) -> io::Result<()> {
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

pub fn symlinkat(target: &ffi::CStr, dir_fd: RawFd, path: &ffi::CStr) -> io::Result<()> {
    if unsafe { libc::symlinkat(target.as_ptr(), dir_fd, path.as_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[inline]
pub fn get_symloop_max() -> Option<u16> {
    let res = unsafe { libc::sysconf(libc::_SC_SYMLOOP_MAX) };

    if res >= 0 {
        // A C long could easily be larger than a u16, but values that high (>= 2 ** 16!) should
        // never occur in SYMLOOP_MAX.
        Some(res as u16)
    } else {
        None
    }
}

pub fn path_basename(path: &Path) -> Option<&ffi::OsStr> {
    // This is equivalent to path.file_name(), except it leaves trailing slashes in place.

    if path == Path::new("/") || path.ends_with("..") {
        return None;
    }

    // Get a byte array
    let mut bytes = path.as_os_str().as_bytes();

    // Only leave one trailing slash
    while bytes.ends_with(b"//") {
        bytes = &bytes[..bytes.len() - 1];
    }

    // Now find the last trailing slash that isn't at the end
    let start_index = match bytes.iter().take(bytes.len() - 1).rposition(|&c| c == b'/') {
        Some(i) => i + 1,
        None => 0,
    };

    Some(ffi::OsStr::from_bytes(&bytes[start_index..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ebadf_errors() {
        assert_eq!(fstat(-1).unwrap_err().raw_os_error(), Some(libc::EBADF));
        assert_eq!(
            fstatat(-1, &ffi::CStr::from_bytes_with_nul(b"dir\0").unwrap(), 0)
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

        let name = ffi::CStr::from_bytes_with_nul(b"dir\0").unwrap();

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

        let name = ffi::CStr::from_bytes_with_nul(b"file\0").unwrap();

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
    fn test_path_basename() {
        for path in ["/", "//", "..", "a/.."].iter() {
            assert_eq!(path_basename(Path::new(path)), None);
        }

        assert_eq!(path_basename(Path::new("a")), Some(ffi::OsStr::new("a")));
        assert_eq!(path_basename(Path::new("a/")), Some(ffi::OsStr::new("a/")));
        assert_eq!(path_basename(Path::new("a//")), Some(ffi::OsStr::new("a/")));

        assert_eq!(path_basename(Path::new("a/b")), Some(ffi::OsStr::new("b")));
        assert_eq!(
            path_basename(Path::new("a/b/")),
            Some(ffi::OsStr::new("b/"))
        );
        assert_eq!(
            path_basename(Path::new("a/b//")),
            Some(ffi::OsStr::new("b/"))
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
}
