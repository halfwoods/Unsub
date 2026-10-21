//! Playing to speakers on the local network: Chromecasts (Google Cast), Sonos and other UPnP / DLNA
//! renderers.
//!
//! The crate finds speakers ([`browse`]), connects to one ([`connect`]) and drives it through the
//! [`Receiver`] trait: a speaker is given a link and fetches the audio itself. [`Links`] holds the
//! secrets those links are made of. A [`Driver`] runs one speaker for as long as it is chosen:
//! it loads each song, watches where the speaker has got to and moves on to the next, and says what
//! happens as [`Event`]s.
//!
//! The crate knows nothing about the library, the audio engine or the web server that serves the
//! links; the host supplies what a song is ([`Provider`]) and where it is served from.

#![forbid(unsafe_code)]

mod chromecast;
mod device;
mod discovery;
mod driver;
mod error;
mod lan;
mod links;
mod media;
mod upnp;

pub use device::{Device, Kind, MAX_DEPTH, MAX_RATE};
pub use discovery::{Browser, browse};
pub use driver::{Command, Driver, Event, Handle, Provider, Snapshot, spawn};
pub use error::{Error, Result};
pub use lan::local_address_toward;
pub use links::{LIFETIME, Links};
pub use media::{AudioFormat, Codec, Media, Receiver, State, Status};

use device::Endpoint;

/// Connects to a speaker. This starts the player app on a Chromecast, which takes a few seconds.
pub fn connect(device: &Device) -> Result<Box<dyn Receiver>> {
    match &device.endpoint {
        Endpoint::Chromecast { port } => {
            Ok(Box::new(chromecast::connect_to(device.address, *port)?))
        }
        Endpoint::Upnp(endpoint) => Ok(Box::new(upnp::Renderer::connect(endpoint)?)),
    }
}
