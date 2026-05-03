use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionMediaProperties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

/// Re-extract the thumbnail this often even when the track signature is
/// unchanged. Catches sources that swap artwork mid-track (Spotify
/// occasionally lazily replaces the cover, browser tabs change favicons,
/// some videos cycle keyframes) without re-doing the expensive async
/// stream read every snapshot tick.
const THUMBNAIL_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct MediaState {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub has_session: bool,
    /// Album art / video thumbnail encoded as a base64 data URL. Cached per
    /// track signature (title|artist) so we don't re-extract every probe
    /// tick — thumbnail extraction does an async stream read which would
    /// otherwise dominate the snapshot loop.
    pub thumbnail: Option<String>,
}

pub fn current() -> Result<MediaState> {
    let session = match current_session() {
        Ok(s) => s,
        Err(_) => return Ok(MediaState::default()),
    };
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    let info = session.GetPlaybackInfo()?;
    let title = props.Title()?.to_string();
    let artist = props.Artist()?.to_string();

    let sig = format!("{title}\u{1F}{artist}");
    let mut cache = thumb_cache().lock().unwrap();
    let stale = cache.2.elapsed() > THUMBNAIL_TTL;
    if cache.0 != sig || stale {
        cache.0 = sig;
        cache.1 = extract_thumbnail(&props);
        cache.2 = Instant::now();
    }
    let thumbnail = cache.1.clone();

    Ok(MediaState {
        title,
        artist,
        playing: info.PlaybackStatus()? == Status::Playing,
        has_session: true,
        thumbnail,
    })
}

/// (track signature, last extracted thumbnail data URL, when extracted).
/// `Instant::now()` for the initial value just keeps the type happy — the
/// empty signature ensures the first read always re-extracts anyway.
fn thumb_cache() -> &'static Mutex<(String, Option<String>, Instant)> {
    static SLOT: OnceLock<Mutex<(String, Option<String>, Instant)>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new((String::new(), None, Instant::now())))
}

/// Pull the SMTC thumbnail (Spotify cover, browser tab favicon, video
/// keyframe — whatever the source registers). Returns a data URL ready to
/// drop into <img src=>. Returns None when the session has no thumbnail or
/// the async read fails for any reason — caller treats that as "use a
/// fallback glyph."
fn extract_thumbnail(
    props: &GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<String> {
    use base64::Engine;
    use windows::Storage::Streams::{Buffer, DataReader, InputStreamOptions};

    let thumb_ref = props.Thumbnail().ok()?;
    let stream = thumb_ref.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    // Sanity bound — anything larger than ~5 MB is almost certainly bogus
    // and not worth shipping through Tauri's IPC every snapshot.
    if size == 0 || size > 5_000_000 {
        return None;
    }
    let buffer = Buffer::Create(size).ok()?;
    let read_buffer = stream
        .ReadAsync(&buffer, size, InputStreamOptions::None)
        .ok()?
        .get()
        .ok()?;
    let length = read_buffer.Length().ok()? as usize;
    let reader = DataReader::FromBuffer(&read_buffer).ok()?;
    let mut bytes = vec![0u8; length];
    reader.ReadBytes(&mut bytes).ok()?;

    // Sniff the MIME from the first bytes so the data URL renders cleanly
    // in the dock without us guessing.
    let mime = if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        // Fallback — most SMTC sources hand out JPEG, so guessing JPEG when
        // unknown is more right than not.
        "image/jpeg"
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{mime};base64,{b64}"))
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
