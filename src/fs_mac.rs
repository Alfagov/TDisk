//! macOS bulk metadata enumeration: one syscall per batch, without per-file stat calls.
use crate::filesystem::FileNode;
use libc::{
    ATTR_BIT_MAP_COUNT, ATTR_CMN_NAME, ATTR_CMN_OBJTYPE, ATTR_CMN_RETURNED_ATTRS,
    ATTR_FILE_ALLOCSIZE, attrlist,
};
use std::cell::RefCell;
use std::ffi::{OsString, c_void};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::{ffi::OsStringExt, fs::OpenOptionsExt};
use std::path::Path;

const ATTR_CMN_ERROR: u32 = 0x20000000;
const VREG: u32 = 1;
const VDIR: u32 = 2;

thread_local! {
    // u64 backing guarantees the ABI's 8-byte alignment. Reused once per worker,
    // instead of allocating and retaining 128 KiB per recursive directory.
    static BUFFER: RefCell<Vec<u64>> = RefCell::new(vec![0; 128 * 1024 / 8]);
}

pub fn read_directory(path: &Path) -> io::Result<(Vec<FileNode>, u64)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let mut attrs = attrlist {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_ERROR | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_ALLOCSIZE,
        forkattr: 0,
    };
    BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        let mut children = Vec::new();
        let mut errors = 0;
        loop {
            // SAFETY: fd is an open directory; attrs and the aligned buffer remain
            // valid for the call, with the exact writable buffer size supplied.
            let count = unsafe {
                libc::getattrlistbulk(
                    file.as_raw_fd(),
                    &mut attrs as *mut attrlist as *mut c_void,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() * 8,
                    0,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                break;
            }
            // SAFETY: all bytes are initialized; the slice does not outlive the
            // exclusive buffer borrow. Record parsing below bounds-checks all fields.
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8)
            };
            let mut offset = 0;
            for _ in 0..count {
                let length = word(bytes, offset)? as usize;
                if length < 24 {
                    return Err(invalid_record());
                }
                let end = offset.checked_add(length).ok_or_else(invalid_record)?;
                let record = bytes.get(offset..end).ok_or_else(invalid_record)?;
                match parse_record(record)? {
                    Entry::Node(node) => children.push(node),
                    Entry::Error => errors += 1,
                    Entry::Ignored => {}
                }
                offset = end;
            }
        }
        Ok((children, errors))
    })
}

enum Entry {
    Node(FileNode),
    Error,
    Ignored,
}

fn invalid_record() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid getattrlistbulk record")
}

fn word(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset.checked_add(4).ok_or_else(invalid_record)?;
    Ok(u32::from_ne_bytes(
        bytes
            .get(offset..end)
            .ok_or_else(invalid_record)?
            .try_into()
            .unwrap(),
    ))
}

fn parse_record(record: &[u8]) -> io::Result<Entry> {
    let common = word(record, 4)?;
    let file = word(record, 16)?;
    let mut cursor = 24; // length plus attribute_set_t (five u32 masks)
    // ATTR_CMN_ERROR precedes NAME, despite the numeric ordering of their bits.
    // See Apple's getattrlistbulk(2) ABI and example.
    if common & ATTR_CMN_ERROR != 0 {
        let error = word(record, cursor)?;
        cursor += 4;
        if error != 0 {
            return Ok(Entry::Error);
        }
    }
    if common & ATTR_CMN_NAME == 0 {
        return Ok(Entry::Error);
    }
    let name_offset = word(record, cursor)? as i32;
    let name_length = word(record, cursor + 4)? as usize;
    let start = cursor
        .checked_add_signed(name_offset as isize)
        .ok_or_else(invalid_record)?;
    let end = start.checked_add(name_length).ok_or_else(invalid_record)?;
    let name = record.get(start..end).ok_or_else(invalid_record)?;
    let name = name.strip_suffix(&[0]).ok_or_else(invalid_record)?;
    if name.is_empty() || name.contains(&0) || name.contains(&b'/') || name == b"." || name == b".."
    {
        return Err(invalid_record());
    }
    cursor += 8;
    if common & ATTR_CMN_OBJTYPE == 0 {
        return Ok(Entry::Error);
    }
    let kind = word(record, cursor)?;
    cursor += 4;
    let size = match kind {
        VDIR => 0,
        VREG if file & ATTR_FILE_ALLOCSIZE != 0 => {
            let bytes = record.get(cursor..cursor + 8).ok_or_else(invalid_record)?;
            let size = i64::from_ne_bytes(bytes.try_into().unwrap());
            u64::try_from(size).map_err(|_| invalid_record())?
        }
        VREG => return Ok(Entry::Error),
        _ => return Ok(Entry::Ignored), // Do not follow symlinks or open special files.
    };
    Ok(Entry::Node(FileNode::new(
        OsString::from_vec(name.to_vec()),
        size,
        kind == VDIR,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &[u8], error: Option<u32>) -> Vec<u8> {
        let header = 24 + if error.is_some() { 4 } else { 0 };
        let length = header + 8 + 4 + 8 + name.len() + 1;
        let mut record = Vec::new();
        for value in [
            length as u32,
            ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_NAME
                | ATTR_CMN_OBJTYPE
                | if error.is_some() { ATTR_CMN_ERROR } else { 0 },
            0,
            0,
            ATTR_FILE_ALLOCSIZE,
            0,
        ] {
            record.extend(value.to_ne_bytes());
        }
        if let Some(error) = error {
            record.extend(error.to_ne_bytes());
        }
        record.extend(20i32.to_ne_bytes());
        record.extend(((name.len() + 1) as u32).to_ne_bytes());
        record.extend(VREG.to_ne_bytes());
        record.extend(8192i64.to_ne_bytes());
        record.extend(name);
        record.push(0);
        record
    }

    #[test]
    fn parses_error_before_name_and_optional_attributes() {
        for error in [None, Some(0)] {
            let Entry::Node(node) = parse_record(&record(b"file", error)).unwrap() else {
                panic!("missing node")
            };
            assert_eq!(node.name, "file");
            assert_eq!(node.size, 8192);
        }
        assert!(matches!(
            parse_record(&record(b"denied", Some(13))).unwrap(),
            Entry::Error
        ));
    }

    #[test]
    fn preserves_non_utf8_and_rejects_malformed_records() {
        let Entry::Node(node) = parse_record(&record(b"\xff", None)).unwrap() else {
            panic!("missing node")
        };
        assert_eq!(node.name.into_vec(), vec![255]);
        for name in [b"..".as_slice(), b"/escape", b"a\0b", b""] {
            assert!(parse_record(&record(name, None)).is_err());
        }
        let valid = record(b"file", None);
        for length in 0..valid.len() {
            assert!(parse_record(&valid[..length]).is_err());
        }
        let mut invalid = valid.clone();
        invalid[24..28].copy_from_slice(&i32::MIN.to_ne_bytes());
        assert!(parse_record(&invalid).is_err());
        let mut invalid = valid;
        invalid[36..44].copy_from_slice(&(-1i64).to_ne_bytes());
        assert!(parse_record(&invalid).is_err());
    }
}
