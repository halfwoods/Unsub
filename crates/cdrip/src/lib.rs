//! CD ripping: identify a disc through MusicBrainz, rip it to FLAC, ALAC or AAC, check each track
//! against AccurateRip, tag it and attach cover art.
//!
//! ```text
//! discs()  ->  lookup(disc, client)  ->  rip(disc, release, settings, client, progress, cancel)
//! ```
//!
//! Audio comes through a [`DiscReader`]. macOS exposes audio CDs as volumes of AIFF files plus a
//! `.TOC.plist`, which [`VolumeReader`] reads; a Windows reader can be added behind the same trait.
//! The network goes through a [`MetadataClient`] ([`UreqClient`] for real use), so tests and
//! callers can supply their own.

#![deny(unsafe_code)]

mod accuraterip;
mod disc;
mod encode;
mod error;
mod http;
mod musicbrainz;
mod naming;
mod rip;
mod tag;
#[cfg(test)]
mod testing;
mod toc;

pub use accuraterip::AccurateRip;
pub use disc::{
    Disc, DiscReader, DiscTrack, TrackAudio, VolumeReader, discs, eject, system_reader,
};
pub use encode::Format;
pub use error::{Error, Result};
pub use http::{MetadataClient, UreqClient};
pub use musicbrainz::{Release, ReleaseTrack, lookup};
pub use rip::{RipProgress, RipSettings, RippedTrack, rip, rip_with};
pub use toc::{Toc, musicbrainz_disc_id};
