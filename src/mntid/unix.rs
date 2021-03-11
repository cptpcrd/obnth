use std::io;
use std::os::unix::prelude::*;

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct MountId(u64);

#[inline]
pub fn identify_mount(fd: RawFd) -> io::Result<MountId> {
    let st = crate::util::fstat(fd)?;
    Ok(MountId(st.st_dev as u64))
}
