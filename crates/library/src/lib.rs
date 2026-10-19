mod artwork;
mod error;
mod library;
mod matching;
mod model;
mod normalize;
mod query;
mod scan;
mod schema;
mod store;
mod tags;

pub use error::{Error, Result};
pub use library::Library;
pub use model::{
    Artwork, ExternalTrack, LibraryStats, MatchQuality, MediaKind, ScanProgress, ScanSummary,
    SortKey, Track, TrackMatch, TrackQuery,
};
