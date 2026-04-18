use std::io::{Read, Seek};

/// A reader that implements both [`Read`] and [`Seek`].
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}
