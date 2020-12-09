use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::unix::prelude::*;
use std::path::Path;

/// Represents a string that can be cheaply re-cast as a `Path`, and possibly also as a `CStr`.
///
/// The design of this was inspired by `openat`'s `AsPath` trait and `nix`'s `NixPath` trait. It's
/// essentially a combination of `AsRef<Path>` and `NixPath`, which allows it to avoid unnecessary
/// memory allocation in multiple cases.
pub trait AsPath {
    /// Convert this string to a `Path`.
    ///
    /// This serves a similar purpose to `AsRef<Path>::as_ref()`, so many of the `AsRef` rules apply
    /// (i.e. it should be very inexpensive and never fail).
    fn as_path(&self) -> &Path;

    /// Calls the given closure with a version of `self` converted to a `CStr`.
    ///
    /// The `CStr` may actually be a `CString` (allocated from the heap), or it may be the original
    /// string if that string is already nul-terminated.
    ///
    /// IMPORTANT: If the string contains an interior nul byte that prevents it from being converted
    /// to a `CString`, the closure will not be called, and a `std::io::Error` converted from a
    /// `std::ffi::NulError` will be returned.
    #[inline]
    fn with_cstr<T, F: FnMut(&CStr) -> io::Result<T>>(&self, mut f: F) -> io::Result<T> {
        f(&CString::new(self.as_path().as_os_str().as_bytes())?)
    }
}

impl<T> AsPath for T
where
    T: AsRef<Path>,
{
    #[inline]
    fn as_path(&self) -> &Path {
        self.as_ref()
    }
}

impl AsPath for CStr {
    #[inline]
    fn as_path(&self) -> &Path {
        OsStr::from_bytes(self.to_bytes()).as_ref()
    }

    #[inline]
    fn with_cstr<T, F: FnMut(&CStr) -> io::Result<T>>(&self, mut f: F) -> io::Result<T> {
        f(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;

    #[test]
    fn test_as_path() {
        assert_eq!("abc/def".as_path(), Path::new("abc/def"));
        assert_eq!(String::from("abc/def").as_path(), Path::new("abc/def"));

        assert_eq!(OsStr::new("abc/def").as_path(), Path::new("abc/def"));
        assert_eq!(OsString::from("abc/def").as_path(), Path::new("abc/def"));

        assert_eq!(
            CStr::from_bytes_with_nul(b"abc/def\0").unwrap().as_path(),
            Path::new("abc/def")
        );
        assert_eq!(
            CString::new(b"abc/def".as_ref()).unwrap().as_path(),
            Path::new("abc/def")
        );
    }

    #[test]
    fn test_with_cstr() {
        let expected = CStr::from_bytes_with_nul(b"abc/def\0").unwrap();

        "abc/def"
            .with_cstr(|s| {
                assert_eq!(s, expected);
                Ok(())
            })
            .unwrap();
        String::from("abc/def")
            .with_cstr(|s| {
                assert_eq!(s, expected);
                Ok(())
            })
            .unwrap();

        OsStr::new("abc/def")
            .with_cstr(|s| {
                assert_eq!(s, expected);
                Ok(())
            })
            .unwrap();
        OsString::from("abc/def")
            .with_cstr(|s| {
                assert_eq!(s, expected);
                Ok(())
            })
            .unwrap();

        CStr::from_bytes_with_nul(b"abc/def\0")
            .unwrap()
            .with_cstr(|s| {
                assert_eq!(s, expected);
                Ok(())
            })
            .unwrap();
        CString::new("abc/def")
            .unwrap()
            .with_cstr(|s| {
                assert_eq!(s, expected);
                Ok(())
            })
            .unwrap();
    }
}
