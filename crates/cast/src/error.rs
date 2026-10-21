use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The speaker hung up, stopped answering or could not be reached.
    #[error("{0}")]
    Lost(String),
    /// The speaker answered, but said no.
    #[error("{0}")]
    Refused(String),
    /// The speaker answered with something this crate does not understand.
    #[error("unexpected reply from the speaker: {0}")]
    Protocol(String),
    #[error("discovery error: {0}")]
    Discovery(String),
}

impl Error {
    /// Whether the speaker is gone, rather than having refused one request. Casting cannot go on.
    pub fn is_lost(&self) -> bool {
        match self {
            Self::Lost(_) => true,
            Self::Io(error) => matches!(
                error.kind(),
                io::ErrorKind::TimedOut
                    | io::ErrorKind::WouldBlock
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::UnexpectedEof
            ),
            _ => false,
        }
    }
}

impl From<mdns_sd::Error> for Error {
    fn from(error: mdns_sd::Error) -> Self {
        Self::Discovery(error.to_string())
    }
}

impl From<unsub_remote::Error> for Error {
    fn from(error: unsub_remote::Error) -> Self {
        Self::Discovery(error.to_string())
    }
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(code) => Self::Protocol(format!("HTTP status {code}")),
            other => Self::Lost(format!("can't reach the speaker: {other}")),
        }
    }
}
