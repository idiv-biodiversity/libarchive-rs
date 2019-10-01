use libarchive_sys as ffi;
use std::ffi::CStr;
use std::fmt::{self, Display};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ErrorKind {
    Io,
    LibArchive,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Error {
    pub message: String,
    pub kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(message: &str) -> Error {
        Error {
            message: String::from(message),
            kind: ErrorKind::LibArchive,
        }
    }

    pub(crate) fn from_archive(archive: *mut ffi::archive) -> Error {
        unsafe {
            let msg = ffi::archive_error_string(archive);
            let msg = CStr::from_ptr(msg);
            let msg = msg.to_string_lossy();
            Error::new(&msg)
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {
    fn description(&self) -> &str {
        &*self.message
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error {
            message: format!("{}", error),
            kind: ErrorKind::Io,
        }
    }
}
