use std::path::Path;

use obnth::{Dir, FileType, LookupFlags};

#[test]
fn test_file_types_char() {
    check_file_type(Path::new("/dev/tty"), FileType::Character);
    check_file_type(Path::new("/dev/null"), FileType::Character);
    check_file_type(Path::new("/dev/zero"), FileType::Character);
}

#[test]
fn test_file_types_dir() {
    check_file_type(Path::new("/dev"), FileType::Directory);
}

#[test]
fn test_file_types_file() {
    check_file_type(Path::new("/etc/passwd"), FileType::File);
}

fn check_file_type(path: &Path, ftype: FileType) {
    // First, use metadata() to check the file type
    let meta = Dir::open("/")
        .unwrap()
        .metadata(path, LookupFlags::IN_ROOT)
        .unwrap();
    assert_eq!(meta.file_type(), ftype);

    // Then iterate through the parent directory and make sure the file type matches
    let fname = path.file_name().unwrap();
    let mut found = false;
    for entry in Dir::open(path.parent().unwrap())
        .unwrap()
        .list_self()
        .unwrap()
    {
        let entry = entry.unwrap();
        if fname == entry.name() {
            if let Some(entry_ftype) = entry.file_type() {
                assert_eq!(entry_ftype, ftype);
            }
            found = true;
            break;
        }
    }
    assert!(found);
}
