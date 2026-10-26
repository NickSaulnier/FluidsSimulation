//! The looks' thumbnails (see `render::thumbnail`), cached as PNG files in `thumbnails`
//! beside the settings and loaded into textures as the browser shows them. A file is
//! named after the look and its fingerprint (`looks::fingerprint`), so editing a look
//! makes it a new thumbnail; the old one is removed.

use crate::looks::{self, Look};
use crate::render::thumbnail::SIZE;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Default)]
pub struct Thumbnails {
    folder: Option<PathBuf>,
    /// By key: loaded, or known not to be on disk.
    textures: HashMap<String, Option<egui::TextureHandle>>,
    /// Keys asked for this run (a capture that failed isn't tried again).
    asked: HashSet<String>,
}

impl Thumbnails {
    pub fn new(folder: Option<PathBuf>) -> Self {
        Self { folder, ..Self::default() }
    }

    /// Where the thumbnails live: `thumbnails` beside the settings file.
    pub fn user_folder() -> Option<PathBuf> {
        Some(crate::ui::settings::path()?.parent()?.join("thumbnails"))
    }

    /// The thumbnail's key for `look` (its file's stem).
    pub fn key(look: &Look) -> String {
        format!("{}-{:016x}", looks::file_stem(&look.name), looks::fingerprint(look))
    }

    /// Whether `look` still needs a thumbnail (none cached, none asked for yet this run).
    /// Marks it asked.
    pub fn wants(&mut self, look: &Look) -> Option<String> {
        self.folder.as_ref()?;
        let key = Self::key(look);
        if self.asked.contains(&key) || self.path(&key).is_some_and(|p| p.exists()) {
            return None;
        }
        self.asked.insert(key.clone());
        Some(key)
    }

    /// `look`'s thumbnail, if there is one (loaded from disk the first time).
    pub fn get(&mut self, ctx: &egui::Context, look: &Look) -> Option<egui::TextureHandle> {
        let key = Self::key(look);
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let texture = self.path(&key).and_then(|p| read_png(&p)).map(|image| ctx.load_texture(&key, image, egui::TextureOptions::LINEAR));
        self.textures.insert(key, texture.clone());
        texture
    }

    /// Keep `pixels` (`SIZE`, RGBA) as the thumbnail for `key`, on disk and loaded,
    /// in place of any older one of the same look.
    pub fn store(&mut self, ctx: &egui::Context, key: &str, pixels: &[u8]) {
        let image = egui::ColorImage::from_rgba_unmultiplied(SIZE, pixels);
        self.textures.insert(key.to_string(), Some(ctx.load_texture(key, image, egui::TextureOptions::LINEAR)));
        let (Some(folder), Some(path)) = (self.folder.clone(), self.path(key)) else { return };
        let stem = key.rsplit_once('-').map_or(key, |(stem, _)| stem);
        if let Ok(entries) = std::fs::read_dir(&folder) {
            for e in entries.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.rsplit_once('-').is_some_and(|(s, _)| s == stem) && name != format!("{key}.png") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        if let Err(e) = std::fs::create_dir_all(&folder).map_err(|e| e.to_string()).and_then(|()| write_png(&path, pixels)) {
            log::warn!("couldn't save the thumbnail {}: {e}", path.display());
        }
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        Some(self.folder.as_ref()?.join(format!("{key}.png")))
    }
}

fn write_png(path: &std::path::Path, pixels: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIZE[0] as u32, SIZE[1] as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())
}

/// A thumbnail file, if it reads as one (RGBA, 8 bits, `SIZE`).
fn read_png(path: &std::path::Path) -> Option<egui::ColorImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    let size = [info.width as usize, info.height as usize];
    (info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight && size == SIZE)
        .then(|| egui::ColorImage::from_rgba_unmultiplied(size, &pixels[..info.buffer_size()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored thumbnail comes back from disk, replaces the look's older one, and a look
    /// with one isn't asked for again.
    #[test]
    fn thumbnails_are_stored_and_found() {
        let dir = std::env::temp_dir().join(format!("fluid-toy-thumbs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut t = Thumbnails::new(Some(dir.clone()));
        let library = looks::Library::builtin();
        let look = library.looks[0].clone();
        let key = t.wants(&look).expect("none yet");
        assert!(t.wants(&look).is_none(), "asked once per run");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}-0000000000000000.png", looks::file_stem(&look.name))), b"old").unwrap();
        let pixels: Vec<u8> = (0..SIZE[0] * SIZE[1] * 4).map(|i| (i % 251) as u8).collect();
        t.store(&ctx, &key, &pixels);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).map(|e| e.file_name()).collect();
        assert_eq!(files.len(), 1, "the old thumbnail was removed: {files:?}");
        let image = read_png(&dir.join(format!("{key}.png"))).expect("reads back");
        assert_eq!(image.size, SIZE);
        let mut fresh = Thumbnails::new(Some(dir.clone()));
        assert!(fresh.wants(&look).is_none(), "on disk already");
        assert!(fresh.get(&ctx, &look).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
