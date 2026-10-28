//! What the computer says is playing: on Windows, the system media session (the media
//! transport controls that Spotify, browsers and media players report to) gives the
//! title, artist, album, whether it's playing, and the cover art. The calls block, so a
//! thread reads them every `POLL_S` and hands each change to the app (`Media::poll`).
//! Elsewhere nothing is reported.
//!
//! The cover can arrive a moment after the title (Spotify updates them separately), so
//! for a few reads after a new song the art is read again until it differs from the last
//! song's.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// How often the session is read, seconds.
#[cfg(windows)]
const POLL_S: f32 = 1.0;
/// Cover art is scaled to fit this many pixels square.
#[cfg(windows)]
const ART_SIZE: u32 = 256;
/// How many reads after a new song the art is looked for again.
#[cfg(windows)]
const ART_RETRIES: u32 = 4;

/// A song as the session reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub art: Option<Art>,
}

impl Song {
    /// Whether `other` is the same song (by title, artist and album; not whether it's
    /// playing or its art yet).
    pub fn same(&self, other: &Song) -> bool {
        (&self.title, &self.artist, &self.album) == (&other.title, &other.artist, &other.album)
    }
}

/// Cover art: RGBA, 8 bits a channel, `size` pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Art {
    pub size: [u32; 2],
    pub rgba: Arc<Vec<u8>>,
}

/// What changed, for the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A different song (or the first).
    NewSong(Song),
    /// The same song, now playing or paused, or its art arrived.
    Updated(Song),
    /// Nothing reports playing any more.
    Gone,
}

/// The session, read on its own thread.
pub struct Media {
    events: Option<Receiver<Event>>,
    /// The song as last reported.
    pub song: Option<Song>,
}

impl Media {
    /// Start reading the session (on Windows; elsewhere this reports nothing).
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        let started = spawn(tx);
        Self { events: started.then_some(rx), song: None }
    }

    /// The changes since last asked, oldest first.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(rx) = &self.events else { return out };
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    self.song = match &event {
                        Event::NewSong(s) | Event::Updated(s) => Some(s.clone()),
                        Event::Gone => None,
                    };
                    out.push(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.events = None;
                    break;
                }
            }
        }
        out
    }
}

#[cfg(not(windows))]
fn spawn(_tx: mpsc::Sender<Event>) -> bool {
    false
}

#[cfg(windows)]
fn spawn(tx: mpsc::Sender<Event>) -> bool {
    std::thread::Builder::new().name("media session".into()).spawn(move || windows_session::run(&tx)).is_ok()
}

#[cfg(windows)]
mod windows_session {
    use super::{ART_RETRIES, ART_SIZE, Art, Event, POLL_S, Song};
    use std::sync::Arc;
    use std::sync::mpsc::Sender;
    use std::time::Duration;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform, ColorManagementMode, ExifOrientationMode};
    use windows::Media::Control::{GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

    /// Read the session until the app goes (the channel closes).
    pub fn run(tx: &Sender<Event>) {
        // Safety: initialising WinRT for this thread, which owns no other COM state.
        if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            log::warn!("no media session: {e}");
            return;
        }
        let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => m,
            Err(e) => return log::warn!("no media session: {e}"),
        };
        let mut last: Option<Song> = None;
        // Reads left to look for a new song's art, and the art of the song before.
        let mut retries = 0;
        let mut previous_art: Option<Art> = None;
        loop {
            let now = read(&manager);
            let event = match (&last, now) {
                (None, None) => None,
                (Some(_), None) => Some(Event::Gone),
                (Some(old), Some(mut song)) if song.same(old) => {
                    song.art = old.art.clone();
                    if retries > 0 {
                        retries -= 1;
                        if let Some(art) = read_art(&manager).filter(|a| Some(a) != previous_art.as_ref()) {
                            song.art = Some(art);
                            retries = 0;
                        }
                    }
                    (&song != old).then_some(Event::Updated(song))
                }
                (_, Some(mut song)) => {
                    previous_art = last.as_ref().and_then(|s| s.art.clone());
                    song.art = read_art(&manager).filter(|a| Some(a) != previous_art.as_ref());
                    retries = if song.art.is_none() { ART_RETRIES } else { 0 };
                    Some(Event::NewSong(song))
                }
            };
            if let Some(event) = event {
                last = match &event {
                    Event::NewSong(s) | Event::Updated(s) => Some(s.clone()),
                    Event::Gone => None,
                };
                if tx.send(event).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs_f32(POLL_S));
        }
    }

    /// The current session's song, without art (none if nothing reports one).
    fn read(manager: &Manager) -> Option<Song> {
        let session = manager.GetCurrentSession().ok()?;
        let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let title = props.Title().ok()?.to_string();
        if title.is_empty() {
            return None;
        }
        let playing = session.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).is_ok_and(|s| s == Status::Playing);
        let text = |s: windows::core::Result<windows::core::HSTRING>| s.map(|h| h.to_string()).unwrap_or_default();
        Some(Song { title, artist: text(props.Artist()), album: text(props.AlbumTitle()), playing, art: None })
    }

    /// The current song's cover, decoded by Windows and scaled to fit `ART_SIZE`.
    fn read_art(manager: &Manager) -> Option<Art> {
        let session = manager.GetCurrentSession().ok()?;
        let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
        let decoder = BitmapDecoder::CreateAsync(&stream).ok()?.join().ok()?;
        let (w, h) = (decoder.OrientedPixelWidth().ok()?, decoder.OrientedPixelHeight().ok()?);
        if w == 0 || h == 0 {
            return None;
        }
        let scale = (ART_SIZE as f32 / w.max(h) as f32).min(1.0);
        let size = [((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1)];
        let transform = BitmapTransform::new().ok()?;
        transform.SetScaledWidth(size[0]).ok()?;
        transform.SetScaledHeight(size[1]).ok()?;
        transform.SetInterpolationMode(BitmapInterpolationMode::Fant).ok()?;
        let pixels = decoder
            .GetPixelDataTransformedAsync(BitmapPixelFormat::Rgba8, BitmapAlphaMode::Straight, &transform, ExifOrientationMode::RespectExifOrientation, ColorManagementMode::DoNotColorManage)
            .ok()?
            .join()
            .ok()?
            .DetachPixelData()
            .ok()?;
        (pixels.len() == (size[0] * size[1] * 4) as usize).then(|| Art { size, rgba: Arc::new(pixels.to_vec()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what the session says is playing (run by hand, with something playing).
    #[test]
    #[ignore]
    fn print_the_session() {
        let mut media = Media::start();
        std::thread::sleep(std::time::Duration::from_secs(3));
        for event in media.poll() {
            match event {
                Event::NewSong(s) | Event::Updated(s) => {
                    println!("{} - {} ({}), playing {}, art {:?}", s.artist, s.title, s.album, s.playing, s.art.map(|a| a.size));
                }
                Event::Gone => println!("gone"),
            }
        }
    }
}
