use anyhow::{anyhow, Result};
use serde::Serialize;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
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
    let session = match current_session() {
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

fn current_session() -> Result<GlobalSystemMediaTransportControlsSession> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?
        .get()?;
    manager.GetCurrentSession()
        .map_err(|e| anyhow!("no active media session: {e}"))
}

pub fn toggle_play_pause() -> Result<()> {
    let s = current_session()?;
    s.TryTogglePlayPauseAsync()?.get()?;
    Ok(())
}
pub fn next() -> Result<()> {
    let s = current_session()?;
    s.TrySkipNextAsync()?.get()?;
    Ok(())
}
pub fn prev() -> Result<()> {
    let s = current_session()?;
    s.TrySkipPreviousAsync()?.get()?;
    Ok(())
}
