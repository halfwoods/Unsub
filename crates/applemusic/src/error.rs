#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("library export is not a readable property list: {0}")]
    Plist(#[from] plist::Error),
    /// The system library could not be opened, including when the user declines the "Media & Apple
    /// Music" privacy prompt.
    #[error("music library unavailable: {0}")]
    Unavailable(String),
}

pub type Result<T> = std::result::Result<T, Error>;
