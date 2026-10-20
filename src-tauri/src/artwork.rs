use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager};

use crate::devices::DevicesState;
use crate::library::LibraryState;

/// Serves `artwork://localhost/<track id>` and `artwork://localhost/device/<device id>/<track id>`
/// so <img> tags load cover art straight from the library or a connected iPod.
pub fn response(app: &AppHandle, path: &str, query: Option<&str>) -> Response<Vec<u8>> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    // `?size=N` asks for a thumbnail: Grid and Cover Flow never need multi-megabyte originals.
    let size = query.and_then(|q| q.strip_prefix("size=")).and_then(|n| n.parse().ok());
    let artwork = match parts.as_slice() {
        [track] => track.parse().ok().and_then(|id| app.state::<LibraryState>().artwork(id, size)),
        ["device", device_id, track] => track.parse().ok().and_then(|id| app.state::<DevicesState>().artwork(device_id, id)),
        _ => None,
    };
    let response = match artwork {
        Some(art) => Response::builder().header("Content-Type", art.mime).header("Cache-Control", "max-age=3600").body(art.bytes),
        None => Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()),
    };
    response.unwrap_or_else(|_| Response::new(Vec::new()))
}
