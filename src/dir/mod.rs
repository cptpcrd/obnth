use std::collections::VecDeque;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};

use crate::{constants, open_beneath, util, AsPath, LookupFlags};

mod file_meta;
mod iter;
mod open_opts;

pub use file_meta::{FileType, Metadata};
pub use iter::{Entry, ReadDirIter, SeekPos};
pub use open_opts::OpenOptions;

#[cfg(target_os = "linux")]
bitflags::bitflags! {
    /// Linux-specific: Flags for [`rename2()`].
    ///
    /// If any of these flags are not supported by the filesystem, [`rename2()`] will fail with
    /// `EINVAL`.
    ///
    /// [`rename2()`]: ./fn.rename2.html
    pub struct Rename2Flags: libc::c_int {
        /// Rename the file without replacing the "new" file if it exists (fail with `EEXIST` in
        /// that case).
        ///
        /// This requires support from the underlying filesystem; older kernels do not support this
        /// for a number of filesystems. See renameat2(2) for more details.
        const NOREPLACE = libc::RENAME_NOREPLACE;
        /// Atomically exchange the "old" and "new" files.
        const EXCHANGE = libc::RENAME_EXCHANGE;
        /// Create a "whiteout" object at the source of the rename while performing the rename.
        /// Useful for overlay/union filesystems.
        ///
        /// Added in Linux 3.18. Requires CAP_MKNOD.
        const WHITEOUT = libc::RENAME_WHITEOUT;
    }
}

#[inline]
fn cstr(s: &OsStr) -> io::Result<CString> {
    Ok(CString::new(s.as_bytes())?)
}

/// A wrapper around a directory file descriptor that allows opening files within that directory.
#[derive(Debug)]
pub struct Dir {
    fd: RawFd,
}

impl Dir {
    /// Open the specified directory.
    pub fn open<P: AsPath>(path: P) -> io::Result<Self> {
        path.with_cstr(|s| {
            Ok(Self {
                fd: util::openat_raw(libc::AT_FDCWD, &s, constants::DIR_OPEN_FLAGS, 0)?,
            })
        })
    }

    #[inline]
    fn reopen_raw(&self, flags: libc::c_int) -> io::Result<RawFd> {
        util::openat_raw(
            self.fd,
            unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") },
            flags,
            0,
        )
    }

    /// Open the parent directory of this directory, without checking if it's open to the root
    /// directory.
    ///
    /// If this directory is open to the root directory, this method will return a new directory
    /// open to the same directory (similarly to [`try_clone()`]).
    ///
    /// [`try_clone()`]: #method.try_clone
    #[inline]
    pub fn parent_unchecked(&self) -> io::Result<Self> {
        Ok(Self {
            fd: util::openat_raw(
                self.fd,
                &unsafe { CStr::from_bytes_with_nul_unchecked(b"..\0") },
                constants::DIR_OPEN_FLAGS,
                0,
            )?,
        })
    }

    /// Open the parent directory of this directory.
    ///
    /// This returns `Ok(None)` if this directory is open to the root directory.
    pub fn parent(&self) -> io::Result<Option<Self>> {
        let parent = self.parent_unchecked()?;

        if util::samestat(&util::fstat(self.fd)?, &util::fstat(parent.fd)?) {
            Ok(None)
        } else {
            Ok(Some(parent))
        }
    }

    /// Open a subdirectory of this directory.
    ///
    /// `path` or one of its components can refer to a symlink (unless `LookupFlags::NO_SYMLINKS`
    /// is passed), but the specified subdirectory must be contained within this directory.
    pub fn sub_dir<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<Self> {
        Ok(Self {
            fd: open_beneath(self.fd, path, constants::DIR_OPEN_FLAGS, 0, lookup_flags)?
                .into_raw_fd(),
        })
    }

    /// Create a directory within this directory.
    pub fn create_dir<P: AsPath>(
        &self,
        path: P,
        mode: libc::mode_t,
        lookup_flags: LookupFlags,
    ) -> io::Result<()> {
        let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

        if let Some(fname) = fname {
            let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

            let fname = crate::util::strip_trailing_slashes(fname);

            util::mkdirat(fd, &cstr(fname)?, mode)
        } else {
            Err(io::Error::from_raw_os_error(libc::EEXIST))
        }
    }

    /// Remove a subdirectory of this directory.
    pub fn remove_dir<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<()> {
        let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

        if let Some(fname) = fname {
            let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

            let fname = crate::util::strip_trailing_slashes(fname);

            match util::unlinkat(fd, &cstr(fname)?, true) {
                Err(e) => {
                    #[cfg(not(any(
                        target_os = "linux",
                        target_os = "android",
                        target_os = "freebsd",
                        target_os = "dragonfly",
                        target_os = "openbsd",
                        target_os = "netbsd",
                        target_os = "macos",
                        target_os = "ios"
                    )))]
                    if e.raw_os_error() == Some(libc::EEXIST) {
                        return Err(io::Error::from_raw_os_error(libc::ENOTEMPTY));
                    }

                    Err(e)
                }
                Ok(()) => Ok(()),
            }
        } else {
            Err(std::io::Error::from_raw_os_error(libc::EBUSY))
        }
    }

    /// Remove a file within this directory.
    pub fn remove_file<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<()> {
        let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

        if let Some(fname) = fname {
            let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

            let fname = crate::util::strip_trailing_slashes(fname);

            util::unlinkat(fd, &cstr(fname)?, false)
        } else {
            Err(io::Error::from_raw_os_error(libc::EISDIR))
        }
    }

    /// Create a symlink within this directory.
    ///
    /// `path` specifies the path where the symlink is created, and `target` specifies the file
    /// that the symlink will point to. Note that the order is swapped compared to the C `symlink()`
    /// function (and Rust's `std::os::unix::fs::symlink()`).
    pub fn symlink<P: AsPath, T: AsPath>(
        &self,
        path: P,
        target: T,
        lookup_flags: LookupFlags,
    ) -> io::Result<()> {
        let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

        if let Some(fname) = fname {
            let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

            let fname = crate::util::strip_trailing_slashes(fname);

            target.with_cstr(|target| util::symlinkat(target, fd, &cstr(fname)?))
        } else {
            Err(io::Error::from_raw_os_error(libc::EEXIST))
        }
    }

    /// Read the contents of the specified symlink.
    pub fn read_link<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<PathBuf> {
        cfg_if::cfg_if! {
            if #[cfg(all(target_os = "linux", feature = "openat2"))] {
                // On Linux, we can actually get a file descriptor to the *symlink*, then
                // readlink() that. However, if we don't have openat2() then this costs an extra
                // syscall, so let's only do it if the `openat2` feature is enabled.

                let file = open_beneath(
                    self.fd,
                    path,
                    libc::O_PATH | libc::O_NOFOLLOW,
                    0,
                    lookup_flags,
                )?;

                match util::readlinkat(file.as_raw_fd(), unsafe {
                    CStr::from_bytes_with_nul_unchecked(b"\0".as_ref())
                }) {
                    Ok(target) => Ok(target),

                    // This error means we got a file descriptor that doesn't point to a symlink
                    Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                        Err(io::Error::from_raw_os_error(libc::EINVAL))
                    }

                    Err(e) => Err(e),
                }
            } else {
                // On other OSes (or without openat2()), we have to split the path and perform a
                // few more allocations.

                let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

                if let Some(fname) = fname {
                    let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

                    let fname = crate::util::strip_trailing_slashes(fname);

                    util::readlinkat(fd, &cstr(fname)?)
                } else {
                    Err(io::Error::from_raw_os_error(libc::EINVAL))
                }
            }
        }
    }

    /// Rename a file in this directory.
    ///
    /// This is exactly equivalent to `rename(self, old, self, new, lookup_flags)`.
    pub fn local_rename<P: AsPath, R: AsPath>(
        &self,
        old: P,
        new: R,
        lookup_flags: LookupFlags,
    ) -> io::Result<()> {
        rename(self, old, self, new, lookup_flags)
    }

    /// List the contents of this directory.
    pub fn list_self(&self) -> io::Result<ReadDirIter> {
        ReadDirIter::new_consume(self.reopen_raw(libc::O_DIRECTORY | libc::O_RDONLY)?)
    }

    /// List the contents of the specified subdirectory.
    ///
    /// This is equivalent to `self.sub_dir(path, lookup_flags)?.list_self()`, but more efficient.
    pub fn list_dir<P: AsPath>(
        &self,
        path: P,
        lookup_flags: LookupFlags,
    ) -> io::Result<ReadDirIter> {
        ReadDirIter::new_consume(
            open_beneath(
                self.fd,
                path,
                libc::O_DIRECTORY | libc::O_RDONLY,
                0,
                lookup_flags,
            )?
            .into_raw_fd(),
        )
    }

    /// Try to "clone" this `Dir`.
    ///
    /// This is equivalent to `self.sub_dir(".")`, but more efficient.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: util::dup(self.fd)?,
        })
    }

    /// Retrieve metadata of this directory.
    ///
    /// This is equivalent to `self.metadata(".", LookupFlags::empty())`, but it's significantly
    /// more efficient.
    pub fn self_metadata(&self) -> io::Result<Metadata> {
        util::fstat(self.fd).map(Metadata::new)
    }

    /// Retrieve information on the file with the given path.
    ///
    /// The specified file must be located within this directory. Symlinks in the final component
    /// of the path are not followed.
    pub fn metadata<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<Metadata> {
        let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

        let subdir = subdir.as_ref().unwrap_or(self);

        if let Some(fname) = fname {
            let fname = crate::util::strip_trailing_slashes(fname);

            fname.with_cstr(|s| {
                util::fstatat(subdir.as_raw_fd(), s, libc::AT_SYMLINK_NOFOLLOW).map(Metadata::new)
            })
        } else {
            subdir.self_metadata()
        }
    }

    /// Recover the path to the directory that this `Dir` is currently open to.
    ///
    /// **WARNINGS (make sure to read)**:
    /// - **Do NOT** use this path to open any files within the directory (i.e.
    ///   `File::open(path.join("file.txt"))`! That would defeat the entire purpose of this crate
    ///   by opening vectors for symlink attacks.
    /// - If a potentially malicious user controls a parent directory of the directory that this
    ///   `Dir` is currently open to, the path returned by this function is NOT safe to use.
    ///
    /// OS-specific optimizations:
    /// - On Linux, this will try `readlink("/proc/self/fd/$fd")`.
    /// - On macOS, this will try `fcntl(fd, F_GETPATH)`.
    ///
    /// If either of these techniques fails (or on other platforms), it will fall back on a more
    /// reliable (but slower) strategy.
    ///
    /// Some notes:
    /// - If the directory has been deleted, this function will fail with `ENOENT` (this is
    ///   guaranteed, though beware of race conditions).
    /// - This function may or may not fail with `EACCES` if the currrent process does not have
    ///   access to one or more of this directory's parent directories.
    pub fn recover_path(&self) -> io::Result<PathBuf> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Ok(path) = std::fs::read_link(format!("/proc/self/fd/{}", self.fd)) {
            let path_bytes = path.as_os_str().as_bytes();

            if path_bytes.starts_with(b"/") && !path_bytes.ends_with(b" (deleted)") {
                return Ok(path);
            }
        }

        let self_meta = self.self_metadata()?;

        #[cfg(target_os = "macos")]
        {
            let mut buf = [0u8; libc::PATH_MAX as usize];

            if unsafe { libc::fcntl(self.fd, libc::F_GETPATH, buf.as_mut_ptr()) } == 0 {
                let index = buf.iter().position(|&c| c == 0).unwrap();

                let c_path = unsafe { CStr::from_bytes_with_nul_unchecked(&buf[..index + 1]) };

                // F_GETPATH will return the old path if the directory is deleted, so let's make
                // sure it exists (and while we're at it, let's check that it's the same file using
                // samestat()).

                if let Ok(path_stat) = util::fstatat(libc::AT_FDCWD, c_path, 0) {
                    if util::samestat(&self_meta.stat(), &path_stat) {
                        return Ok(PathBuf::from(OsStr::from_bytes(&buf[..index])));
                    }
                }
            }
        }

        #[inline]
        fn recover_entry(parent: &Dir, sub_meta: &Metadata) -> io::Result<Entry> {
            for entry in parent.list_self()? {
                let entry = entry?;

                // Only check directories (or files with unknown types)
                match entry.file_type() {
                    Some(FileType::Directory) | None => {
                        // stat() the entry and see if it matches.
                        //
                        // We can't check entry.ino() to avoid stat() because that doesn't work
                        // when you cross filesystem boundaries.
                        if let Ok(entry_meta) = entry.metadata() {
                            if same_meta(sub_meta, &entry_meta) {
                                return Ok(entry);
                            }
                        }
                    }

                    _ => (),
                }
            }

            Err(io::Error::from_raw_os_error(libc::ENOENT))
        }

        let mut res = VecDeque::new();

        let mut sub_meta = self_meta;
        let mut parent = self.parent_unchecked()?;

        loop {
            let parent_meta = parent.self_metadata()?;

            if same_meta(&sub_meta, &parent_meta) {
                // Rewinding with ".." didn't move us; we must have hit the root

                if res.is_empty() {
                    res.push_front(b'/');
                }

                return Ok(PathBuf::from(OsString::from_vec(res.into())));
            }

            let entry = recover_entry(&parent, &sub_meta)?;
            let entry_name = entry.name();

            res.reserve(entry_name.len() + 1);

            for ch in entry_name.as_bytes().iter().rev().copied() {
                res.push_front(ch);
            }
            res.push_front(b'/');

            parent = parent.parent_unchecked()?;
            sub_meta = parent_meta;
        }
    }

    /// Set this process's current working directory to this directory.
    ///
    /// This is roughly equivalent to `std::env::set_current_dir(self.recover_path()?)`, but 1) it
    /// is **much** more efficient, and 2) it is more secure (notably, it avoids race conditions).
    pub fn change_cwd_to(&self) -> io::Result<()> {
        if unsafe { libc::fchdir(self.fd) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Return an `OpenOptions` struct that can be use to open files within this directory.
    ///
    /// See the documentation of [`OpenOptions`] for more details.
    ///
    /// [`OpenOptions`]: ./struct.OpenOptions.html
    #[inline]
    pub fn open_file(&self) -> OpenOptions {
        OpenOptions::beneath(self)
    }
}

impl Drop for Dir {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

impl AsRawFd for Dir {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl IntoRawFd for Dir {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        let fd = self.fd;
        std::mem::forget(self);
        fd
    }
}

impl FromRawFd for Dir {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self { fd }
    }
}

/// Create a hardlink to a file in (possibly) a different directory.
pub fn hardlink<P, R>(
    old_dir: &Dir,
    old_path: P,
    new_dir: &Dir,
    new_path: R,
    lookup_flags: LookupFlags,
) -> io::Result<()>
where
    P: AsPath,
    R: AsPath,
{
    let (old_subdir, old_fname) =
        prepare_inner_operation(old_dir, old_path.as_path(), lookup_flags)?;

    let old_fname = if let Some(old_fname) = old_fname {
        crate::util::strip_trailing_slashes(old_fname)
    } else {
        // Assume we can't create hardlinks to directories (it seems that macOS *can*, but it's
        // hacky)
        return Err(std::io::Error::from_raw_os_error(libc::EPERM));
    };

    let (new_subdir, new_fname) =
        prepare_inner_operation(new_dir, new_path.as_path(), lookup_flags)?;

    let old_subdir = old_subdir.as_ref().unwrap_or(old_dir);
    let new_subdir = new_subdir.as_ref().unwrap_or(new_dir);

    if let Some(new_fname) = new_fname {
        let new_fname = crate::util::strip_trailing_slashes(new_fname);

        old_fname.with_cstr(|old_fname| {
            new_fname.with_cstr(|new_fname| {
                util::linkat(
                    old_subdir.as_raw_fd(),
                    old_fname,
                    new_subdir.as_raw_fd(),
                    new_fname,
                    0,
                )
            })
        })
    } else {
        // The "new" path cannot exist already
        Err(std::io::Error::from_raw_os_error(libc::EEXIST))
    }
}

/// Rename a file across directories.
pub fn rename<P, R>(
    old_dir: &Dir,
    old_path: P,
    new_dir: &Dir,
    new_path: R,
    lookup_flags: LookupFlags,
) -> io::Result<()>
where
    P: AsPath,
    R: AsPath,
{
    let (old_subdir, old_fname) =
        prepare_inner_operation(old_dir, old_path.as_path(), lookup_flags)?;
    let old_subdir = old_subdir.as_ref().unwrap_or(old_dir);

    let old_fname = if let Some(old_fname) = old_fname {
        crate::util::strip_trailing_slashes(old_fname)
    } else {
        return Err(std::io::Error::from_raw_os_error(libc::EBUSY));
    };

    let (new_subdir, new_fname) =
        prepare_inner_operation(new_dir, new_path.as_path(), lookup_flags)?;
    let new_subdir = new_subdir.as_ref().unwrap_or(new_dir);

    if let Some(new_fname) = new_fname {
        let new_fname = crate::util::strip_trailing_slashes(new_fname);

        old_fname.with_cstr(|old_fname| {
            new_fname.with_cstr(|new_fname| {
                util::renameat(
                    old_subdir.as_raw_fd(),
                    old_fname,
                    new_subdir.as_raw_fd(),
                    new_fname,
                )
            })
        })
    } else {
        Err(std::io::Error::from_raw_os_error(libc::EBUSY))
    }
}

/// Linux-specific: Rename a file across directories, specifying extra flags to modify behavior.
///
/// This calls the `renameat2()` syscall, which was added in Linux 3.15. It will fail with `ENOSYS`
/// on older kernels, and it will fail with `EINVAL` if any of the given `flags` are not supported
/// by the filesystem. See renameat2(2) for more details.
///
/// Otherwise, the semantics of this are identical to [`rename()`].
///
/// [`rename()`]: ./fn.rename.html
#[cfg(target_os = "linux")]
pub fn rename2<P, R>(
    old_dir: &Dir,
    old_path: P,
    new_dir: &Dir,
    new_path: R,
    flags: Rename2Flags,
    lookup_flags: LookupFlags,
) -> io::Result<()>
where
    P: AsPath,
    R: AsPath,
{
    let (old_subdir, old_fname) =
        prepare_inner_operation(old_dir, old_path.as_path(), lookup_flags)?;
    let old_subdir = old_subdir.as_ref().unwrap_or(old_dir);

    let old_fname = if let Some(old_fname) = old_fname {
        crate::util::strip_trailing_slashes(old_fname)
    } else {
        return Err(std::io::Error::from_raw_os_error(libc::EBUSY));
    };

    let (new_subdir, new_fname) =
        prepare_inner_operation(new_dir, new_path.as_path(), lookup_flags)?;
    let new_subdir = new_subdir.as_ref().unwrap_or(new_dir);

    if let Some(new_fname) = new_fname {
        let new_fname = crate::util::strip_trailing_slashes(new_fname);

        old_fname.with_cstr(|old_fname| {
            new_fname.with_cstr(|new_fname| {
                util::renameat2(
                    old_subdir.as_raw_fd(),
                    old_fname,
                    new_subdir.as_raw_fd(),
                    new_fname,
                    flags.bits,
                )
            })
        })
    } else {
        Err(std::io::Error::from_raw_os_error(libc::EBUSY))
    }
}

#[inline]
fn same_meta(a: &Metadata, b: &Metadata) -> bool {
    util::samestat(a.stat(), b.stat())
}

fn prepare_inner_operation<'a>(
    dir: &Dir,
    mut path: &'a Path,
    lookup_flags: LookupFlags,
) -> io::Result<(Option<Dir>, Option<&'a OsStr>)> {
    match path.strip_prefix("/") {
        Ok(p) => {
            // If we didn't get the IN_ROOT flag, then a path starting with "/" is disallowed.
            if !lookup_flags.contains(LookupFlags::IN_ROOT) {
                return Err(io::Error::from_raw_os_error(libc::EXDEV));
            }

            // Trim the "/" prefix
            path = p;

            if path.as_os_str().is_empty() {
                // Just "/"
                return Ok((None, None));
            }
        }

        // Not an absolute path
        Err(_) => {
            if path.as_os_str().is_empty() {
                // Empty path -> ENOENT
                return Err(io::Error::from_raw_os_error(libc::ENOENT));
            }
        }
    }

    debug_assert!(!path.as_os_str().as_bytes().is_empty());
    debug_assert!(!path.as_os_str().as_bytes().starts_with(b"/"));

    if let Some((parent, fname)) = util::path_split(path) {
        debug_assert!(!path.ends_with(".."));

        Ok((
            if let Some(parent) = parent {
                Some(dir.sub_dir(parent, lookup_flags)?)
            } else {
                None
            },
            match fname.as_bytes() {
                b"." | b"./" => None,
                _ => Some(fname),
            },
        ))
    } else {
        debug_assert!(path.ends_with(".."));

        // So this is a path like "a/b/..". We can't really get a (containing directory, filename)
        // pair out of this.

        Ok((Some(dir.sub_dir(path, lookup_flags)?), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_dir(a: &Dir, b: &Dir) -> io::Result<bool> {
        Ok(same_meta(&a.self_metadata()?, &b.self_metadata()?))
    }

    #[test]
    fn test_prepare_inner_operation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir_path = tmpdir.as_ref();
        let tmpdir = Dir::open(tmpdir_path).unwrap();

        tmpdir.create_dir("a", 0o777, LookupFlags::empty()).unwrap();
        tmpdir
            .create_dir("a/b", 0o777, LookupFlags::empty())
            .unwrap();

        for (path, lookup_flags, expect_dname, expect_fname) in [
            ("/", LookupFlags::IN_ROOT, None, None),
            (".", LookupFlags::empty(), None, None),
            ("a", LookupFlags::empty(), None, Some("a")),
            ("a/", LookupFlags::empty(), None, Some("a/")),
            ("a/.", LookupFlags::empty(), Some("a"), None),
            ("a/b", LookupFlags::empty(), Some("a"), Some("b")),
            ("a/b/.", LookupFlags::empty(), Some("a/b"), None),
            ("a/b/c", LookupFlags::empty(), Some("a/b"), Some("c")),
            ("a/..", LookupFlags::empty(), Some("."), None),
            ("/..", LookupFlags::IN_ROOT, Some("."), None),
        ]
        .iter()
        {
            let (subdir, fname) =
                prepare_inner_operation(&tmpdir, Path::new(path), *lookup_flags).unwrap();

            if let Some(expect_dname) = expect_dname {
                assert!(same_dir(
                    &tmpdir.sub_dir(*expect_dname, LookupFlags::empty()).unwrap(),
                    subdir.as_ref().unwrap()
                )
                .unwrap());
            } else {
                assert!(subdir.is_none());
            }

            assert_eq!(expect_fname.map(OsStr::new), fname);
        }

        for (path, lookup_flags, eno) in [
            ("/", LookupFlags::empty(), libc::EXDEV),
            ("..", LookupFlags::empty(), libc::EXDEV),
            ("a/../..", LookupFlags::empty(), libc::EXDEV),
            ("", LookupFlags::empty(), libc::ENOENT),
        ]
        .iter()
        {
            assert_eq!(
                prepare_inner_operation(&tmpdir, Path::new(path), *lookup_flags)
                    .unwrap_err()
                    .raw_os_error(),
                Some(*eno)
            );
        }
    }

    #[test]
    fn test_metadata() {
        for path in [PathBuf::from("."), PathBuf::from("/"), std::env::temp_dir()].iter() {
            let dir = Dir::open(path).unwrap();

            let meta1 = dir.self_metadata().unwrap();

            let meta2 = dir.metadata(".", LookupFlags::empty()).unwrap();
            assert!(util::samestat(meta1.stat(), meta2.stat()));

            let meta3 = dir.metadata("/", LookupFlags::IN_ROOT).unwrap();
            assert!(util::samestat(meta1.stat(), meta3.stat()));

            let meta4 = dir.metadata("..", LookupFlags::IN_ROOT).unwrap();
            assert!(util::samestat(meta1.stat(), meta4.stat()));
        }
    }

    #[test]
    fn test_try_clone() {
        for path in [".", "/"].iter() {
            let dir = Dir::open(*path).unwrap();
            assert!(same_dir(&dir, &dir.try_clone().unwrap()).unwrap());
        }
    }
}
