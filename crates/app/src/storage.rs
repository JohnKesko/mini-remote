//! Files in the app's own folder (on a Mac: ~/Library/Application Support/Mini Remote):
//! saved connections, settings, trusted server identities and screen thumbnails.
//! Passwords never go here; they live in the system password store.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::{Connection, Settings};

const CONNECTIONS_FILE: &str = "connections.json";
const SETTINGS_FILE: &str = "settings.json";
const TRUSTED_FILE: &str = "trusted.json";
const THUMBNAILS_DIR: &str = "thumbnails";
/// Thumbnails are scaled down to at most this width.
pub const THUMBNAIL_WIDTH: u32 = 480;

#[derive(Clone, Debug)]
pub struct Storage {
    dir: Option<PathBuf>,
}

impl Storage {
    /// Open the app folder, creating it if needed. Without a folder nothing is saved.
    pub fn open() -> Self {
        // MINI_REMOTE_DATA_DIR keeps test runs away from the real settings.
        let dir = std::env::var_os("MINI_REMOTE_DATA_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| directories::ProjectDirs::from("", "", "Mini Remote").map(|d| d.config_dir().to_path_buf()));
        if let Some(dir) = &dir
            && let Err(err) = fs::create_dir_all(dir.join(THUMBNAILS_DIR))
        {
            log::warn!("could not create {}: {err}", dir.display());
        }
        if dir.is_none() {
            log::warn!("no folder for saved settings; nothing will be remembered");
        }
        Storage { dir }
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(dir.join(THUMBNAILS_DIR));
        Storage { dir: Some(dir) }
    }

    pub fn load_connections(&self) -> Vec<Connection> {
        let mut list: Vec<Connection> = self.load(CONNECTIONS_FILE).unwrap_or_default();
        // Older or hand-edited files may lack ids.
        for c in &mut list {
            if c.id.is_empty() {
                c.id = crate::model::new_id();
            }
        }
        list
    }

    pub fn save_connections(&self, list: &[Connection]) {
        self.save(CONNECTIONS_FILE, &list);
    }

    pub fn load_settings(&self) -> Settings {
        self.load(SETTINGS_FILE).unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &Settings) {
        self.save(SETTINGS_FILE, settings);
    }

    /// Trusted server identity fingerprints by "host:port".
    pub fn load_trusted(&self) -> BTreeMap<String, String> {
        self.load(TRUSTED_FILE).unwrap_or_default()
    }

    pub fn save_trusted(&self, trusted: &BTreeMap<String, String>) {
        self.save(TRUSTED_FILE, trusted);
    }

    /// Where the thumbnail of a connection's last screen is kept.
    pub fn thumbnail_path(&self, connection_id: &str) -> Option<PathBuf> {
        let safe: String = connection_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        if safe.is_empty() {
            return None;
        }
        Some(self.dir.as_ref()?.join(THUMBNAILS_DIR).join(format!("{safe}.png")))
    }

    pub fn delete_thumbnail(&self, connection_id: &str) {
        if let Some(path) = self.thumbnail_path(connection_id) {
            let _ = fs::remove_file(path);
        }
    }

    fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let path = self.dir.as_ref()?.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                log::warn!("could not read {}: {err}", path.display());
                return None;
            }
        };
        match serde_json::from_str(&text) {
            Ok(value) => Some(value),
            Err(err) => {
                log::warn!("could not understand {}: {err}", path.display());
                // Keep the broken file for the user instead of overwriting it.
                let _ = fs::copy(&path, path.with_extension("json.broken"));
                None
            }
        }
    }

    fn save<T: Serialize + ?Sized>(&self, name: &str, value: &T) {
        let Some(dir) = &self.dir else { return };
        let path = dir.join(name);
        let result = serde_json::to_string_pretty(value)
            .map_err(io::Error::other)
            .and_then(|text| write_atomic(&path, text.as_bytes()));
        if let Err(err) = result {
            log::warn!("could not save {}: {err}", path.display());
        }
    }
}

/// Write to a temporary file and rename it, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// Scale an RGBA picture down to thumbnail size and save it as PNG.
pub fn save_thumbnail(path: &Path, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), String> {
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("picture has the wrong size")?;
    let thumb = if width > THUMBNAIL_WIDTH {
        let h = (u64::from(height) * u64::from(THUMBNAIL_WIDTH) / u64::from(width)).max(1) as u32;
        image::imageops::resize(&img, THUMBNAIL_WIDTH, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut bytes = Vec::new();
    thumb
        .write_to(&mut io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    write_atomic(path, &bytes).map_err(|e| e.to_string())
}

/// Load a thumbnail for showing on the home screen.
pub fn load_thumbnail(path: &Path) -> Option<egui::ColorImage> {
    let bytes = fs::read(path).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// Save a full-size screenshot as PNG.
pub fn save_png(path: &Path, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), String> {
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("picture has the wrong size")?;
    img.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mini-remote-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("storage");
        let storage = Storage::at(dir.clone());
        let c = Connection { id: "abc".into(), host: "pi".into(), ..Connection::default() };
        storage.save_connections(std::slice::from_ref(&c));
        assert_eq!(storage.load_connections(), vec![c]);

        let mut trusted = BTreeMap::new();
        trusted.insert("pi:5900".to_string(), "aa:bb".to_string());
        storage.save_trusted(&trusted);
        assert_eq!(storage.load_trusted(), trusted);

        let path = storage.thumbnail_path("abc").unwrap();
        save_thumbnail(&path, 2, 1, vec![255; 8]).unwrap();
        assert_eq!(load_thumbnail(&path).unwrap().size, [2, 1]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let c: Connection = serde_json::from_str(r#"{"host":"pi"}"#).unwrap();
        assert_eq!(c.port, 5900);
        assert!(c.share_clipboard);
    }
}
