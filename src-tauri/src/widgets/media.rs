use anyhow::Result;
use serde::Serialize;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct MediaState {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub has_session: bool,
}

pub fn current() -> Result<MediaState> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?
        .get()?;
    let session = match manager.GetCurrentSession() {
        Ok(s) => s,
        Err(_) => return Ok(MediaState::default()),
    };
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    let info = session.GetPlaybackInfo()?;
    Ok(MediaState {
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        playing: info.PlaybackStatus()? == Status::Playing,
        has_session: true,
    })
}
