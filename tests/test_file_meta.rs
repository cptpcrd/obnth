use std::ffi::CString;
use std::fs;
use std::os::unix::net::UnixListener;
use std::os::unix::prelude::*;

use obnth::{Dir, FileType, LookupFlags, Metadata};

pub fn same_meta(m1: &Metadata, m2: &fs::Metadata) -> bool {
    m1.dev() as u64 == m2.dev() && m1.ino() as u64 == m2.ino()
}

#[test]
fn test_file_meta_basic() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    tmpdir
        .create_dir("dir", 0o777, LookupFlags::empty())
        .unwrap();
    let dir_meta = tmpdir.metadata("dir", LookupFlags::empty()).unwrap();
    let dir_meta2 = tmpdir_path.join("dir").metadata().unwrap();

    tmpdir
        .open_file()
        .write(true)
        .create_new(true)
        .open("file")
        .unwrap();
    let file_meta = tmpdir.metadata("file", LookupFlags::empty()).unwrap();
    let file_meta2 = tmpdir_path.join("file").metadata().unwrap();

    tmpdir
        .open_file()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open("rofile")
        .unwrap();
    let rofile_meta = tmpdir.metadata("rofile", LookupFlags::empty()).unwrap();
    let rofile_meta2 = tmpdir_path.join("rofile").metadata().unwrap();

    tmpdir
        .symlink("link", "dest", LookupFlags::empty())
        .unwrap();
    let link_meta = tmpdir.metadata("link", LookupFlags::empty()).unwrap();
    let link_meta2 = tmpdir_path.join("link").symlink_metadata().unwrap();

    UnixListener::bind(tmpdir_path.join("sock")).unwrap();
    let sock_meta = tmpdir.metadata("sock", LookupFlags::empty()).unwrap();
    let sock_meta2 = tmpdir_path.join("sock").symlink_metadata().unwrap();

    let c_fifo_path = CString::new(tmpdir_path.join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_fifo_path.as_ptr(), 0o777,) }, 0,);
    let fifo_meta = tmpdir.metadata("fifo", LookupFlags::empty()).unwrap();
    let fifo_meta2 = tmpdir_path.join("fifo").metadata().unwrap();

    assert_eq!(dir_meta.file_type(), FileType::Directory);
    assert!(dir_meta.is_dir());
    assert!(!dir_meta.is_file());
    assert!(same_meta(&dir_meta, &dir_meta2));

    assert_eq!(file_meta.file_type(), FileType::File);
    assert!(file_meta.is_file());
    assert!(!file_meta.is_dir());
    assert!(!file_meta.permissions().readonly());
    assert_eq!(file_meta.len(), 0);
    assert!(same_meta(&file_meta, &file_meta2));

    assert_eq!(rofile_meta.file_type(), FileType::File);
    assert!(rofile_meta.is_file());
    assert!(!rofile_meta.is_dir());
    assert!(rofile_meta.permissions().readonly());
    assert_eq!(rofile_meta.len(), 0);
    assert!(same_meta(&rofile_meta, &rofile_meta2));

    assert_eq!(link_meta.file_type(), FileType::Symlink);
    assert!(!link_meta.is_file());
    assert!(!link_meta.is_dir());
    assert_eq!(link_meta.len(), 4);
    assert!(same_meta(&link_meta, &link_meta2));

    assert_eq!(sock_meta.file_type(), FileType::Socket);
    assert!(!sock_meta.is_file());
    assert!(!sock_meta.is_dir());
    assert!(same_meta(&sock_meta, &sock_meta2));

    assert_eq!(fifo_meta.file_type(), FileType::Fifo);
    assert!(!fifo_meta.is_file());
    assert!(!fifo_meta.is_dir());
    assert!(same_meta(&fifo_meta, &fifo_meta2));
}
