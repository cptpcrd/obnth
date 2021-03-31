use std::ffi::CStr;
use std::io;
use std::io::prelude::*;
use std::mem::MaybeUninit;
use std::os::unix::prelude::*;

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct MountId(u32);

#[inline]
pub fn identify_mount(fd: RawFd) -> io::Result<MountId> {
    get_mnt_id(fd).map(MountId)
}

#[repr(C)]
struct file_handle {
    pub handle_bytes: libc::c_uint,
    pub handle_type: libc::c_int,
}

extern "C" {
    fn name_to_handle_at(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        handle: *mut file_handle,
        mount_id: *mut libc::c_int,
        flags: libc::c_int,
    ) -> libc::c_int;
}

const PROC_SUPER_MAGIC: libc::c_long = 0x9fa0;

#[inline]
fn statfs(path: &CStr) -> io::Result<libc::statfs> {
    let mut buf = MaybeUninit::uninit();
    if unsafe { libc::statfs(path.as_ptr(), buf.as_mut_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { buf.assume_init() })
    }
}

fn get_mnt_id(fd: RawFd) -> io::Result<u32> {
    if let Some(mnt_id) = get_mnt_id_name_handle(fd)? {
        return Ok(mnt_id);
    }

    if let Some(mnt_id) = get_mnt_id_procfs(fd)? {
        return Ok(mnt_id);
    }

    Err(io::Error::new(
        io::ErrorKind::Other,
        "unable to get mount ID; is /proc mounted?",
    ))
}

fn get_mnt_id_name_handle(fd: RawFd) -> io::Result<Option<u32>> {
    // name_to_handle_at() (added in Linux 2.6.39) allows retrieving the mount ID

    let mut handle = file_handle {
        handle_bytes: 0,
        handle_type: 0,
    };

    let mut mnt_id = -1;

    if unsafe {
        name_to_handle_at(
            fd,
            b"\0".as_ptr() as *mut _,
            &mut handle,
            &mut mnt_id,
            libc::AT_EMPTY_PATH,
        )
    } == 0
    {
        // name_to_handle_at() *should* fail with EOVERFLOW if given this empty `handle`. If it
        // succeeds, something must be wrong and we should skip using name_to_handle_at() altogether.

        return Ok(None);
    }

    match unsafe { *crate::util::errno_ptr() } {
        // EOVERFLOW is expected, and the mount ID *should* be set
        libc::EOVERFLOW => {
            debug_assert!(mnt_id >= 0);
            Ok(Some(mnt_id as u32))
        }

        // ENOSYS (older kernels), EPERM (possibly from seccomp), and EOPNOTSUPP (not supported by
        // the filesystem) mean it's not available
        libc::ENOSYS | libc::EPERM | libc::EOPNOTSUPP => Ok(None),

        // Other errors indicate different issues (e.g. EBADF)
        eno => Err(io::Error::from_raw_os_error(eno)),
    }
}

fn get_mnt_id_procfs(fd: RawFd) -> io::Result<Option<u32>> {
    // The `mnt_id` field in `/proc/self/fdinfo/$FD` (present since Linux 3.15) provides the mount
    // ID

    // Is `/proc` mounted, and is it really a procfs?
    if !is_procfs_real() {
        return Ok(None);
    }

    let path = format!("/proc/self/fdinfo/{}\0", fd);
    let path = CStr::from_bytes_with_nul(path.as_bytes()).unwrap();

    let mut file = match crate::util::openat(libc::AT_FDCWD, path, libc::O_RDONLY, 0) {
        Ok(f) => io::BufReader::new(f),

        Err(e) => {
            // Translate ENOENT to EBADF
            // is_procfs_real() made sure that `/proc` is really a procfs, so ENOENT can't mean
            // anything else.
            return Err(if e.raw_os_error() == Some(libc::ENOENT) {
                io::Error::from_raw_os_error(libc::EBADF)
            } else {
                e
            });
        }
    };

    let mut buf = Vec::with_capacity(20);

    loop {
        let n = file.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None);
        }

        if buf.starts_with(b"mnt_id:") {
            // If there's a parse error, this will return Ok(None)
            return Ok(std::str::from_utf8(&buf[7..])
                .ok()
                .and_then(|s| s.trim().parse().ok()));
        }

        buf.clear();
    }
}

/// Check whether the filesystem mounted on /proc (if any) is really a procfs
#[inline]
fn is_procfs_real() -> bool {
    matches!(
        statfs(unsafe { CStr::from_bytes_with_nul_unchecked(b"/proc\0") }),
        Ok(stat) if stat.f_type == PROC_SUPER_MAGIC as _,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::Path;

    #[test]
    fn test_get_mnt_id() {
        fn check(fd: RawFd) {
            let mnt_id = get_mnt_id(fd).unwrap();

            if let Some(id) = get_mnt_id_name_handle(fd).unwrap() {
                assert_eq!(mnt_id, id);
            }

            assert_eq!(get_mnt_id_procfs(fd).unwrap().unwrap(), mnt_id);
        }

        for path in [
            Path::new("/"),
            Path::new("."),
            Path::new("/proc"),
            &std::env::temp_dir(),
        ]
        .iter()
        {
            let file = fs::File::open(path).unwrap();
            check(file.as_raw_fd());
        }

        assert_eq!(
            get_mnt_id(-1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
        assert_eq!(
            get_mnt_id_name_handle(-1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
        assert_eq!(
            get_mnt_id_procfs(-1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
    }

    #[test]
    fn test_statfs() {
        assert_eq!(
            statfs(CStr::from_bytes_with_nul(b"NOEXIST\0").unwrap())
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );

        assert!(is_procfs_real());
    }
}
