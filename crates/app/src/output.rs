//! Screenshots and recordings of the picture (the composite, without the interface).
//!
//! - **Screenshots** (F12): the frame at the window's size, as a PNG in
//!   `Pictures\fluid-toy`, named after the look.
//! - **Recordings** (F10): frames at up to `RECORD_HEIGHT`, `RECORD_FPS` a second, piped
//!   raw to `ffmpeg` (which must be on the PATH) and encoded to MP4 in
//!   `Videos\fluid-toy`. The app's frames come when they come, so the `Pacer` repeats or
//!   skips them to keep the video in time. A track playing from a file goes in as the
//!   sound, from where it was when the recording started; live audio isn't recorded.

use crate::render::grab::Image;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Instant;

pub const RECORD_FPS: u32 = 30;
/// Recordings are at most this tall (the window's shape, even sizes).
pub const RECORD_HEIGHT: u32 = 1080;
/// Frames queued for the encoder before more are dropped.
const QUEUE: usize = 8;

/// `<home>\<under>\fluid-toy` (Pictures or Videos), or beside the settings without a
/// home folder.
pub fn folder(under: &str) -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(PathBuf::from);
    match home {
        Some(home) => Some(home.join(under).join("fluid-toy")),
        None => Some(crate::ui::settings::path()?.parent()?.join(under.to_lowercase())),
    }
}

/// The first `<stem> <n>.<ext>` in `folder` that doesn't exist yet (from 1), the stem
/// made safe for a file name.
pub fn free_name(folder: &Path, stem: &str, ext: &str) -> PathBuf {
    let safe: String = stem.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let safe = safe.trim();
    let safe = if safe.is_empty() { "fluid-toy" } else { safe };
    (1..).map(|n| folder.join(format!("{safe} {n}.{ext}"))).find(|p| !p.exists()).expect("a free name")
}

/// Save `image` as a PNG named after `stem`, on its own thread; `done` gets where it
/// went, or what went wrong.
pub fn save_screenshot(image: Image, stem: &str, done: std::sync::mpsc::Sender<Result<PathBuf, String>>) {
    let Some(folder) = folder("Pictures") else {
        let _ = done.send(Err("no folder for pictures".into()));
        return;
    };
    let stem = stem.to_string();
    std::thread::spawn(move || {
        let result = std::fs::create_dir_all(&folder).map_err(|e| e.to_string()).and_then(|()| {
            let path = free_name(&folder, &stem, "png");
            write_png(&path, &image).map(|()| path)
        });
        let _ = done.send(result);
    });
}

pub fn write_png(path: &Path, image: &Image) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.size[0], image.size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&image.rgba).map_err(|e| e.to_string())
}

/// The size to record a `window`-sized picture at: its shape, at most `RECORD_HEIGHT`
/// tall, both sides even (as the encoder needs).
pub fn record_size(window: [u32; 2]) -> [u32; 2] {
    let height = window[1].clamp(2, RECORD_HEIGHT);
    let width = (window[0] as f32 * height as f32 / window[1].max(1) as f32).round() as u32;
    [width.max(2) & !1, height & !1]
}

/// Keeps a video's frames in time with the app's: how many video frames each grabbed
/// frame stands for (0 if it came too soon, more than 1 if the app fell behind).
pub struct Pacer {
    fps: f64,
    /// The next video frame's time, seconds from the start.
    next: f64,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        Self { fps: f64::from(fps), next: 0.0 }
    }

    /// Whether a frame grabbed at `t` seconds would be used.
    pub fn due(&self, t: f64) -> bool {
        t >= self.next
    }

    /// How many video frames a frame grabbed at `t` seconds fills.
    pub fn frames(&mut self, t: f64) -> usize {
        let mut n = 0;
        while self.next <= t {
            n += 1;
            self.next += 1.0 / self.fps;
        }
        n
    }
}

/// The arguments for ffmpeg: raw `size` frames at `fps` on its input (blue first if
/// `bgra`), plus a track's sound from `audio` (its file, and where in it to start), to
/// `out`.
pub fn ffmpeg_args(size: [u32; 2], fps: u32, audio: Option<(&Path, f32)>, out: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba"].map(String::from).to_vec();
    args.extend(["-s".into(), format!("{}x{}", size[0], size[1]), "-framerate".into(), fps.to_string(), "-i".into(), "-".into()]);
    if let Some((track, start)) = audio {
        args.extend(["-ss".into(), format!("{start:.3}"), "-i".into(), track.display().to_string()]);
        args.extend(["-map", "0:v:0", "-map", "1:a:0", "-c:a", "aac", "-b:a", "192k", "-shortest"].map(String::from));
    }
    args.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p"].map(String::from));
    args.push(out.display().to_string());
    args
}

/// A recording under way.
pub struct Recorder {
    pub path: PathBuf,
    pub size: [u32; 2],
    started: Instant,
    pacer: Pacer,
    frames: SyncSender<Arc<Vec<u8>>>,
    writer: Option<std::thread::JoinHandle<()>>,
    child: Child,
    /// Video frames dropped because the encoder fell behind.
    pub dropped: u32,
}

impl Recorder {
    /// Start recording a `window`-sized picture, named after `stem`, with `audio` (see
    /// `ffmpeg_args`).
    pub fn start(window: [u32; 2], stem: &str, audio: Option<(&Path, f32)>) -> Result<Self, String> {
        let folder = folder("Videos").ok_or("no folder for videos")?;
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let path = free_name(&folder, stem, "mp4");
        let size = record_size(window);
        let mut child = Command::new("ffmpeg")
            .args(ffmpeg_args(size, RECORD_FPS, audio, &path))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "recording needs ffmpeg (ffmpeg.org) on the PATH".to_string() } else { format!("can't start ffmpeg: {e}") })?;
        let stdin = child.stdin.take().ok_or("no pipe to ffmpeg")?;
        let (frames, rx) = sync_channel(QUEUE);
        let writer = std::thread::Builder::new().name("recording".into()).spawn(move || feed(stdin, &rx)).map_err(|e| e.to_string())?;
        Ok(Self { path, size, started: Instant::now(), pacer: Pacer::new(RECORD_FPS), frames, writer: Some(writer), child, dropped: 0 })
    }

    /// Seconds since the recording started.
    pub fn time(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Whether to grab this frame (the next video frame is due).
    pub fn wants_frame(&self) -> bool {
        self.pacer.due(self.time())
    }

    /// A frame grabbed at `t` seconds: queue it for as many video frames as it stands
    /// for (a frame's worth is dropped if the encoder can't keep up).
    pub fn push(&mut self, t: f64, image: Image) {
        if image.size != self.size {
            return;
        }
        let frame = Arc::new(image.rgba);
        for _ in 0..self.pacer.frames(t) {
            match self.frames.try_send(frame.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => self.dropped += 1,
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    /// Finish: close the pipe and let ffmpeg write the file (on a thread, so the app
    /// doesn't wait). `done` gets the file, or ffmpeg's complaint.
    pub fn stop(mut self, done: std::sync::mpsc::Sender<Result<PathBuf, String>>) {
        let path = self.path.clone();
        let writer = self.writer.take();
        drop(self.frames);
        std::thread::spawn(move || {
            if let Some(w) = writer {
                let _ = w.join();
            }
            let result = match self.child.wait_with_output() {
                Ok(out) if out.status.success() => Ok(path),
                Ok(out) => Err(format!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim())),
                Err(e) => Err(e.to_string()),
            };
            let _ = done.send(result);
        });
    }
}

/// Write queued frames into ffmpeg until the queue closes.
fn feed(mut stdin: ChildStdin, rx: &Receiver<Arc<Vec<u8>>>) {
    while let Ok(frame) = rx.recv() {
        if stdin.write_all(&frame).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A video frame for every tick: none for a frame that comes early, several for one
    /// that comes late.
    #[test]
    fn the_pacer_keeps_time() {
        let mut p = Pacer::new(30);
        assert_eq!(p.frames(0.0), 1);
        assert!(!p.due(0.01));
        assert_eq!(p.frames(0.01), 0, "too soon");
        assert_eq!(p.frames(0.034), 1);
        assert_eq!(p.frames(0.2), 5, "the app stalled: the frame stands in for those it missed");
        let total: usize = (1..=600).map(|k| p.frames(0.2 + k as f64 / 144.0)).sum();
        assert!((total as f64 - 30.0 * 600.0 / 144.0).abs() <= 1.0, "{total} frames for {} s", 600.0 / 144.0);
    }

    #[test]
    fn recordings_keep_the_shape_with_even_sides() {
        assert_eq!(record_size([3840, 2160]), [1920, 1080]);
        assert_eq!(record_size([1441, 761]), [1440, 760]);
        assert_eq!(record_size([800, 601]), [800, 600]);
    }

    #[test]
    fn names_are_numbered_from_one() {
        let dir = std::env::temp_dir().join(format!("fluid-toy-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_name(&dir, "Lava lamp", "png"), dir.join("Lava lamp 1.png"));
        std::fs::write(dir.join("Lava lamp 1.png"), b"").unwrap();
        assert_eq!(free_name(&dir, "Lava lamp", "png"), dir.join("Lava lamp 2.png"));
        assert_eq!(free_name(&dir, "a/b:c?", "mp4"), dir.join("a_b_c_ 1.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ffmpeg_gets_raw_frames_and_the_track() {
        let args = ffmpeg_args([1920, 1080], 30, Some((Path::new("song.mp3"), 12.5)), Path::new("out.mp4"));
        let joined = args.join(" ");
        assert!(joined.contains("-f rawvideo -pix_fmt rgba -s 1920x1080 -framerate 30 -i -"), "{joined}");
        assert!(joined.contains("-ss 12.500 -i song.mp3") && joined.contains("-shortest"), "{joined}");
        assert!(joined.ends_with("-pix_fmt yuv420p out.mp4"), "{joined}");
        assert!(!ffmpeg_args([2, 2], 30, None, Path::new("o.mp4")).join(" ").contains("-map"));
    }
}
