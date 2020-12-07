#[cfg(target_os = "linux")]
#[repr(transparent)]
bitflags::bitflags! {
    pub struct ResolveFlags: u64 {
        const NO_XDEV = 0x01;
        const NO_MAGICLINKS = 0x02;
        const NO_SYMLINKS = 0x04;
        const BENEATH = 0x08;
        const IN_ROOT = 0x10;
    }
}

#[cfg(target_os = "linux")]
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct open_how {
    pub flags: u64,
    pub mode: u64,
    pub resolve: ResolveFlags,
}

// Correct on every architecture except alpha, which Rust doesn't support
#[cfg(target_os = "linux")]
pub const SYS_OPENAT2: libc::c_long = 437;
