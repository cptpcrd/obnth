use std::fs;
use std::io;
use std::os::unix::prelude::*;

use crate::{AsPath, Dir, LookupFlags};

/// A struct that can be used to open files within a directory.
///
/// This is directly analogous to `std::fs::OpenOptions`, except that it only looks up files within
/// a specific `Dir`.
///
/// An `OpenOptions` struct can be created with [`Dir::open_file()`].
///
/// [`Dir::open_file()`]: ./struct.Dir.html#method.open_file
#[derive(Clone, Debug)]
pub struct OpenOptions<'a> {
    dir: &'a Dir,
    read: bool,
    write: bool,
    create: bool,
    create_new: bool,
    append: bool,
    truncate: bool,
    custom_flags: libc::c_int,
    mode: libc::mode_t,
    lookup_flags: LookupFlags,
}

impl<'a> OpenOptions<'a> {
    #[inline]
    pub(crate) fn beneath(dir: &'a Dir) -> Self {
        Self {
            dir,
            read: false,
            write: false,
            create: false,
            create_new: false,
            append: false,
            truncate: false,
            custom_flags: 0,
            mode: 0o666,
            lookup_flags: LookupFlags::empty(),
        }
    }

    /// Enable the option for read access.
    #[inline]
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    /// Enable the option for write access.
    #[inline]
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    /// Create the a new file if it does not exist.
    #[inline]
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    /// Create a new file, failing if it already exists.
    ///
    /// This is atomic. Enabling it causes [`.create()`] and [`.truncate()`] to be ignored.
    ///
    /// [`.create()`]: #method.create
    /// [`.truncate()`]: #method.truncate
    #[inline]
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    /// Enable append mode.
    #[inline]
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    /// If the file already exists, truncate it while opening.
    #[inline]
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    /// Set the mode with which the file will be opened (e.g `0o777`).
    ///
    /// The OS will mask out the system umask value.
    #[inline]
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        self.mode = mode as libc::mode_t;
        self
    }

    /// Pass custom flags when opening the file.
    ///
    /// Like `std::fs::OpenOptions`, `O_ACCMODE` is masked out from the given flags.
    #[inline]
    pub fn custom_flags(&mut self, flags: libc::c_int) -> &mut Self {
        self.custom_flags = flags;
        self
    }

    /// Set the "lookup flags" used when opening the file.
    ///
    /// See [`LookupFlags`] for more information. (By default, none of the "lookup flags" are
    /// enabled.)
    ///
    /// [`LookupFlags`]: ./struct.LookupFlags.html
    pub fn lookup_flags(&mut self, lookup_flags: LookupFlags) -> &mut Self {
        self.lookup_flags = lookup_flags;
        self
    }

    fn flags(&self) -> io::Result<libc::c_int> {
        let mut flags = self.custom_flags & !libc::O_ACCMODE;

        if self.write || self.append {
            if self.read {
                flags |= libc::O_RDWR;
            } else {
                flags |= libc::O_WRONLY;
            }

            if self.create_new {
                flags |= libc::O_CREAT | libc::O_EXCL;
            } else {
                if self.create {
                    flags |= libc::O_CREAT;
                }

                if self.truncate {
                    flags |= libc::O_TRUNC;
                }
            }

            if self.append {
                flags |= libc::O_APPEND;
            }
        } else if self.read {
            flags |= libc::O_RDONLY;

            if self.create_new || self.create || self.truncate {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
        } else {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        Ok(flags)
    }

    /// Open the file at `path` with the options specified by `path`.
    #[inline]
    pub fn open<P: AsPath>(&self, path: P) -> io::Result<fs::File> {
        crate::open_beneath(
            self.dir.as_raw_fd(),
            path,
            self.flags()?,
            self.mode,
            self.lookup_flags,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_flags() {
        let dir = Dir::open("/").unwrap();
        let opts = dir.open_file();

        assert_eq!(opts.clone().read(true).flags().unwrap(), libc::O_RDONLY);
        assert_eq!(opts.clone().write(true).flags().unwrap(), libc::O_WRONLY);
        assert_eq!(
            opts.clone().read(true).write(true).flags().unwrap(),
            libc::O_RDWR
        );

        assert_eq!(
            opts.clone().append(true).flags().unwrap(),
            libc::O_WRONLY | libc::O_APPEND
        );
        assert_eq!(
            opts.clone().read(true).append(true).flags().unwrap(),
            libc::O_RDWR | libc::O_APPEND
        );

        assert_eq!(
            opts.clone().write(true).create_new(true).flags().unwrap(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
        );
        assert_eq!(
            opts.clone()
                .read(true)
                .write(true)
                .create_new(true)
                .flags()
                .unwrap(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
        );

        assert_eq!(
            opts.clone().write(true).create(true).flags().unwrap(),
            libc::O_WRONLY | libc::O_CREAT
        );

        assert_eq!(
            opts.clone().write(true).truncate(true).flags().unwrap(),
            libc::O_WRONLY | libc::O_TRUNC
        );

        assert_eq!(
            opts.clone()
                .write(true)
                .create(true)
                .truncate(true)
                .flags()
                .unwrap(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC
        );

        assert_eq!(
            opts.clone().flags().unwrap_err().raw_os_error(),
            Some(libc::EINVAL)
        );

        assert_eq!(
            opts.clone()
                .read(true)
                .create(true)
                .flags()
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(
            opts.clone()
                .read(true)
                .create_new(true)
                .flags()
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(
            opts.clone()
                .read(true)
                .truncate(true)
                .flags()
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
    }

    #[test]
    fn test_custom_flags() {
        let dir = Dir::open("/").unwrap();
        let opts = dir.open_file();

        assert_eq!(
            opts.clone()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .flags()
                .unwrap(),
            libc::O_RDONLY | libc::O_NOFOLLOW
        );
    }
}
