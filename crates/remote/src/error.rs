pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] ureq::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unreadable response: {0}")]
    Json(#[from] serde_json::Error),
    /// The server answered, but not in the Subsonic JSON envelope: usually the wrong URL.
    #[error("not a Subsonic server: {0}")]
    Unexpected(String),
    #[error("wrong username or password")]
    WrongCredentials,
    /// Subsonic error 41: servers that check passwords elsewhere (LDAP) cannot verify a token.
    #[error("the server does not support token authentication")]
    TokenAuthUnsupported,
    #[error("this user is not allowed to do that")]
    NotAuthorized,
    #[error("not found on the server")]
    NotFound,
    /// Any other Subsonic error, with the server's own code and words.
    #[error("server error {code}: {message}")]
    Server { code: u32, message: String },
    #[error("server answered with HTTP status {0}")]
    Status(u16),
    #[error("a {size}-byte download does not fit in the {limit}-byte cache")]
    TooLargeForCache { size: u64, limit: u64 },
    /// A download that failed or stopped; the cause was reported to whoever waited for it.
    #[error("download failed: {0}")]
    DownloadFailed(String),
    #[error("discovery error: {0}")]
    Discovery(#[from] mdns_sd::Error),
}

impl Error {
    /// Maps the code of a Subsonic `error` element to a typed error.
    pub(crate) fn from_subsonic(code: u32, message: String) -> Self {
        match code {
            40 => Self::WrongCredentials,
            41 => Self::TokenAuthUnsupported,
            50 => Self::NotAuthorized,
            70 => Self::NotFound,
            _ => Self::Server { code, message },
        }
    }
}
