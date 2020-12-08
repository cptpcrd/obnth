// On Linux/Android, O_PATH provides similar (though not *quite* identical) semantics to POSIX's
// O_SEARCH
#[cfg(any(target_os = "linux", target_os = "android"))]
pub const DIR_OPEN_FLAGS: libc::c_int = libc::O_PATH | libc::O_DIRECTORY;

// On FreeBSD, O_SEARCH (added in newer versions) is just an alias for O_EXEC, and O_EXEC on
// directories has historically had similar semantics to POSIX's O_SEARCH (even before the alias was
// added)
#[cfg(target_os = "freebsd")]
pub const DIR_OPEN_FLAGS: libc::c_int = libc::O_EXEC | libc::O_DIRECTORY;

// These OSes have an actual O_SEARCH flag
#[cfg(any(target_os = "solaris", target_os = "illumos"))]
pub const DIR_OPEN_FLAGS: libc::c_int = libc::O_SEARCH | libc::O_DIRECTORY;

// No (known) equivalent of O_SEARCH on the current platform.
// O_RDONLY will usually work OK, except when encountering directories with modes like --x------
// (0o100). Unfortunately, we just won't be able to handle those cases on these platforms.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "solaris",
    target_os = "illumos",
)))]
pub const DIR_OPEN_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;

// Linux's default (it seems sysconf(_SC_SYMLOOP_MAX) always fails on glibc, and this is a
// reasonable limit)
pub const DEFAULT_SYMLOOP_MAX: u16 = 40;
