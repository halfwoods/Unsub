//! The macOS system library, read through Apple's public iTunesLibrary.framework. No generated
//! bindings exist for it, so the classes are declared here straight from the SDK headers
//! (ITLibrary.h, ITLibMediaItem.h, ITLibMediaEntity.h, ITLibAlbum.h, ITLibArtist.h). This module
//! is the crate's only unsafe surface.

use std::path::PathBuf;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::NSObject;
use objc2::{extern_class, extern_methods};
use objc2_foundation::{NSArray, NSDate, NSError, NSNumber, NSString, NSURL};

use crate::classify::{Evidence, classify};
use crate::error::{Error, Result};
use crate::item::{MediaType, MusicLibraryItem};

#[link(name = "iTunesLibrary", kind = "framework")]
unsafe extern "C" {}

const API_VERSION: &str = "1.1";

// Raw values of ITLibMediaItemMediaKind and ITLibMediaItemLocationType in ITLibMediaItem.h.
const MEDIA_KIND_SONG: usize = 2;
const MEDIA_KIND_MOVIE: usize = 3;
const MEDIA_KIND_PODCAST: usize = 4;
const MEDIA_KIND_AUDIOBOOK: usize = 5;
const MEDIA_KIND_MUSIC_VIDEO: usize = 7;
const MEDIA_KIND_TV_SHOW: usize = 8;
const MEDIA_KIND_HOME_VIDEO: usize = 12;
const LOCATION_TYPE_FILE: usize = 1;
const LOCATION_TYPE_REMOTE: usize = 3;

extern_class!(
    // SAFETY: ITLibrary.h declares `ITLibrary : NSObject`.
    #[unsafe(super(NSObject))]
    struct ITLibrary;
);

extern_class!(
    // SAFETY: ITLibMediaEntity.h declares `ITLibMediaEntity : NSObject`.
    #[unsafe(super(NSObject))]
    struct ITLibMediaEntity;
);

extern_class!(
    // SAFETY: ITLibMediaItem.h declares `ITLibMediaItem : ITLibMediaEntity`.
    #[unsafe(super(ITLibMediaEntity, NSObject))]
    struct ITLibMediaItem;
);

extern_class!(
    // SAFETY: ITLibAlbum.h declares `ITLibAlbum : NSObject`.
    #[unsafe(super(NSObject))]
    struct ITLibAlbum;
);

extern_class!(
    // SAFETY: ITLibArtist.h declares `ITLibArtist : NSObject`.
    #[unsafe(super(NSObject))]
    struct ITLibArtist;
);

impl ITLibrary {
    extern_methods!(
        // SAFETY: selectors, argument types and nullability are copied from ITLibrary.h; the
        // factory returns nil and fills in the NSError when it fails.
        #[unsafe(method(libraryWithAPIVersion:error:_))]
        fn with_api_version(
            version: &NSString,
        ) -> std::result::Result<Retained<Self>, Retained<NSError>>;

        #[unsafe(method(allMediaItems))]
        fn all_media_items(&self) -> Retained<NSArray<ITLibMediaItem>>;
    );
}

impl ITLibMediaEntity {
    extern_methods!(
        // SAFETY: selector and type are copied from ITLibMediaEntity.h (`NSNumber *`, nonnull).
        #[unsafe(method(persistentID))]
        fn persistent_id(&self) -> Retained<NSNumber>;
    );
}

impl ITLibMediaItem {
    extern_methods!(
        // SAFETY: selectors, types and nullability are copied from ITLibMediaItem.h; NSUInteger
        // and NSInteger map to usize and isize, and BOOL to bool.
        #[unsafe(method(title))]
        fn title(&self) -> Retained<NSString>;

        #[unsafe(method(artist))]
        fn artist(&self) -> Option<Retained<ITLibArtist>>;

        #[unsafe(method(album))]
        fn album(&self) -> Retained<ITLibAlbum>;

        #[unsafe(method(genre))]
        fn genre(&self) -> Retained<NSString>;

        #[unsafe(method(kind))]
        fn kind(&self) -> Option<Retained<NSString>>;

        #[unsafe(method(mediaKind))]
        fn media_kind(&self) -> usize;

        #[unsafe(method(totalTime))]
        fn total_time(&self) -> usize;

        #[unsafe(method(trackNumber))]
        fn track_number(&self) -> usize;

        #[unsafe(method(year))]
        fn year(&self) -> usize;

        #[unsafe(method(rating))]
        fn rating(&self) -> isize;

        #[unsafe(method(playCount))]
        fn play_count(&self) -> usize;

        #[unsafe(method(addedDate))]
        fn added_date(&self) -> Option<Retained<NSDate>>;

        #[unsafe(method(location))]
        fn location(&self) -> Option<Retained<NSURL>>;

        #[unsafe(method(locationType))]
        fn location_type(&self) -> usize;

        #[unsafe(method(isPurchased))]
        fn is_purchased(&self) -> bool;

        #[unsafe(method(isCloud))]
        fn is_cloud(&self) -> bool;

        #[unsafe(method(isDRMProtected))]
        fn is_drm_protected(&self) -> bool;
    );
}

impl ITLibAlbum {
    extern_methods!(
        // SAFETY: selectors, types and nullability are copied from ITLibAlbum.h.
        #[unsafe(method(title))]
        fn title(&self) -> Option<Retained<NSString>>;

        #[unsafe(method(albumArtist))]
        fn album_artist(&self) -> Option<Retained<NSString>>;

        #[unsafe(method(discNumber))]
        fn disc_number(&self) -> usize;
    );
}

impl ITLibArtist {
    extern_methods!(
        // SAFETY: selector and type are copied from ITLibArtist.h.
        #[unsafe(method(name))]
        fn name(&self) -> Option<Retained<NSString>>;
    );
}

/// Reads the whole system library. The first call prompts the user for "Media & Apple Music"
/// access; declining it, like any other failure to open the library, is `Error::Unavailable`.
pub fn read_system_library() -> Result<Vec<MusicLibraryItem>> {
    // The framework hands back autoreleased objects, and this may run on a thread with no pool of
    // its own, so drain them here instead of leaking them until the thread exits.
    autoreleasepool(|_| {
        let library = ITLibrary::with_api_version(&NSString::from_str(API_VERSION))
            .map_err(|error| Error::Unavailable(error.localizedDescription().to_string()))?;
        Ok(library
            .all_media_items()
            .iter()
            .map(|item| to_item(&item))
            .collect())
    })
}

fn to_item(item: &ITLibMediaItem) -> MusicLibraryItem {
    let album = item.album();
    let kind = text(item.kind());
    let location = item
        .location()
        .filter(|url| url.isFileURL())
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));
    let file_present = location.as_ref().is_some_and(|path| path.is_file());
    let ownership = classify(&Evidence {
        kind: &kind,
        // The framework has no Apple Music flag; only the kind prefix identifies subscription items.
        apple_music: false,
        purchased: item.is_purchased(),
        drm_protected: item.is_drm_protected(),
        in_cloud: item.is_cloud() || item.location_type() == LOCATION_TYPE_REMOTE,
        file_recorded: location.is_some() && item.location_type() == LOCATION_TYPE_FILE,
        file_present,
    });
    MusicLibraryItem {
        persistent_id: format!("{:016X}", item.persistent_id().as_u64()),
        title: item.title().to_string(),
        artist: text(item.artist().and_then(|artist| artist.name())),
        album_artist: text(album.album_artist()),
        album: text(album.title()),
        genre: item.genre().to_string(),
        duration_ms: present(item.total_time()),
        track_number: present(item.track_number()),
        disc_number: present(album.disc_number()),
        year: present(item.year()),
        kind,
        media: media_type(item.media_kind()),
        ownership,
        location,
        file_present,
        play_count: u32::try_from(item.play_count()).unwrap_or(u32::MAX),
        rating: item.rating().clamp(0, 100) as u8,
        date_added: item
            .added_date()
            .map(|date| date.timeIntervalSince1970() as i64),
    }
}

fn media_type(raw: usize) -> MediaType {
    match raw {
        MEDIA_KIND_SONG => MediaType::Music,
        MEDIA_KIND_PODCAST => MediaType::Podcast,
        MEDIA_KIND_AUDIOBOOK => MediaType::Audiobook,
        MEDIA_KIND_MOVIE | MEDIA_KIND_MUSIC_VIDEO | MEDIA_KIND_TV_SHOW | MEDIA_KIND_HOME_VIDEO => {
            MediaType::Video
        }
        _ => MediaType::Other,
    }
}

fn text(string: Option<Retained<NSString>>) -> String {
    string.map(|string| string.to_string()).unwrap_or_default()
}

/// The framework reports a missing number as zero.
fn present<T: TryFrom<usize>>(value: usize) -> Option<T> {
    if value == 0 {
        None
    } else {
        T::try_from(value).ok()
    }
}
