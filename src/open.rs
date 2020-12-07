use std::borrow::Cow;
use std::collections::VecDeque;
use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::io;
use std::os::unix::prelude::*;
use std::path::{Component, Path};

use crate::{constants, sys, util, AsPath};

bitflags::bitflags! {
    /// Flags that modify path loookup when opening a file/directory beneath another directory.
    pub struct LookupFlags: u32 {
        /// Fail if any symlinks are encountered during path resolution.
        const NO_SYMLINKS = 0x01;

        /// Behave as if the process had `chroot()`ed to the directory.
        ///
        /// If this is specified, absolute paths and paths with `..` elements that try to escape the
        /// directory (i.e. `/` or `a/../..`) will stay at the original directory instead of failing
        /// with EXDEV.
        const IN_ROOT = 0x02;
    }
}

/// Open a file beneath the specified directory.
///
/// This is equivalent to `libc::openat(dir_fd, path, flags, mode)` except that the resolved file
/// is guaranteed to be within the directory referred to by `dir_fd` (and the `lookup_flags`
/// argument can further alter behavior; see [`LookupFlags`] for more information).
///
/// [`LookupFlags`]: ./struct.LookupFlags.html
pub fn open_beneath<P: AsPath>(
    dir_fd: RawFd,
    path: P,
    flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<fs::File> {
    if dir_fd == libc::AT_FDCWD {
        // An actual directory must be specified
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }

    #[cfg(all(feature = "openat2", target_os = "linux"))]
    if let Some(file) =
        path.with_cstr(|s| open_beneath_openat2(dir_fd, s, flags, mode, lookup_flags))?
    {
        return Ok(file);
    }

    do_open_beneath(dir_fd, path.as_path(), flags, mode, lookup_flags)
}

#[cfg(all(feature = "openat2", target_os = "linux"))]
fn open_beneath_openat2(
    dir_fd: RawFd,
    path: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<Option<fs::File>> {
    let mut how = sys::open_how {
        flags: (flags | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: sys::ResolveFlags::NO_MAGICLINKS,
    };

    if flags & libc::O_CREAT == libc::O_CREAT || flags & libc::O_TMPFILE == libc::O_TMPFILE {
        how.mode = (mode & 0o777) as u64;
    }

    if lookup_flags.contains(LookupFlags::IN_ROOT) {
        how.resolve |= sys::ResolveFlags::IN_ROOT;
    } else {
        how.resolve |= sys::ResolveFlags::BENEATH;
    }

    if lookup_flags.contains(LookupFlags::NO_SYMLINKS) {
        how.resolve |= sys::ResolveFlags::NO_SYMLINKS;
    }

    let res = unsafe {
        libc::syscall(
            sys::SYS_OPENAT2,
            dir_fd,
            path.as_ptr(),
            &mut how,
            std::mem::size_of::<sys::open_how>(),
        )
    };

    if res >= 0 {
        Ok(Some(unsafe { fs::File::from_raw_fd(res as RawFd) }))
    } else {
        match unsafe { *libc::__errno_location() } {
            // ENOSYS obviously means we're on a kernel that doesn't have openat2().
            // E2BIG means an unsupported extension was specified.
            //
            // EPERM *could* mean that the file is sealed (from open(2)). However, there's another
            // (more likely?) possibility: Sometimes seccomp filters block all syscalls (and return
            // EPERM) by default, then only allow a carefully audited list of syscalls. If the
            // seccomp filter doesn't include openat2() (or the current libseccomp isn't aware of
            // it), then we might get EPERM when we *really* should be getting ENOSYS. So let's
            // fall back on the traditional technique in that case.
            libc::ENOSYS | libc::E2BIG | libc::EPERM => Ok(None),

            eno => Err(io::Error::from_raw_os_error(eno)),
        }
    }
}

fn map_component_cstring(component: Component) -> io::Result<Cow<CStr>> {
    Ok(match component {
        Component::CurDir => Cow::Borrowed(unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") }),
        Component::RootDir => Cow::Borrowed(unsafe { CStr::from_bytes_with_nul_unchecked(b"/\0") }),
        Component::ParentDir => {
            Cow::Borrowed(unsafe { CStr::from_bytes_with_nul_unchecked(b"..\0") })
        }

        Component::Normal(fname) => Cow::Owned(CString::new(fname.as_bytes())?),

        // This is a Unix-only crate
        Component::Prefix(_) => unreachable!(),
    })
}

fn split_path(
    path: &Path,
    mut flags: libc::c_int,
) -> io::Result<VecDeque<(Cow<CStr>, libc::c_int)>> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }

    if path.as_os_str().as_bytes().ends_with(b"/") || path.as_os_str().as_bytes().ends_with(b"/.") {
        flags |= libc::O_DIRECTORY;
    }

    let mut queue = VecDeque::new();

    let mut it = path.components().peekable();
    while let Some(component) = it.next() {
        let component_flags = if it.peek().is_some() {
            constants::DIR_OPEN_FLAGS
        } else {
            flags
        };

        queue.push_back((map_component_cstring(component)?, component_flags));
    }

    Ok(queue)
}

fn split_link_path_into(
    path: &Path,
    mut flags: libc::c_int,
    queue: &mut VecDeque<(Cow<CStr>, libc::c_int)>,
) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }

    if path.as_os_str().as_bytes().ends_with(b"/") || path.as_os_str().as_bytes().ends_with(b"/.") {
        flags |= libc::O_DIRECTORY;
    }

    for (i, component) in path.components().rev().enumerate() {
        let component_flags = if i == 0 {
            flags
        } else {
            constants::DIR_OPEN_FLAGS
        };

        // We need a CString because the given `path` might go out of scope before this is used
        // again
        let component = Cow::Owned(map_component_cstring(component)?.into_owned());

        queue.push_front((component, component_flags));
    }

    Ok(())
}

fn check_beneath(base_fd: RawFd, dir_fd_stat: &libc::stat) -> io::Result<()> {
    // We need to rewind up the directory tree and make sure that we didn't escape because of
    // race conditions with "..".

    let mut prev_stat = util::fstat(base_fd)?;

    let mut cur_file: Option<fs::File> = None;

    loop {
        let cur_fd = cur_file.as_ref().map(|f| f.as_raw_fd()).unwrap_or(base_fd);

        let cur_stat = util::fstat(cur_fd)?;

        if util::samestat(&cur_stat, dir_fd_stat) {
            // We found it! We *didn't* escape.
            return Ok(());
        } else if cur_fd != base_fd && util::samestat(&cur_stat, &prev_stat) {
            // Trying to open ".." brought us the same directory. That means we're at "/"
            // (the REAL "/").
            // So we escaped the "beneath" directory.
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }

        prev_stat = cur_stat;

        cur_file = Some(util::openat(
            cur_fd,
            unsafe { CStr::from_bytes_with_nul_unchecked(b"..\0") },
            constants::DIR_OPEN_FLAGS,
            0,
        )?);
    }
}

fn do_open_beneath(
    dir_fd: RawFd,
    orig_path: &Path,
    orig_flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<fs::File> {
    debug_assert_ne!(dir_fd, libc::AT_FDCWD);

    let dir_fd_stat = util::fstat(dir_fd)?;

    if dir_fd_stat.st_mode & libc::S_IFDIR != libc::S_IFDIR {
        return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
    }

    let mut parts = split_path(orig_path, orig_flags)?;

    let max_symlinks = if lookup_flags.contains(LookupFlags::NO_SYMLINKS) {
        0
    } else {
        util::get_symloop_max().unwrap_or(constants::DEFAULT_SYMLOOP_MAX)
    };

    let mut found_symlinks = 0;

    let mut cur_file: Option<fs::File> = None;
    let mut saw_parent_elem = false;

    #[inline]
    fn open_part(
        dir_fd: RawFd,
        path: &CStr,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> io::Result<fs::File> {
        let file = util::openat(dir_fd, path, flags | libc::O_NOFOLLOW, mode)?;

        #[cfg(target_os = "linux")]
        if flags & (libc::O_PATH | libc::O_NOFOLLOW | libc::O_DIRECTORY) == libc::O_PATH {
            // On Linux, O_PATH|O_NOFOLLOW will return a file descriptor open to the *symlink*
            // (though adding in O_DIRECTORY will prevent this by only allowing a directory). Since
            // we "add in" O_NOFOLLOW, if O_PATH was specified and neither O_NOFOLLOW nor
            // O_DIRECTORY was, we might accidentally open a symlink when that isn't what the user
            // wants.
            //
            // So let's check if it's a symlink in that case.

            if file.metadata()?.file_type().is_symlink() {
                return Err(io::Error::from_raw_os_error(libc::ELOOP));
            }
        }

        Ok(file)
    }

    while let Some((part, flags)) = parts.pop_front() {
        // Sanity check -- `flags` can only ever be something other than DIR_OPEN_FLAGS if there
        // are no components left
        debug_assert!(flags == constants::DIR_OPEN_FLAGS || parts.is_empty());

        let cur_fd = cur_file.as_ref().map(|f| f.as_raw_fd()).unwrap_or(dir_fd);

        match part.to_bytes() {
            b"/" => {
                if !lookup_flags.contains(LookupFlags::IN_ROOT) {
                    return Err(io::Error::from_raw_os_error(libc::EXDEV));
                }

                cur_file = None;
                debug_assert!(!saw_parent_elem);
            }

            b".." => {
                if cur_file.is_none() || util::samestat(&util::fstat(cur_fd)?, &dir_fd_stat) {
                    if !lookup_flags.contains(LookupFlags::IN_ROOT) {
                        return Err(io::Error::from_raw_os_error(libc::EXDEV));
                    }

                    cur_file = None;
                    saw_parent_elem = false;
                } else {
                    cur_file = Some(util::openat(
                        cur_fd,
                        unsafe { CStr::from_bytes_with_nul_unchecked(b"..\0") },
                        flags,
                        mode,
                    )?);

                    saw_parent_elem = true;
                }
            }

            b"." => debug_assert!(flags == constants::DIR_OPEN_FLAGS || cur_file.is_none()),

            _ => {
                if saw_parent_elem {
                    check_beneath(cur_fd, &dir_fd_stat)?;
                    saw_parent_elem = false;
                }

                match open_part(cur_fd, &part, flags, mode) {
                    Ok(f) => cur_file = Some(f),

                    Err(e) => {
                        // When flags=O_DIRECTORY|O_NOFLLOW, if the last component is a symlink then
                        // it will fail with ENOTDIR.
                        //
                        // Otherwise, when the last component is a symlink, most OSes return ELOOP.
                        // However, FreeBSD returns EMLINK and NetBSD returns EFTYPE.

                        let eno = e.raw_os_error().unwrap();

                        #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
                        let eno = if eno == libc::EMLINK {
                            libc::ELOOP
                        } else {
                            eno
                        };

                        #[cfg(target_os = "netbsd")]
                        let eno = if eno == libc::EFTYPE {
                            libc::ELOOP
                        } else {
                            eno
                        };

                        if eno != libc::ELOOP && eno != libc::ENOTDIR {
                            return Err(e);
                        }

                        // It may have failed because it's a symlink.
                        // (If ex.errno != errno.ENOTDIR, it's definitely a symlink.)

                        let target = match util::readlinkat(cur_fd, &part) {
                            // Successfully read the symlink
                            Ok(t) => t,

                            // EINVAL means it's not a symlink
                            Err(e2) if e2.raw_os_error() == Some(libc::EINVAL) => {
                                return Err(if eno == libc::ENOTDIR {
                                    // All we knew was that it wasn't a directory, so it's probably
                                    // another file type.
                                    e
                                } else {
                                    // We got ELOOP, indicating it *was* a symlink. Then we got EINVAL,
                                    // indicating that it *wasn't* a symlink.
                                    // This probably means a race condition. Let's pass up EAGAIN.
                                    io::Error::from_raw_os_error(libc::EAGAIN)
                                });
                            }

                            // Pass other errors up to the caller
                            Err(e2) => return Err(e2),
                        };

                        found_symlinks += 1;
                        if flags & libc::O_NOFOLLOW == libc::O_NOFOLLOW
                            || found_symlinks > max_symlinks
                        {
                            return Err(io::Error::from_raw_os_error(libc::ELOOP));
                        }

                        split_link_path_into(
                            Path::new(OsStr::from_bytes(target.to_bytes())),
                            flags,
                            &mut parts,
                        )?;
                    }
                }
            }
        }
    }

    if saw_parent_elem {
        check_beneath(cur_file.as_ref().unwrap().as_raw_fd(), &dir_fd_stat)?;
    }

    if let Some(cur_file) = cur_file {
        Ok(cur_file)
    } else {
        util::openat(
            dir_fd,
            unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") },
            orig_flags,
            mode,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_path() {
        assert_eq!(
            split_path("abc".as_ref(), libc::O_RDONLY).unwrap(),
            &[(Cow::Owned(CString::new("abc").unwrap()), libc::O_RDONLY)]
        );

        assert_eq!(
            split_path("abc/def".as_ref(), libc::O_RDONLY).unwrap(),
            &[
                (
                    Cow::Owned(CString::new("abc").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (Cow::Owned(CString::new("def").unwrap()), libc::O_RDONLY)
            ]
        );

        assert_eq!(
            split_path("/abc/./../def".as_ref(), libc::O_RDONLY).unwrap(),
            &[
                (
                    Cow::Owned(CString::new("/").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("abc").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("..").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (Cow::Owned(CString::new("def").unwrap()), libc::O_RDONLY)
            ]
        );

        assert_eq!(
            split_path("".as_ref(), libc::O_RDONLY)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );
    }

    #[test]
    fn test_split_link_path_into() {
        let mut parts = VecDeque::new();

        parts.push_back((Cow::Owned(CString::new("END").unwrap()), 0));
        split_link_path_into("abc/def".as_ref(), libc::O_RDONLY, &mut parts).unwrap();
        assert_eq!(
            parts,
            &[
                (
                    Cow::Owned(CString::new("abc").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (Cow::Owned(CString::new("def").unwrap()), libc::O_RDONLY),
                (Cow::Owned(CString::new("END").unwrap()), 0),
            ]
        );
        parts.clear();

        parts.push_back((Cow::Owned(CString::new("END").unwrap()), 0));
        split_link_path_into("/abc/./../def".as_ref(), libc::O_RDONLY, &mut parts).unwrap();
        assert_eq!(
            parts,
            &[
                (
                    Cow::Owned(CString::new("/").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("abc").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("..").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (Cow::Owned(CString::new("def").unwrap()), libc::O_RDONLY),
                (Cow::Owned(CString::new("END").unwrap()), 0),
            ]
        );
        parts.clear();

        assert_eq!(
            split_link_path_into("".as_ref(), libc::O_RDONLY, &mut parts)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );
        assert!(parts.is_empty());
    }

    #[test]
    fn test_check_beneath() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir = tmpdir.as_ref();

        let tmpdir_file = fs::File::open(tmpdir).unwrap();
        let tmpdir_fd = tmpdir_file.as_raw_fd();

        check_beneath(tmpdir_fd, &util::fstat(tmpdir_fd).unwrap()).unwrap();

        assert_eq!(
            check_beneath(
                fs::File::open("/").unwrap().as_raw_fd(),
                &util::fstat(tmpdir_fd).unwrap()
            )
            .unwrap_err()
            .raw_os_error(),
            Some(libc::EAGAIN)
        );
    }
}
