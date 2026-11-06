//! Renders of slow blocks (diagrams, raw Typst) kept on disk, so reopening a
//! document does not compile them again. One PNG (the raster) and one JSON
//! file (the id-free layout) per render, named by the render's cache key.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::compile::Raster;
use crate::layout::RawLayout;

/// Files kept before the oldest renders are removed (two per render).
const MAX_FILES: usize = 1000;

pub struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    /// Use (and create) `dir`; trims old entries.
    pub fn open(dir: impl Into<PathBuf>) -> Option<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir).ok()?;
        let cache = Self { dir };
        cache.prune();
        Some(cache)
    }

    fn paths(&self, key: u64) -> (PathBuf, PathBuf) {
        let base = self.dir.join(format!("{key:016x}"));
        (base.with_extension("png"), base.with_extension("json"))
    }

    pub fn load(&self, key: u64) -> Option<(Raster, RawLayout)> {
        let (png, json) = self.paths(key);
        let layout: RawLayout = serde_json::from_slice(&fs::read(json).ok()?).ok()?;
        let pixmap = tiny_skia::Pixmap::decode_png(&fs::read(png).ok()?).ok()?;
        let raster = Raster {
            width: pixmap.width(),
            height: pixmap.height(),
            data: pixmap.take(),
        };
        Some((raster, layout))
    }

    /// Store a render. Failures (full disk, …) are ignored: it is a cache.
    pub fn store(&self, key: u64, raster: &Raster, layout: &RawLayout) {
        let Some(size) = tiny_skia::IntSize::from_wh(raster.width, raster.height) else {
            return;
        };
        let Some(pixmap) = tiny_skia::Pixmap::from_vec(raster.data.clone(), size) else {
            return;
        };
        let (Ok(png), Ok(json)) = (pixmap.encode_png(), serde_json::to_vec(layout)) else {
            return;
        };
        let (png_path, json_path) = self.paths(key);
        // The layout is written last and read first, so a half-written
        // entry is never used.
        let _ = write_atomically(&png_path, &png).and_then(|_| write_atomically(&json_path, &json));
    }

    /// Remove the oldest files beyond [`MAX_FILES`].
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        if files.len() <= MAX_FILES {
            return;
        }
        files.sort();
        for (_, path) in &files[..files.len() - MAX_FILES] {
            let _ = fs::remove_file(path);
        }
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}
