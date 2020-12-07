use std::fs;
use std::os::unix::prelude::*;

/// Represents the possible file types.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum FileType {
    File,
    Directory,
    Symlink,
    Socket,
    Block,
    Character,
    Fifo,
}

/// Represents metadata information about a file. Similar to `std::fs::Metadata`.
pub struct Metadata {
    stat: libc::stat,
}

#[allow(clippy::len_without_is_empty)]
impl Metadata {
    #[inline]
    pub(crate) fn new(stat: libc::stat) -> Self {
        Self { stat }
    }

    /// Get the type of this file.
    ///
    /// Returns `FileType::Other` if the file type is not recognized (unlikely).
    pub fn file_type(&self) -> FileType {
        match self.stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => FileType::File,
            libc::S_IFDIR => FileType::Directory,
            libc::S_IFLNK => FileType::Symlink,
            libc::S_IFSOCK => FileType::Socket,
            libc::S_IFBLK => FileType::Block,
            libc::S_IFCHR => FileType::Character,
            libc::S_IFIFO => FileType::Fifo,
            _ => unreachable!(),
        }
    }

    /// Returns a reference to the underlying `libc::stat` structure.
    #[inline]
    pub fn stat(&self) -> &libc::stat {
        &self.stat
    }

    /// Returns `true` if this `Metadata` object refers to a regular file; `false` if it does not.
    #[inline]
    pub fn is_file(&self) -> bool {
        self.stat.st_mode & libc::S_IFMT == libc::S_IFREG
    }

    /// Returns `true` if this `Metadata` object refers to a regular directory; `false` if it does
    /// not.
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.stat.st_mode & libc::S_IFMT == libc::S_IFDIR
    }

    /// Get the permissions of this file.
    #[inline]
    pub fn permissions(&self) -> fs::Permissions {
        fs::Permissions::from_mode(self.stat.st_mode as u32)
    }

    /// Get the size of this file.
    #[inline]
    pub fn len(&self) -> u64 {
        self.stat.st_size as u64
    }
}
