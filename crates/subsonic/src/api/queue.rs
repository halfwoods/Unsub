//! The play queue clients save so another device can pick up where this one stopped.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{Ctx, Reply, doc};
use crate::{
    Result,
    model::format_iso8601,
    session::PlayQueue,
    wire::{PlayQueueDoc, SongEntry},
};

/// `id` once per song in order, `current` the song playing and `position` how far into it, in
/// milliseconds. Saving a queue with no songs clears it.
pub async fn save(ctx: &Ctx) -> Result<Reply> {
    let ids: Vec<String> = ctx
        .params
        .all("id")
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    let position_ms = ctx.params.number::<i64>("position")?.unwrap_or(0).max(0);
    let queue = (!ids.is_empty()).then(|| PlayQueue {
        ids,
        current: ctx.params.get("current").map(str::to_owned),
        position_ms,
        changed: unix_seconds(),
        changed_by: ctx.params.get("c").unwrap_or("unknown").to_owned(),
    });
    ctx.state.session.save_queue(queue);
    Ok(Reply::Empty)
}

/// Nothing saved is an empty answer, not an error. Songs deleted since are dropped.
pub async fn get(ctx: &Ctx) -> Result<Reply> {
    let Some(queue) = ctx.state.session.queue() else {
        return Ok(Reply::Empty);
    };
    let ids = queue.ids.clone();
    let songs = ctx
        .source(move |source| {
            ids.iter()
                .filter_map(|id| source.song(id).transpose())
                .collect::<Result<Vec<_>>>()
        })
        .await?;
    // A current song that is gone would leave the client pointing at nothing.
    let current = queue
        .current
        .as_deref()
        .filter(|current| songs.iter().any(|song| song.id == *current));
    doc(
        "playQueue",
        &PlayQueueDoc {
            current,
            position: queue.position_ms,
            username: &ctx.state.config.username,
            changed: format_iso8601(queue.changed),
            changed_by: &queue.changed_by,
            entry: SongEntry::all(&songs),
        },
    )
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
