//! Read-only access to the user's Apple Music / iTunes library. Every song is classified by
//! whether the user actually owns it, so a streaming-only track can be bought and an owned file
//! can be imported. Two sources feed the same output: the macOS system library (through Apple's
//! public iTunesLibrary.framework) and an exported library XML.

mod classify;
mod error;
mod item;
mod locate;
#[cfg(target_os = "macos")]
mod macos;
mod xml;

pub use error::{Error, Result};
pub use item::{MediaType, MusicLibrary, MusicLibraryItem, MusicPlaylist, Ownership, PlaylistKind};
pub use locate::{Confidence, LocateProgress, LocateStage, Located, LocatedFile, locate_in_folder};
#[cfg(target_os = "macos")]
pub use macos::{read_system_library, read_system_music_library};
pub use xml::{read_xml, read_xml_library};
