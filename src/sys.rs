#[cfg(any(target_os = "macos", target_os = "ios"))]
pub const O_NOFOLLOW_ANY: libc::c_int = 0x20000000;
