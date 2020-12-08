//! # What is this crate useful for?
//!
//! `obnth` makes it easy to safely open files in untrusted directories. This may be useful, for
//! example, in web servers that serve files from user-controlled "webdocs" directories, or in
//! set-UID programs that need to open files based on user-supplied information.
//!
//! As a more concrete example, say you're serving files from `/srv/user1`:
//! ```no_run
//! # use obnth::Dir;
//! let dir = Dir::open("/srv/user1").unwrap();
//!
//! // If `a` and/or `a/index.html` are symlinks, they will be followed, but
//! // they can't escape `/srv/user1`.
//! let file = dir.open_file().open("a/index.html").unwrap();
//! ```
//!
//! # Why is this dangerous? Can't I just use `Path::join()`?
//!
//! A naive implementation of the above use case would just do something like this:
//! ```no_run
//! # use std::fs::File;
//! # use std::path::Path;
//! // DO NOT DO THIS
//! let file = File::open(Path::new("/srv/user1").join("a/index.html")).unwrap();
//! ```
//!
//! However, this is very dangerous. Just consider the case where `/srv/user1/a/index.html` is a
//! symlink to, say, `/etc/shadow`. If the program in question is a web server, this essentially
//! gives control of the system to anyone who can create a symlink in the target directory.
//!
//! The next logical attempt is to use `Path::canonicalize()` and then validate the path before
//! opening it:
//! ```no_run
//! # use std::fs::File;
//! # use std::path::Path;
//! // DO NOT DO THIS
//! let path = Path::new("/srv/user1").join("a/index.html").canonicalize().unwrap();
//! assert!(path.starts_with("/srv/user1"));
//! let file = File::open(path).unwrap();
//! ```
//!
//! However, between the call to `Path::canonicalize()` and the call to `File::open()`, an attacker
//! may replace `a/index.html` with a symlink to another file, and still trick the program into
//! opening files it shouldn't.
//!
//! # Why not use the `openat` or `libpathrs` crates?
//!
//! - `openat` provides friendly interfaces to the `*at()` functions. However, it doesn't perform
//!   any validation of the paths being provided, and you can very easily escape the directory by
//!   specifying absolute paths or paths containg `..` components.
//!
//!   `openat` is useful, for example, if you need to walk through a directory tree in a very
//!   controlled manner, and all you want is slightly higher-level interfaces to the `*at()`
//!   functions. However, if you just want to open a file within the given directory (and guarantee
//!   that it won't escape), `obnth` may be more useful.
//!
//! - `libpathrs` serves a similar role to this library, but it is very much Linux-specific.
//!   `obnth` works on Linux, macOS, and the BSDs (which necessitated certain differences in the
//!   semantics).

mod as_path;
mod constants;
mod dir;
mod open;
mod sys;
mod util;

pub use as_path::*;
pub use dir::*;
pub use open::*;
