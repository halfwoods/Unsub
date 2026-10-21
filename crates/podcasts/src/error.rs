pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("http error: {0}")]
    Http(#[from] ureq::Error),
    #[error("unexpected http status {status} from {url}")]
    Status { status: u16, url: String },
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid podcast feed: {0}")]
    Parse(String),
    #[error("invalid OPML: {0}")]
    Opml(String),
    #[error("invalid feed URL: {0}")]
    InvalidUrl(String),
    #[error("unexpected reply from the podcast directory: {0}")]
    Directory(String),
    #[error(
        "Unsub couldn't find the Podcasts app's library. Open the Podcasts app once, then try again."
    )]
    PodcastsAppMissing,
    #[error(
        "Unsub isn't allowed to read the Podcasts app's library. Turn Unsub on in System Settings › Privacy & Security › Full Disk Access, then try again."
    )]
    PodcastsAppDenied,
    #[error(
        "the Podcasts app's library is laid out in a way this version of Unsub doesn't recognize"
    )]
    PodcastsAppLayout,
    #[error("feed {0} not found")]
    FeedNotFound(i64),
    #[error("episode {0} not found")]
    EpisodeNotFound(i64),
    #[error("download cancelled")]
    Cancelled,
    #[error("download stalled: the server stopped sending data")]
    Stalled,
    #[error("database schema version {0} was written by a newer version of Unsub")]
    UnsupportedSchema(usize),
}
