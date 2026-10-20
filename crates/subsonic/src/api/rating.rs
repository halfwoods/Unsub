//! Ratings and stars. What starring means is the library's business; see
//! [`MusicSource::starred`](crate::MusicSource::starred).

use super::{Ctx, Reply};
use crate::{Error, Result, Targets};

/// `rating` is 1 to 5 stars, or 0 to remove the rating.
pub async fn set_rating(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let stars: u8 = ctx
        .params
        .number("rating")?
        .ok_or(Error::MissingParameter("rating"))?;
    if stars > 5 {
        return Err(Error::InvalidParameter("rating"));
    }
    ctx.source(move |source| source.set_rating(&id, stars))
        .await?;
    Ok(Reply::Empty)
}

/// `star` and `unstar` take songs as `id`, albums as `albumId` and artists as `artistId`, each as
/// often as needed.
pub async fn star(ctx: &Ctx, starring: bool) -> Result<Reply> {
    let named = |name: &str| -> Vec<String> {
        ctx.params
            .all(name)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let targets = Targets {
        songs: named("id"),
        albums: named("albumId"),
        artists: named("artistId"),
    };
    if targets == Targets::default() {
        return Err(Error::MissingParameter("id"));
    }
    ctx.source(move |source| {
        if starring {
            source.star(&targets)
        } else {
            source.unstar(&targets)
        }
    })
    .await?;
    Ok(Reply::Empty)
}
