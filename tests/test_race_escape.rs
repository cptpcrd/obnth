use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::prelude::*;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use obnth::{open_beneath, LookupFlags};

fn same_meta(m1: &fs::Metadata, m2: &fs::Metadata) -> bool {
    m1.ino() == m2.ino() && m1.dev() == m2.dev()
}

fn check_beneath(base_file: &fs::File, dir_fd_meta: &fs::Metadata) -> io::Result<()> {
    // We need to rewind up the directory tree and make sure that we didn't escape because of
    // race conditions with "..".

    let mut prev_meta = None;

    let mut cur_file: Option<fs::File> = None;

    loop {
        let cur_file_ref = cur_file.as_ref().unwrap_or(base_file);

        let cur_meta = cur_file_ref.metadata()?;

        if same_meta(&cur_meta, dir_fd_meta) {
            // We found it! We *didn't* escape.
            return Ok(());
        } else if let Some(prev_meta) = prev_meta.as_ref() {
            if same_meta(&cur_meta, prev_meta) {
                // Trying to open ".." brought us the same directory. That means we're at "/"
                // (the REAL "/").
                // So we escaped the "beneath" directory.
                return Err(io::Error::from_raw_os_error(libc::EXDEV));
            }
        }

        prev_meta = Some(cur_meta);

        let new_fd = unsafe {
            libc::openat(
                cur_file_ref.as_raw_fd(),
                b"..\0".as_ptr() as *const _,
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
        };
        assert!(new_fd >= 0);
        cur_file = Some(unsafe { fs::File::from_raw_fd(new_fd) });
    }
}

#[test]
fn test_race_escape() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir = tmpdir.as_ref();

    fs::create_dir(tmpdir.join("a")).unwrap();
    fs::create_dir(tmpdir.join("a/b")).unwrap();

    let a_file = fs::File::open(tmpdir.join("a")).unwrap();
    let a_fd = a_file.as_raw_fd();
    let a_meta = a_file.metadata().unwrap();

    let thread_running = Arc::new(AtomicBool::new(true));

    let path1 = CString::new(tmpdir.join("a/b").as_os_str().as_bytes()).unwrap();
    let path2 = CString::new(tmpdir.join("b").as_os_str().as_bytes()).unwrap();
    let thread_running_clone = thread_running.clone();
    let join_handle = std::thread::spawn(move || {
        while thread_running_clone.load(Ordering::SeqCst) {
            for _ in 0..10000 {
                unsafe {
                    libc::rename(path1.as_ptr(), path2.as_ptr());
                    libc::rename(path2.as_ptr(), path1.as_ptr());
                }
            }
        }
    });

    let mut successes = 0u64;
    let mut eagains = 0u64;
    let mut enoents = 0u64;
    let mut escapes = 0u64;

    for path in &[
        CString::new("b/..").unwrap(),
        CString::new("b/../b/..").unwrap(),
        CString::new(
            Path::new("b/../../")
                .join(tmpdir.file_name().unwrap())
                .as_os_str()
                .as_bytes(),
        )
        .unwrap(),
    ] {
        for _ in 0..20000 {
            let res = open_beneath(
                a_fd,
                path,
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
                LookupFlags::IN_ROOT,
            );

            match res {
                Ok(f) => {
                    if let Err(e) = check_beneath(&f, &a_meta) {
                        escapes += 1;
                        println!("WARNING: Possible escape detected! (Error: {})", e);
                    }

                    successes += 1;
                }
                Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => eagains += 1,
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => enoents += 1,
                Err(e) => panic!("{}", e),
            }
        }
    }

    thread_running.store(false, Ordering::SeqCst);
    join_handle.join().unwrap();

    println!("Opened successfully: {} times", successes);
    println!("Failed with ENOENT: {} times", enoents);
    println!("Failed with EAGAIN: {} times", eagains);
    println!("Escaped: {} times", escapes);

    if escapes > 0 {
        panic!("Escape detected!");
    }
}
