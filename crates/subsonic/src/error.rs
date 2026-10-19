use std::io;

pub type Result<T> = std::result::Result<T, Error>;

/// Every failure the crate reports, both to the host and (via [`Error::code`]) to Subsonic clients.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not start the server: {0}")]
    Start(#[source] io::Error),
    #[error("invalid server configuration: {0}")]
    InvalidConfig(&'static str),
    /// For the host's [`MusicSource`](crate::MusicSource) to report a failure it cannot recover from.
    #[error("{0}")]
    Source(String),
    #[error("Required parameter is missing: {0}")]
    MissingParameter(&'static str),
    #[error("Invalid value for parameter: {0}")]
    InvalidParameter(&'static str),
    #[error("Wrong username or password")]
    WrongCredentials,
    #[error("Provided authentication mechanism not supported")]
    UnsupportedAuth,
    #[error("Multiple conflicting authentication mechanisms provided")]
    ConflictingAuth,
    #[error("Invalid API key")]
    InvalidApiKey,
    #[error("User is not authorized for the given operation")]
    NotAuthorized,
    #[error("The requested data was not found")]
    NotFound,
    #[error("Unsupported endpoint: {0}")]
    UnsupportedEndpoint(String),
    #[error("i/o error: {0}")]
    Io(io::Error),
    #[error("could not serialize the response: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl Error {
    /// The Subsonic / OpenSubsonic error code clients expect for this failure.
    pub fn code(&self) -> u32 {
        match self {
            Self::MissingParameter(_) => 10,
            Self::WrongCredentials => 40,
            Self::UnsupportedAuth => 42,
            Self::ConflictingAuth => 43,
            Self::InvalidApiKey => 44,
            Self::NotAuthorized => 50,
            Self::NotFound => 70,
            Self::Start(_)
            | Self::InvalidConfig(_)
            | Self::Source(_)
            | Self::InvalidParameter(_)
            | Self::UnsupportedEndpoint(_)
            | Self::Io(_)
            | Self::Serialize(_) => 0,
        }
    }
}

impl From<io::Error> for Error {
    /// A file that vanished from disk is "not found" to the client, not a server fault.
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::NotFound {
            Self::NotFound
        } else {
            Self::Io(err)
        }
    }
}
