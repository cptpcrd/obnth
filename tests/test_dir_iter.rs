use std::ffi::CString;
use std::io;
use std::os::unix::net::UnixListener;
use std::os::unix::prelude::*;

use obnth::{Dir, Entry, FileType, LookupFlags};

fn check_entries_match(entries_a: &[Entry], entries_b: &[Entry]) {
    assert_eq!(entries_a.len(), entries_b.len());

    let mut entries_a = entries_a.to_vec();
    entries_a.sort_unstable_by(|a, b| a.name().cmp(b.name()));

    let mut entries_b = entries_b.to_vec();
    entries_b.sort_unstable_by(|a, b| a.name().cmp(b.name()));

    for (a, b) in entries_a.iter().zip(entries_b.iter()) {
        assert_eq!(a.name(), b.name());
    }
}

#[test]
fn test_dir_iter_basic() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    tmpdir
        .create_dir("dir", 0o777, LookupFlags::empty())
        .unwrap();
    let dir_meta = tmpdir_path.join("dir").metadata().unwrap();

    tmpdir
        .open_file()
        .write(true)
        .create_new(true)
        .open("file")
        .unwrap();
    let file_meta = tmpdir_path.join("file").metadata().unwrap();

    tmpdir
        .symlink("link", "dest", LookupFlags::empty())
        .unwrap();
    let link_meta = tmpdir_path.join("link").symlink_metadata().unwrap();

    UnixListener::bind(tmpdir_path.join("sock")).unwrap();
    let sock_meta = tmpdir_path.join("sock").symlink_metadata().unwrap();

    let c_fifo_path = CString::new(tmpdir_path.join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_fifo_path.as_ptr(), 0o777,) }, 0,);
    let fifo_meta = tmpdir_path.join("fifo").metadata().unwrap();

    let mut reader = tmpdir.list_self().unwrap();

    let mut entries = reader.by_ref().collect::<io::Result<Vec<Entry>>>().unwrap();

    // Now rewind and make sure it yields the same entries
    reader.rewind();
    let entries_alt = reader.collect::<io::Result<Vec<Entry>>>().unwrap();

    // Now list it with list_dir()
    let entries_alt2 = tmpdir
        .list_dir(".", LookupFlags::empty())
        .unwrap()
        .collect::<io::Result<Vec<Entry>>>()
        .unwrap();

    check_entries_match(&entries, &entries_alt);
    check_entries_match(&entries, &entries_alt2);

    let expected_entries = &[
        (b"dir".as_ref(), dir_meta.ino(), FileType::Directory),
        (b"fifo".as_ref(), fifo_meta.ino(), FileType::Fifo),
        (b"file".as_ref(), file_meta.ino(), FileType::File),
        (b"link".as_ref(), link_meta.ino(), FileType::Symlink),
        (b"sock".as_ref(), sock_meta.ino(), FileType::Socket),
    ];

    assert_eq!(entries.len(), expected_entries.len());

    entries.sort_unstable_by(|a, b| a.name().cmp(b.name()));
    for (entry, (name, ino, expect_ftype)) in entries.iter().zip(expected_entries.iter()) {
        assert_eq!(entry.name().as_bytes(), *name);
        assert_eq!(entry.ino(), *ino);
        if let Some(ftype) = entry.file_type() {
            assert_eq!(ftype, *expect_ftype);
        }
    }
}

#[cfg(not(target_os = "android"))]
#[test]
fn test_dir_iter_seek() {
    let tmpdir = tempfile::tempdir().unwrap();
    let tmpdir_path = tmpdir.as_ref();
    let tmpdir = Dir::open(tmpdir_path).unwrap();

    tmpdir
        .create_dir("dir", 0o777, LookupFlags::empty())
        .unwrap();

    tmpdir
        .open_file()
        .write(true)
        .create_new(true)
        .open("file")
        .unwrap();

    tmpdir
        .symlink("link", "dest", LookupFlags::empty())
        .unwrap();

    UnixListener::bind(tmpdir_path.join("sock")).unwrap();

    let c_fifo_path = CString::new(tmpdir_path.join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_fifo_path.as_ptr(), 0o777,) }, 0,);

    let mut reader = tmpdir.list_self().unwrap();

    // Get the starting position
    let start_pos = reader.tell();

    // Collect all the entries
    let entries = reader.by_ref().collect::<io::Result<Vec<Entry>>>().unwrap();

    // Now it's exhausted, and we can get the ending position
    assert!(reader.next().is_none());
    let end_pos = reader.tell();

    // Seek to the start and make sure everything matches
    reader.seek(start_pos);
    check_entries_match(
        &entries,
        &reader.by_ref().collect::<io::Result<Vec<Entry>>>().unwrap(),
    );

    // Seek to the end and make sure it's empty
    reader.seek(end_pos);
    assert!(reader.next().is_none());
}
