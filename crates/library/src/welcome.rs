use crate::error::Result;
use crate::library::Library;
use crate::model::TrackQuery;

const WELCOMED: &str = "welcomed";

impl Library {
    /// Whether the first-run welcome is due: the library has no songs (stations don't count) and the
    /// welcome hasn't been answered. A library that has songs is marked welcomed on the spot, so deleting
    /// them all later can't bring the welcome back.
    pub fn welcome_due(&mut self) -> Result<bool> {
        if self.setting(WELCOMED)?.is_some() {
            return Ok(false);
        }
        let songs = self
            .stats(&TrackQuery {
                kind: None,
                ..TrackQuery::default()
            })?
            .count;
        if songs == 0 {
            return Ok(true);
        }
        self.set_welcomed()?;
        Ok(false)
    }

    /// The user picked an option in the welcome, or skipped it.
    pub fn set_welcomed(&mut self) -> Result<()> {
        self.put_setting(WELCOMED, "1")
    }
}
