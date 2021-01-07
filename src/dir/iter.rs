use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::unix::prelude::*;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::util;

use super::{FileType, Metadata};

#[derive(Debug)]
struct Dstream {
    dir: NonNull<libc::DIR>,
}

impl Dstream {
    #[inline]
    fn as_ptr(&self) -> *mut libc::DIR {
        self.dir.as_ptr()
    }
}

impl AsRawFd for Dstream {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        unsafe { libc::dirfd(self.dir.as_ptr()) }
    }
}

impl Drop for Dstream {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.dir.as_ptr());
        }
    }
}

/// An iterator over the entries of a directory.
#[derive(Debug)]
pub struct ReadDirIter {
    dstream: Arc<Dstream>,
}

impl ReadDirIter {
    #[inline]
    pub(crate) fn new_consume(fd: RawFd) -> io::Result<Self> {
        match NonNull::new(unsafe { libc::fdopendir(fd) }) {
            Some(dir) => Ok(Self {
                dstream: Arc::new(Dstream { dir }),
            }),

            None => {
                let err = io::Error::last_os_error();
                unsafe {
                    libc::close(fd);
                }
                Err(err)
            }
        }
    }

    /// Rewind to the beginning of the directory.
    ///
    /// This directly corresponds to rewinddir(3).
    #[inline]
    pub fn rewind(&mut self) {
        unsafe {
            libc::rewinddir(self.dstream.as_ptr());
        }
    }

    /// Get the current seek position.
    ///
    /// This directly corresponds to telldir(3). It is currently not available on Android.
    #[cfg(not(target_os = "android"))]
    #[inline]
    pub fn tell(&self) -> SeekPos {
        SeekPos(unsafe { libc::telldir(self.dstream.as_ptr()) })
    }

    /// Set the new seek position.
    ///
    /// This directly corresponds to seekdir(3). `pos` must be a value previously returned by
    /// [`tell()`]. This function is currently not available on Android.
    ///
    /// [`tell()`]: #method.tell
    #[cfg(not(target_os = "android"))]
    #[inline]
    pub fn seek(&mut self, pos: SeekPos) {
        unsafe {
            libc::seekdir(self.dstream.as_ptr(), pos.0);
        }
    }
}

impl Iterator for ReadDirIter {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            *util::errno_ptr() = 0;
        }

        loop {
            let raw_entry = unsafe { libc::readdir(self.dstream.as_ptr()) };

            if raw_entry.is_null() {
                return match unsafe { *util::errno_ptr() } {
                    0 => None,
                    eno => Some(Err(io::Error::from_raw_os_error(eno))),
                };
            } else if let Some(entry) = unsafe { Entry::from_raw(&self, raw_entry) } {
                return Some(Ok(entry));
            }
        }
    }
}

/// Represents a seek position for a `ReadDirIter` struct.
///
/// The actual raw offset is not exposed because it is an opaque value that must be obtained with
/// [`tell()`].
///
/// [`tell()`]: ./struct.ReadDirIter.html#method.tell
#[derive(Copy, Clone, Debug)]
pub struct SeekPos(libc::c_long);

/// An entry encountered when iterating over a directory.
#[derive(Clone, Debug)]
pub struct Entry {
    fname: CString,
    ino: u64,
    ftype: Option<FileType>,
    dstream: Arc<Dstream>,
}

impl Entry {
    #[inline]
    unsafe fn from_raw(rdir_it: &ReadDirIter, entry: *const libc::dirent) -> Option<Self> {
        let entry = &*entry;

        let c_fname = CStr::from_ptr(entry.d_name.as_ptr());
        let fname_bytes = c_fname.to_bytes();

        if fname_bytes == b"." || fname_bytes == b".." {
            return None;
        }

        cfg_if::cfg_if! {
            if #[cfg(any(
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "openbsd",
                target_os = "netbsd",
            ))] {
                let ino = entry.d_fileno as u64;
            } else {
                let ino = entry.d_ino as u64;
            }
        }

        Some(Self {
            fname: c_fname.to_owned(),
            ino,
            ftype: match entry.d_type {
                libc::DT_REG => Some(FileType::File),
                libc::DT_DIR => Some(FileType::Directory),
                libc::DT_LNK => Some(FileType::Symlink),
                libc::DT_SOCK => Some(FileType::Socket),
                libc::DT_BLK => Some(FileType::Block),
                libc::DT_CHR => Some(FileType::Character),
                libc::DT_FIFO => Some(FileType::Fifo),
                _ => None,
            },
            dstream: rdir_it.dstream.clone(),
        })
    }

    /// Get the name of this entry.
    #[inline]
    pub fn name(&self) -> &OsStr {
        OsStr::from_bytes(self.fname.as_bytes())
    }

    /// Get this entry's inode.
    ///
    /// Note: If this entry refers to a mountpoint (including bind mounts on Linux), this may be
    /// the inode of the *underlying directory* on which the filesystem is mounted. So this value
    /// may not match, for example, `self.metadata()?.ino()` (which looks up the actual root
    /// directory of the mountpoint).
    #[inline]
    pub fn ino(&self) -> u64 {
        self.ino
    }

    /// Get the entry's file type without making any additional syscalls, if possible.
    ///
    /// If this returns `None`, the OS didn't specify a file type.
    #[inline]
    pub fn file_type(&self) -> Option<FileType> {
        self.ftype
    }

    /// Get the metadata for the file named by this entry.
    ///
    /// This method will not traverse symlinks.
    pub fn metadata(&self) -> io::Result<Metadata> {
        util::fstatat(
            self.dstream.as_raw_fd(),
            &self.fname,
            libc::AT_SYMLINK_NOFOLLOW,
        )
        .map(Metadata::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_consume_error() {
        assert_eq!(
            ReadDirIter::new_consume(-1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
    }
}
