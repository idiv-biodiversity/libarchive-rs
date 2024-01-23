use std::convert::TryInto;
use std::ffi::{c_void, CString};
use std::fs::{self, File};
use std::io::Read;
use std::os::raw::c_char;
use std::path::Path;

use libarchive_sys as ffi;

use crate::Entry;
use crate::Error;
use crate::Result;

pub struct Archive {
    underlying: *mut ffi::archive,
    block_size: usize,
    close_read: bool,
}

impl Archive {
    pub const DEFAULT_BLOCK_SIZE: usize = 65536;

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Archive> {
        let path = path.as_ref();

        let file = path.to_string_lossy();
        let file = CString::new(file.as_bytes()).unwrap();

        let block_size: usize = if cfg!(unix) {
            use std::os::unix::fs::MetadataExt;
            let meta = fs::metadata(path)?;
            let block_size = meta.blksize();
            block_size.try_into().unwrap_or(Archive::DEFAULT_BLOCK_SIZE)
        } else {
            Archive::DEFAULT_BLOCK_SIZE
        };

        Archive::open_filename(file.as_ptr(), block_size)
    }

    pub fn stdin() -> Result<Archive> {
        Archive::open_filename(std::ptr::null(), Archive::DEFAULT_BLOCK_SIZE)
    }

    pub fn create<P: AsRef<Path>>(path: P) -> Result<Archive> {
        let path = path.as_ref();

        // TODO conversion with From?
        let file = path.to_string_lossy();
        let file = CString::new(file.as_bytes()).unwrap();

        // TODO block size
        let block_size = Archive::DEFAULT_BLOCK_SIZE;

        unsafe {
            let archive = ffi::archive_write_new();

            if archive.is_null() {
                return Err(Error::new("archive allocation error"));
            }

            match ffi::archive_write_set_format_filter_by_ext(
                archive,
                file.as_ptr(),
            ) {
                ffi::fix::ARCHIVE_OK => (),
                _ => return Err(Error::from_archive(archive)),
            }

            let result =
                ffi::archive_write_open_filename(archive, file.as_ptr());

            match result {
                ffi::fix::ARCHIVE_OK => {
                    let archive = Archive {
                        underlying: archive,
                        block_size,
                        close_read: false,
                    };

                    Ok(archive)
                }

                _ => Err(Error::from_archive(archive)),
            }
        }
    }

    fn open_filename(
        path: *const c_char,
        block_size: usize,
    ) -> Result<Archive> {
        unsafe {
            let archive = ffi::archive_read_new();

            if archive.is_null() {
                return Err(Error::new("archive allocation error"));
            }

            ffi::archive_read_support_filter_all(archive);
            ffi::archive_read_support_format_all(archive);

            match ffi::archive_read_open_filename(archive, path, block_size) {
                ffi::fix::ARCHIVE_OK => {
                    let archive = Archive {
                        underlying: archive,
                        block_size,
                        close_read: true,
                    };

                    Ok(archive)
                }

                _ => Err(Error::from_archive(archive)),
            }
        }
    }

    pub fn append_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        file: &mut File,
    ) -> Result<()> {
        let path = path.as_ref();

        let path = path.to_string_lossy();
        let path = CString::new(path.as_bytes()).unwrap();

        unsafe {
            let entry = ffi::archive_entry_new();

            let meta = file.metadata()?;

            // ffi::archive_entry_copy_stat(entry, &stat);
            ffi::archive_entry_set_filetype(entry, ffi::AE_IFREG);
            ffi::archive_entry_set_pathname(entry, path.as_ptr());

            match ffi::archive_write_header(self.underlying, entry) {
                ffi::fix::ARCHIVE_OK => (),
                _ => return Err(Error::from_archive(self.underlying)),
            }

            let block_size: usize = if cfg!(unix) {
                use std::os::unix::fs::MetadataExt;
                let block_size = meta.blksize();
                block_size.try_into().unwrap_or(Archive::DEFAULT_BLOCK_SIZE)
            } else {
                Archive::DEFAULT_BLOCK_SIZE
            };

            let mut buf = vec![0; block_size];

            loop {
                let nbytes = file.read(&mut buf)?;

                if nbytes > 0 {
                    ffi::archive_write_data(
                        self.underlying,
                        buf.as_ptr() as *mut c_void,
                        nbytes,
                    );
                } else {
                    break;
                }
            }

            ffi::archive_entry_free(entry);
        }

        Ok(())
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn entries(self) -> Entries {
        Entries::new(self)
    }
}

impl Drop for Archive {
    fn drop(&mut self) {
        if self.close_read {
            unsafe {
                ffi::archive_read_free(self.underlying);
            }
        } else {
            unsafe {
                ffi::archive_write_free(self.underlying);
            }
        }
    }
}

impl IntoIterator for Archive {
    type Item = Entry;
    type IntoIter = Entries;

    fn into_iter(self) -> Entries {
        self.entries()
    }
}

pub struct Entries {
    archive: Archive,
    current: *mut ffi::archive_entry,
}

impl Entries {
    fn new(archive: Archive) -> Entries {
        Entries {
            archive,
            current: std::ptr::null_mut(),
        }
    }
}

impl Iterator for Entries {
    type Item = Entry;

    fn next(&mut self) -> Option<Entry> {
        unsafe {
            let result = ffi::archive_read_next_header(
                self.archive.underlying,
                &mut self.current,
            );

            match result {
                0 => {
                    let entry = Entry {
                        archive: self.archive.underlying,
                        underlying: self.current,
                    };
                    Some(entry)
                }

                _ => None,
            }
        }
    }
}

// ----------------------------------------------------------------------------
// tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use assert_cmd::prelude::*;
    use assert_fs::prelude::*;
    use predicates::prelude::*;
    use std::process::Command;

    #[test]
    fn archive_read_entries() {
        let temp = assert_fs::TempDir::new().unwrap();

        let source = temp.child("src");
        source.create_dir_all().unwrap();

        source.child("foo").write_str("foo\n").unwrap();
        source.child("bar").write_str("bar\n").unwrap();
        source.child("baz").write_str("baz\n").unwrap();

        let tarball = temp.path().join("src.tar.gz");

        let mut cmd = Command::new("bsdtar");
        cmd.arg("-C").arg(temp.path());
        cmd.arg("-czf").arg(&tarball);
        cmd.arg("src");
        cmd.assert().success();

        let archive = Archive::open(&tarball).unwrap();
        let entries: Vec<String> =
            archive.entries().map(|entry| entry.path()).collect();

        assert_eq!(4, entries.len());
        assert!(entries.iter().any(|path| path == "src/"));
        assert!(entries.iter().any(|path| path == "src/foo"));
        assert!(entries.iter().any(|path| path == "src/bar"));
        assert!(entries.iter().any(|path| path == "src/baz"));

        temp.close().unwrap();
    }

    #[test]
    fn archive_append_file() {
        let temp = assert_fs::TempDir::new().unwrap();

        let source = temp.child("src");
        source.create_dir_all().unwrap();

        let foo = source.child("foo");
        let bar = source.child("bar");
        let baz = source.child("baz");

        foo.write_str("foo\n").unwrap();
        bar.write_str("bar\n").unwrap();
        baz.write_str("baz\n").unwrap();

        let tarball = temp.path().join("src.tar.gz");

        let mut archive = Archive::create(&tarball).unwrap();

        let mut foo = File::open(foo.path()).unwrap();
        let mut bar = File::open(bar.path()).unwrap();
        let mut baz = File::open(baz.path()).unwrap();

        archive.append_file("src/foo", &mut foo).unwrap();
        archive.append_file("src/bar", &mut bar).unwrap();
        archive.append_file("src/baz", &mut baz).unwrap();

        drop(archive);

        // TODO remove
        Command::new("cp")
            .arg(&tarball)
            .arg("/tmp/foo.tar.gz")
            .assert()
            .success();

        Command::new("bsdtar")
            .arg("-tzf")
            .arg(&tarball)
            .assert()
            .success()
            .stdout(predicate::str::contains("src/foo"))
            .stdout(predicate::str::contains("src/bar"))
            .stdout(predicate::str::contains("src/baz"));

        temp.close().unwrap();
    }
}
