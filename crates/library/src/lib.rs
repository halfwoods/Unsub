mod albums;
mod artwork;
mod error;
mod library;
mod matching;
mod model;
mod normalize;
mod playlist;
mod query;
mod scan;
mod schema;
mod smart;
mod store;
mod tags;

pub use error::{Error, Result};
pub use library::Library;
pub use model::{
    AlbumSummary, ArtistSummary, Artwork, ComposerSummary, ExternalStats, ExternalTrack,
    GenreSummary, LibraryStats, MatchQuality, MediaKind, Playlist, PlaylistKind, ScanProgress,
    ScanSummary, SortKey, Track, TrackMatch, TrackQuery,
};
pub use smart::{
    DateUnit, LimitOrder, LimitUnit, SmartField, SmartLimit, SmartOp, SmartRule, SmartRules,
    SmartValue,
};
