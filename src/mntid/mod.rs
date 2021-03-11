cfg_if::cfg_if! {
    if #[cfg(any(target_os = "linux", target_os = "android"))] {
        mod linux;
        pub use linux::{MountId, identify_mount};
    } else {
        mod unix;
        pub use unix::{MountId, identify_mount};
    }
}
