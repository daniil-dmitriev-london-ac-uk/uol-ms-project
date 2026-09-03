use std::fmt;

#[derive(Debug)]
pub enum HeapError {
    Io(std::io::Error),

    Corrupt { what: &'static str, offset: u64 },

    NotFound(u64),
    InvalidArg(&'static str),
}

pub type Result<T> = std::result::Result<T, HeapError>;

impl From<std::io::Error> for HeapError {
    fn from(e: std::io::Error) -> Self {
        HeapError::Io(e)
    }
}

impl fmt::Display for HeapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeapError::Io(e) => write!(f, "io error: {e}"),
            HeapError::Corrupt { what, offset } => write!(f, "corrupt {what} at offset {offset}"),
            HeapError::NotFound(id) => write!(f, "record {id} not found"),
            HeapError::InvalidArg(m) => write!(f, "invalid argument: {m}"),
        }
    }
}

impl std::error::Error for HeapError {}
