//! Export original iPod videos to a user-selected folder, independently of the audio library.
use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::jobs::{self, Kind};
use crate::sync::SYNC_EVENT;
use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Manager};
use unsub_ipod::video;

fn error(e: impl std::fmt::Display) -> CommandError {
    CommandError::from(e.to_string())
}

#[tauri::command]
pub async fn device_videos(app: AppHandle, device_id: String) -> CommandResult<video::Catalog> {
    tauri::async_runtime::spawn_blocking(move || {
        let ipod = app.state::<DevicesState>().ipod(&device_id)?;
        video::list(&ipod).map_err(error)
    })
    .await
    .map_err(error)?
}

#[tauri::command]
pub async fn device_video_export_preview(
    app: AppHandle,
    device_id: String,
    ids: Vec<String>,
    destination: String,
) -> CommandResult<video::Preview> {
    tauri::async_runtime::spawn_blocking(move || {
        let ipod = app.state::<DevicesState>().ipod(&device_id)?;
        let catalog = video::list(&ipod).map_err(error)?;
        video::preview(&ipod, &catalog, &ids, Path::new(&destination)).map_err(error)
    })
    .await
    .map_err(error)?
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    device_id: String,
    stage: &'static str,
    done: usize,
    total: usize,
    current: String,
}

#[tauri::command]
pub fn device_video_export(app: AppHandle, device_id: String, ids: Vec<String>, destination: String) -> CommandResult<()> {
    let status = Status { device_id: device_id.clone(), stage: "exportingVideos", done: 0, total: 0, current: "Reading videos…".into() };
    let job = jobs::Job::new(Kind::Sync, SYNC_EVENT).resource(&device_id).starting(status);
    jobs::start(&app, job, move |app, ctx| {
        let run = || -> CommandResult<video::Report> {
            let ipod = app.state::<DevicesState>().ipod(&device_id)?;
            let catalog = video::list(&ipod).map_err(error)?;
            video::export(&ipod, &catalog, &ids, Path::new(&destination), ctx.cancel_flag(), &mut |done, total, title, bytes, size| {
                ctx.progress(
                    bytes,
                    Some(size),
                    (done, title),
                    Status { device_id: device_id.clone(), stage: "exportingVideos", done, total, current: title.into() },
                );
            })
            .map_err(error)
        };
        ctx.notice(match run() {
            Ok(report) => {
                let count = |status| report.items.iter().filter(|item| item.status == status).count();
                format!("{} video(s) copied; {} verified existing copies; {} not copied. {}An export report was saved beside the videos. Protected videos remain protected.", count("copied"), count("skipped"), count("failed") + count("notCopied"), if report.disconnected { "The iPod disconnected. " } else if report.stopped { "Export stopped. " } else { "" })
            },
            Err(e) => format!("Video export failed: {e}"),
        });
    })
}
