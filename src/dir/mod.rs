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

            match util::unlinkat(fd, &cstr(fname)?, true) {
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                    Err(io::Error::from_raw_os_error(libc::ENOTEMPTY))
                }
                res => res,
            }
        } else {
            let is_same = if let Some(subdir) = subdir.as_ref() {
                same_dir(self, subdir)?
            } else {
                true
            };

            Err(std::io::Error::from_raw_os_error(if is_same {
                libc::EBUSY
            } else {
                libc::ENOTEMPTY
            }))
        }
    }

    /// Remove a file within this directory.
    pub fn remove_file<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<()> {
        let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

        if let Some(fname) = fname {
            let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

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

            target.with_cstr(|target| util::symlinkat(target, fd, &cstr(fname)?))
        } else {
            Err(io::Error::from_raw_os_error(libc::EEXIST))
        }
    }

    /// Read the contents of the specified symlink.
    pub fn read_link<P: AsPath>(&self, path: P, lookup_flags: LookupFlags) -> io::Result<PathBuf> {
        // On Linux, we can actually get a file descriptor to the *symlink*, then readlink() that.
        // However, if we don't have openat2() then this costs an extra syscall, so let's only do
        // it if the `openat2` feature is enabled.

        #[cfg(all(target_os = "linux", feature = "openat2"))]
        let target = {
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
                Ok(target) => target,

                // This error means we got a file descriptor that doesn't point to a symlink
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                    return Err(io::Error::from_raw_os_error(libc::EINVAL));
                }

                Err(e) => return Err(e),
            }
        };

        // On other OSes (or without openat2()), we have to split the path and perform a few more
        // allocations.

        #[cfg(not(all(target_os = "linux", feature = "openat2")))]
        let target = {
            let (subdir, fname) = prepare_inner_operation(self, path.as_path(), lookup_flags)?;

            if let Some(fname) = fname {
                let fd = subdir.as_ref().unwrap_or(self).as_raw_fd();

                util::readlinkat(fd, &cstr(fname)?)?
            } else {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
        };

        Ok(target)
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
            fname.with_cstr(|s| {
                util::fstatat(subdir.as_raw_fd(), s, libc::AT_SYMLINK_NOFOLLOW).map(Metadata::new)
            })
        } else {
            subdir.self_metadata()
        }
    }

    /// Recover the path to the directory that this `Dir` is currently open to.
    ///
    /// **WARNING**: Be careful of race conditions, and **don't** use this path to open any files
    /// within the directory.
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

            if unsafe { libc::fcntl(self.fd, libc::F_GETPATH, &mut buf) } == 0 {
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

                match entry.file_type() {
                    Some(FileType::Directory) | None => {
                        if let Ok(entry_stat) = util::fstatat(
                            parent.as_raw_fd(),
                            entry.c_name(),
                            libc::AT_SYMLINK_NOFOLLOW,
                        ) {
                            if util::samestat(sub_meta.stat(), &entry_stat) {
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

fn same_dir(a: &Dir, b: &Dir) -> io::Result<bool> {
    Ok(same_meta(&a.self_metadata()?, &b.self_metadata()?))
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

    // We now know that `path` is not empty, and it doesn't start with a "/"

    if let Some(fname) = util::path_basename(path) {
        debug_assert!(!path.ends_with(".."));

        let fname = if fname.as_bytes() == b"." {
            None
        } else {
            Some(fname)
        };

        // Because of the conditions listed above, path.parent() should never be None
        let parent = path.parent().unwrap();

        if parent.as_os_str().is_empty() {
            // Though it might be empty, in which case we just reuse the existing directory
            Ok((None, fname))
        } else {
            Ok((Some(dir.sub_dir(parent, lookup_flags)?), fname))
        }
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

    #[test]
    fn test_prepare_inner_operation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir_path = tmpdir.as_ref();
        let tmpdir = Dir::open(tmpdir_path).unwrap();

        tmpdir.create_dir("a", 0o777, LookupFlags::empty()).unwrap();

        for (path, lookup_flags, expect_dname, expect_fname) in [
            ("/", LookupFlags::IN_ROOT, None, None),
            (".", LookupFlags::empty(), None, None),
            ("a", LookupFlags::empty(), None, Some("a")),
            ("a/", LookupFlags::empty(), None, Some("a/")),
            ("a/b", LookupFlags::empty(), Some("a"), Some("b")),
            ("a/..", LookupFlags::empty(), Some("."), None),
            ("/..", LookupFlags::IN_ROOT, Some("."), None),
        ]
        .iter()
        {
            let (subdir, fname) =
                prepare_inner_operation(&tmpdir, Path::new(path), *lookup_flags).unwrap();

            if let Some(expect_dname) = expect_dname {
                assert!(same_dir(
                    &tmpdir.sub_dir(expect_dname, LookupFlags::empty()).unwrap(),
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
            let dir = Dir::open(path).unwrap();
            assert!(same_dir(&dir, &dir.try_clone().unwrap()).unwrap());
        }
    }
}
