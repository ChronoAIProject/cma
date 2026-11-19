use std::io::{self, Cursor};

mod sealed {
    pub trait Sealed {}
}

/// Selects the upstream parser used by the codec receiving the admitted bytes.
pub trait HeaderParser: sealed::Sealed {
    type Header;
    fn parse(bytes: &[u8]) -> io::Result<Option<(Self::Header, u64)>>;
}

pub struct Tungstenite024;
pub struct Tungstenite029;

impl sealed::Sealed for Tungstenite024 {}
impl sealed::Sealed for Tungstenite029 {}

impl HeaderParser for Tungstenite024 {
    type Header = tokio_tungstenite::tungstenite::protocol::frame::FrameHeader;

    fn parse(bytes: &[u8]) -> io::Result<Option<(Self::Header, u64)>> {
        Self::Header::parse(&mut Cursor::new(bytes))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

impl HeaderParser for Tungstenite029 {
    type Header = tokio_tungstenite_029::tungstenite::protocol::frame::FrameHeader;

    fn parse(bytes: &[u8]) -> io::Result<Option<(Self::Header, u64)>> {
        Self::Header::parse(&mut Cursor::new(bytes))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}
