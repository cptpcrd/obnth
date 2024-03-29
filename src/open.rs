use std::borrow::Cow;
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::os::unix::prelude::*;
use std::path::{Component, Path};

use crate::{constants, util, AsPath};

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

        /// Block traversal of mount points during path resolution.
        ///
        /// On Linux, this includes bind mounts.
        ///
        /// Note that on Linux, if `openat2()` is not available (e.g. on kernels older than 5.6, or
        /// it's blocked by a seccomp rule) then this option may require `/proc` to be mounted to
        /// work reliably.
        const NO_XDEV = 0x04;
    }
}

/// Returns `true` if this OS has an equivalent of the `O_SEARCH` flag, which allows opening files
/// in certain extra cases.
///
/// **Note**: This is an edge case; most users are unlikely to encounter cases where this is
/// relevant. Keep reading if you want more information.
///
/// # Platform support
///
/// Currently, this will only return `true` on the following platforms:
/// - Linux/Android
/// - FreeBSD
///
/// # Unix directory permissions background
///
/// - **Read** permission (`r--`, bit 4 in numeric permissions) on a directory allows you to list
///   the contents of the directory.
/// - **Write** permission (`-w-`, bit 2 in numeric permissions) on a directory allows you to
///   create and delete files within the directory.
/// - **Execute** permission (`--x`, bit 1 in numeric permissions) on a directory allows you to
///   open files within the directory.
///
/// It's important to understand the difference between read and execute permissions:
/// - If you only have read permission, you can list the contents of the directory, but you can't
///   access any of the files inside it.
/// - If you only have execute permission, you can access files within the directory if you know
///   their names, but you can't list the contents to find out the names in the first place.
///
/// In 99% of all cases, you'll have both (`r-x`, 5 in numeric permissions) and this is a
/// non-issue. Occasionally, however (for example, in shared or public directories), the
/// distinction can become important, and you may have one but not the other.
///
/// # What the return value of this function means
///
/// If this function returns `true`, it means that this library is able to open directories
/// internally for **executing/searching only**. That means that if you don't have read permissions
/// on a directory, you can still open files within it using this library.
///
/// If this function returns `false`, then this library can't open directories for
/// executing/searching only, and you have to have both read and execute permissions on a directory
/// to open files inside it with this library.
///
/// **Note**: Usually, this will also apply to subdirectories of the initial "root" directory. For
/// example, if this function returns `false`, something like
/// `Dir::open("/a").unwrap().open_file().open("b/c")` will fail if you don't have read permission
/// on both `/a` **and** `/a/b`.
#[inline]
pub fn has_o_search() -> bool {
    constants::DIR_OPEN_FLAGS != libc::O_DIRECTORY | libc::O_RDONLY
}

/// Open a file beneath the specified directory.
///
/// This is equivalent to `libc::openat(dir_fd, path, flags, mode)` except for the following
/// differences:
///
/// 1. The resolved file is guaranteed to be within the directory referred to by `dir_fd`.
/// 2. The `lookup_flags` argument can further alter behavior during path resolution; see
///    [`LookupFlags`] for more information.
/// 3. The file will be opened with `O_CLOEXEC|O_NOCTTY`, so its close-on-exec flag will be set and
///    it cannot become the process's controlling terminal.
///
/// [`LookupFlags`]: ./struct.LookupFlags.html
///
/// # Errors
///
/// Besides the normal errors that can occur with calling `openat()`, this function will fail with:
///
/// - `ELOOP` if [`LookupFlags::NO_SYMLINKS`] is given and a component of the given `path` is a
///   symbolic link.
/// - `EXDEV` if any of the other conditions required by the given [`LookupFlags`] are not met.
/// - `EAGAIN` if a race condition occurred that prevented safely resolving the path. This usually
///   involves checking for escapes caused by `..` components.
///
///   In this case it may be desirable to retry the call, though if possible it's recommended to
///   limit the number of retries in order to prevent DOSes (intentional or accidental) by other
///   programs.
pub fn open_beneath<P: AsPath>(
    dir_fd: RawFd,
    path: P,
    flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<fs::File> {
    #[cfg(all(feature = "openat2", target_os = "linux"))]
    if let Some(file) =
        path.with_cstr(|s| open_beneath_openat2(dir_fd, s, flags, mode, lookup_flags))?
    {
        return Ok(file);
    }

    // On macOS, if the O_NOFOLLOW_ANY flag is included, translate that to NO_SYMLINKS
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let (flags, lookup_flags) = if flags & crate::sys::O_NOFOLLOW_ANY == crate::sys::O_NOFOLLOW_ANY
    {
        (
            flags & !crate::sys::O_NOFOLLOW_ANY,
            lookup_flags | LookupFlags::NO_SYMLINKS,
        )
    } else {
        (flags, lookup_flags)
    };

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if let Some(file) =
        path.with_cstr(|s| open_beneath_nofollow_any(dir_fd, s, flags, mode, lookup_flags))?
    {
        return Ok(file);
    }

    do_open_beneath(dir_fd, path.as_path(), flags, mode, lookup_flags)
}

#[cfg(all(feature = "openat2", target_os = "linux"))]
fn open_beneath_openat2(
    dir_fd: RawFd,
    path: &CStr,
    mut flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<Option<fs::File>> {
    if dir_fd == libc::AT_FDCWD {
        // An actual directory must be specified
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }

    // Before we go any further, make sure the current kernel supports openat2()
    if !openat2_rs::has_openat2_cached() {
        return Ok(None);
    }

    // If there's a trailing slash, strip it and add in O_DIRECTORY
    let path: Cow<CStr> = match path.to_bytes().split_last() {
        Some((b'/', rest)) if !rest.is_empty() => {
            flags |= libc::O_DIRECTORY;
            Cow::Owned(unsafe { CString::from_vec_unchecked(rest.to_vec()) })
        }
        _ => Cow::Borrowed(path),
    };

    let mut how = openat2_rs::OpenHow::new(flags | libc::O_NOCTTY | libc::O_CLOEXEC, mode as _);
    how.truncate_flags_mode();

    how.resolve |= openat2_rs::ResolveFlags::NO_MAGICLINKS;
    if lookup_flags.contains(LookupFlags::IN_ROOT) {
        how.resolve |= openat2_rs::ResolveFlags::IN_ROOT;
    } else {
        how.resolve |= openat2_rs::ResolveFlags::BENEATH;
    }
    if lookup_flags.contains(LookupFlags::NO_SYMLINKS) {
        how.resolve |= openat2_rs::ResolveFlags::NO_SYMLINKS;
    }
    if lookup_flags.contains(LookupFlags::NO_XDEV) {
        how.resolve |= openat2_rs::ResolveFlags::NO_XDEV;
    }

    match openat2_rs::openat2_cstr(Some(dir_fd), &path, &how) {
        Ok(fd) => Ok(Some(unsafe { fs::File::from_raw_fd(fd) })),
        // E2BIG means an unsupported extension was specified.
        // EAGAIN is returned from openat2() with RESOLVE_BENEATH or RESOLVE_IN_ROOT if any file is
        // renamed on the system. Fall back on the normal method if this happens.
        Err(e) if matches!(e.raw_os_error(), Some(libc::E2BIG) | Some(libc::EAGAIN)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn open_beneath_nofollow_any(
    dir_fd: RawFd,
    mut path: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<Option<fs::File>> {
    // We can only handle NO_SYMLINKS (possibly together with IN_ROOT)
    if lookup_flags & !LookupFlags::IN_ROOT != LookupFlags::NO_SYMLINKS {
        return Ok(None);
    }

    // We cannot handle any ".." components
    if path
        .to_bytes()
        .split(|&c| c == b'/')
        .any(|part| part == b"..")
    {
        return Ok(None);
    }

    use std::sync::atomic::{AtomicU8, Ordering};
    static HAS_NOFOLLOW_ANY: AtomicU8 = AtomicU8::new(2);
    match HAS_NOFOLLOW_ANY.load(Ordering::Relaxed) {
        0 => return Ok(None),
        1 => (),

        _ => {
            // Trying to open() a file with both O_NOFOLLOW_ANY *and* O_NOFOLLOW should fail with
            // EINVAL
            if matches!(
                util::openat(
                    libc::AT_FDCWD,
                    unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") },
                    crate::sys::O_NOFOLLOW_ANY | libc::O_NOFOLLOW | libc::O_RDONLY,
                    0,
                ),
                Err(e) if e.raw_os_error() == Some(libc::EINVAL),
            ) {
                // Supported
                HAS_NOFOLLOW_ANY.store(1, Ordering::Relaxed);
            } else {
                // Not supported
                HAS_NOFOLLOW_ANY.store(0, Ordering::Relaxed);
                return Ok(None);
            }
        }
    }

    if path.to_bytes().first() == Some(&b'/') {
        if !lookup_flags.contains(LookupFlags::IN_ROOT) {
            // Leading slashes are not allowed unless IN_ROOT is specified
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }

        // IN_ROOT was specified, so the leading slashes are OK. However, O_NOFOLLOW_ANY doesn't
        // handle leading slashes the way we do, so we need to strip them.
        if let Some(index) = path.to_bytes().iter().position(|&c| c != b'/') {
            // We found the index of the first non-slash character.
            // Now ignore all the leading slashes.
            path = &path[index..];
        } else {
            // No non-slashes -> the path is entirely slashes
            // Just reopen the directory
            return util::open_dot(dir_fd, flags, 0).map(Some);
        }
    }

    util::openat(dir_fd, path, flags | crate::sys::O_NOFOLLOW_ANY, mode).map(Some)
}

fn map_component_cstring(component: Component) -> io::Result<Cow<CStr>> {
    Ok(match component {
        Component::RootDir => Cow::Borrowed(unsafe { CStr::from_bytes_with_nul_unchecked(b"/\0") }),
        Component::ParentDir => {
            Cow::Borrowed(unsafe { CStr::from_bytes_with_nul_unchecked(b"..\0") })
        }

        Component::Normal(fname) => Cow::Owned(CString::new(fname.as_bytes())?),

        // Filtered out earlier
        Component::CurDir => unreachable!(),

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

    let mut it = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .peekable();
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

    for (i, component) in path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .rev()
        .enumerate()
    {
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

    if queue.is_empty() {
        // We remove CurDir elements when splitting the paths. This has the consequence that if the
        // last element in the path is a symbolic link pointing to ".", nothing will get added to
        // `queue` with the corresponding flags, so they will not be properly honored (just opened
        // with DIR_OPEN_FLAGS).
        // It's an edge case, but it could happen.

        debug_assert_eq!(path, Path::new("."));

        queue.push_front((
            Cow::Borrowed(unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") }),
            flags,
        ));
    }

    Ok(())
}

fn check_beneath(base_fd: RawFd, dir_fd_stat: &libc::stat) -> io::Result<()> {
    // We need to rewind up the directory tree and make sure that we didn't escape because of
    // race conditions with "..".

    let mut prev_stat = unsafe { std::mem::zeroed() };

    let mut cur_file: Option<fs::File> = None;

    loop {
        let cur_fd = cur_file.as_ref().map(|f| f.as_raw_fd()).unwrap_or(base_fd);

        let cur_stat = util::fstat(cur_fd)?;

        if util::samestat(&cur_stat, dir_fd_stat) {
            // We found it! We *didn't* escape.
            return Ok(());
        } else if cur_file.is_some() && util::samestat(&cur_stat, &prev_stat) {
            // Trying to open ".." brought us the same directory. That means we're at "/"
            // (the REAL "/").
            // So we escaped the "beneath" directory.
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }

        prev_stat = cur_stat;

        cur_file = Some(util::open_dotdot(cur_fd, constants::DIR_OPEN_FLAGS, 0)?);
    }
}

fn do_open_beneath(
    dir_fd: RawFd,
    orig_path: &Path,
    orig_flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
) -> io::Result<fs::File> {
    let dir_fd_stat = util::fstat(dir_fd)?;

    if dir_fd == libc::AT_FDCWD {
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }

    if dir_fd_stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
    }

    let dir_mnt_id = if lookup_flags.contains(LookupFlags::NO_XDEV) {
        Some(crate::mntid::identify_mount(dir_fd)?)
    } else {
        None
    };

    let mut parts = split_path(orig_path, orig_flags)?;

    let mut links = if lookup_flags.contains(LookupFlags::NO_SYMLINKS) {
        util::SymlinkCounter::nolinks()
    } else {
        util::SymlinkCounter::new()
    };

    let mut cur_file: Option<fs::File> = None;
    let mut saw_parent_elem = false;

    fn handle_possible_symlink(
        relfd: RawFd,
        relpath: &CStr,
        flags: libc::c_int,
        eno: libc::c_int,
        links: &mut util::SymlinkCounter,
        parts: &mut VecDeque<(Cow<CStr>, libc::c_int)>,
    ) -> io::Result<()> {
        debug_assert!(matches!(eno, libc::ELOOP | libc::ENOTDIR));

        // If we know it's definitely a symlink, and either a) we were given
        // O_NOFOLLOW for this component, or b) we can't resolve any more symlinks,
        // then let's skip the readlinkat() check and return ELOOP directly.
        if eno == libc::ELOOP && (flags & libc::O_NOFOLLOW == libc::O_NOFOLLOW || links.exhausted())
        {
            return Err(io::Error::from_raw_os_error(
                if flags & libc::O_DIRECTORY == libc::O_DIRECTORY && !links.exhausted() {
                    libc::ENOTDIR
                } else {
                    libc::ELOOP
                },
            ));
        }

        let target = match util::readlinkat(relfd, relpath) {
            // Successfully read the symlink
            Ok(t) => t,

            // EINVAL means it's not a symlink
            Err(e2) if e2.raw_os_error() == Some(libc::EINVAL) => {
                return Err(io::Error::from_raw_os_error(if eno == libc::ENOTDIR {
                    // All we knew was that it wasn't a directory, so it's probably another file
                    // type.
                    libc::ENOTDIR
                } else {
                    // We got ELOOP, indicating it *was* a symlink. Then we got EINVAL, indicating
                    // that it *wasn't* a symlink.
                    // This probably means a race condition. Let's pass up EAGAIN.
                    libc::EAGAIN
                }));
            }

            // Pass other errors up to the caller
            Err(e2) => return Err(e2),
        };

        links.advance()?;
        if flags & libc::O_NOFOLLOW == libc::O_NOFOLLOW {
            return Err(io::Error::from_raw_os_error(
                if flags & libc::O_DIRECTORY == libc::O_DIRECTORY {
                    libc::ENOTDIR
                } else {
                    libc::ELOOP
                },
            ));
        }

        split_link_path_into(&target, flags, parts)?;

        Ok(())
    }

    fn check_mnt_id(
        dir_mnt_id: Option<crate::mntid::MountId>,
        prev_fd: libc::c_int,
        new_file: Option<&fs::File>,
    ) -> io::Result<()> {
        if let Some(dir_mnt_id) = dir_mnt_id {
            if let Some(new_file) = new_file.as_ref() {
                if new_file.as_raw_fd() != prev_fd
                    && crate::mntid::identify_mount(new_file.as_raw_fd())? != dir_mnt_id
                {
                    return Err(io::Error::from_raw_os_error(libc::EXDEV));
                }
            }
        }

        Ok(())
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

                // It's impossible for us to see `/` immediately after seeing `..`.
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
                    cur_file = Some(util::open_dotdot(cur_fd, flags, mode)?);

                    saw_parent_elem = true;
                }
            }

            _ => {
                if saw_parent_elem {
                    check_beneath(cur_fd, &dir_fd_stat)?;
                    saw_parent_elem = false;
                }

                match util::openat(cur_fd, &part, flags | libc::O_NOFOLLOW, mode) {
                    Ok(f) => {
                        // On Linux (and FreeBSD 14.0+), O_PATH|O_NOFOLLOW will return a file
                        // descriptor open to the *symlink* (though adding in O_DIRECTORY will
                        // prevent this by only allowing a directory). Since we "add in" O_NOFOLLOW,
                        // if O_PATH was specified and neither O_NOFOLLOW nor O_DIRECTORY was, we
                        // might accidentally open a symlink when that isn't what the user wants.
                        //
                        // So let's check if it's a symlink in that case.

                        #[cfg(any(target_os = "linux", target_os = "android"))]
                        use libc::O_PATH;
                        #[cfg(target_os = "freebsd")]
                        const O_PATH: libc::c_int = 0x00400000;

                        #[cfg(any(
                            target_os = "linux",
                            target_os = "android",
                            target_os = "freebsd",
                        ))]
                        if flags & (O_PATH | libc::O_NOFOLLOW | libc::O_DIRECTORY) == O_PATH
                            && f.metadata()?.file_type().is_symlink()
                        {
                            // It *is* a symlink.

                            // Now that we have this file descriptor open to a symlink, we can pass
                            // *that* to readlinkat() to resolve the symlink.
                            handle_possible_symlink(
                                f.as_raw_fd(),
                                unsafe { CStr::from_bytes_with_nul_unchecked(b"\0") },
                                flags,
                                libc::ELOOP,
                                &mut links,
                                &mut parts,
                            )?;

                            drop(f);
                            // Stay where we are and skip the mount ID check
                            continue;
                        }

                        cur_file = Some(f);
                    }

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
                        // (If eno == libc::ELOOP, it's definitely a symlink.)
                        handle_possible_symlink(cur_fd, &part, flags, eno, &mut links, &mut parts)?;
                    }
                }
            }
        }

        debug_assert_eq!(
            lookup_flags.contains(LookupFlags::NO_XDEV),
            dir_mnt_id.is_some()
        );

        check_mnt_id(dir_mnt_id, cur_fd, cur_file.as_ref())?;
    }

    if saw_parent_elem {
        check_beneath(cur_file.as_ref().unwrap().as_raw_fd(), &dir_fd_stat)?;
    }

    if let Some(cur_file) = cur_file {
        Ok(cur_file)
    } else {
        util::open_dot(dir_fd, orig_flags, mode)
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
            split_path("./abc/./../def/".as_ref(), libc::O_RDONLY).unwrap(),
            &[
                (
                    Cow::Owned(CString::new("abc").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("..").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("def").unwrap()),
                    libc::O_RDONLY | libc::O_DIRECTORY
                )
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

        parts.push_back((Cow::Owned(CString::new("END").unwrap()), 0));
        split_link_path_into("./abc/./def/.".as_ref(), libc::O_RDONLY, &mut parts).unwrap();
        assert_eq!(
            parts,
            &[
                (
                    Cow::Owned(CString::new("abc").unwrap()),
                    constants::DIR_OPEN_FLAGS
                ),
                (
                    Cow::Owned(CString::new("def").unwrap()),
                    libc::O_RDONLY | libc::O_DIRECTORY
                ),
                (Cow::Owned(CString::new("END").unwrap()), 0),
            ]
        );
        parts.clear();
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
