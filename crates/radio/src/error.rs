use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not reach {url}: {detail}")]
    Network { url: String, detail: String },
    #[error("{url} answered {status}")]
    Http { url: String, status: u16 },
    #[error("too many redirects from {0}")]
    TooManyRedirects(String),
    #[error("not a usable stream address: {0}")]
    InvalidUrl(String),
    /// The directory (or a playlist) said something that could not be understood.
    #[error("unexpected {what}: {detail}")]
    Response { what: &'static str, detail: String },
    #[error("no Radio Browser server answered: {0}")]
    NoMirror(String),
    #[error("{0} is not an audio stream or a playlist of one")]
    NotAStream(String),
    #[error("could not tell what kind of audio {0} carries")]
    UnknownFormat(String),
    /// A valid stream this crate does not play, such as SAMPLE-AES encrypted HLS.
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("HLS: {0}")]
    Hls(String),
    #[error("the stream went quiet")]
    Stalled,
    #[error("cancelled")]
    Cancelled,
}

impl Error {
    pub(crate) fn network(url: impl ToString, detail: impl ToString) -> Self {
        Self::Network {
            url: url.to_string(),
            detail: detail.to_string(),
        }
    }

    /// How a failure looks to a `Read` caller.
    pub(crate) fn into_io(self) -> io::Error {
        let kind = match self {
            Self::Stalled => io::ErrorKind::TimedOut,
            // Not `Interrupted`, which std's helpers would quietly retry.
            Self::Cancelled => io::ErrorKind::Other,
            Self::Network { .. } | Self::Http { .. } | Self::TooManyRedirects(_) => {
                io::ErrorKind::ConnectionAborted
            }
            _ => io::ErrorKind::InvalidData,
        };
        io::Error::new(kind, self)
    }
}

/// Whether an `io::Error` is a [`Error::Cancelled`] that went through a `Read`.
pub(crate) fn is_cancelled(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<Error>())
        .is_some_and(|inner| matches!(inner, Error::Cancelled))
}
