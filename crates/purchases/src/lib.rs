//! Closes the loop on buying music. A download from Bandcamp, Qobuz, 7digital or HDtracks lands
//! in a watched folder; `Inbox` notices it, offers it, and on acceptance unpacks it into the
//! library folder. `purchase_links` finds where an album can be bought in the first place.

mod error;
mod extract;
mod files;
mod guess;
mod ledger;
mod links;
mod model;
mod scan;
mod tags;
mod watch;

pub use error::{Error, Result};
pub use extract::unpack;
pub use ledger::Inbox;
pub use links::purchase_links;
pub use model::{Candidate, Purchase, Store, StoreLink};
pub use watch::{WatchHandle, watch};
