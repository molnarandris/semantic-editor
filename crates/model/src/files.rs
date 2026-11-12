//! The project's files besides the document: pictures, bibliography files,
//! by their path in the project (`images/chain.png`). Cheap to clone (shared
//! until changed); each file's hash is computed once, so compiles can key
//! caches on the files without hashing their bytes again.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// One file's contents and their hash.
#[derive(Debug, PartialEq, Eq)]
struct File {
    bytes: Vec<u8>,
    hash: u64,
}

/// The folder of scratch files (scans of handwritten notes, draft
/// figures): kept in the project, never versioned or exported.
pub const SCRATCH_DIR: &str = "scratch";

/// Whether `path` is a scratch file.
pub fn is_scratch(path: &str) -> bool {
    path.strip_prefix(SCRATCH_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Files(Arc<BTreeMap<String, Arc<File>>>);

impl Files {
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.0.get(path).map(|f| f.bytes.as_slice())
    }

    pub fn insert(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        let file = File {
            hash: h.finish(),
            bytes,
        };
        Arc::make_mut(&mut self.0).insert(path.into(), Arc::new(file));
    }

    pub fn remove(&mut self, path: &str) {
        if self.0.contains_key(path) {
            Arc::make_mut(&mut self.0).remove(path);
        }
    }

    /// Paths and contents, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.0.iter().map(|(p, f)| (p.as_str(), f.bytes.as_slice()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A path in `dir` for a new file called `name` that no file has yet:
    /// `images/chain.png`, else `images/chain-2.png`, …
    pub fn free_path(&self, dir: &str, name: &str) -> String {
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
            _ => (name, String::new()),
        };
        (1..)
            .map(|n| match n {
                1 => format!("{dir}/{stem}{ext}"),
                n => format!("{dir}/{stem}-{n}{ext}"),
            })
            .find(|p| !self.0.contains_key(p))
            .expect("some path is free")
    }
}

impl Hash for Files {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (path, file) in self.0.iter() {
            path.hash(state);
            file.hash.hash(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files() {
        let mut f = Files::default();
        let shared = f.clone();
        f.insert("images/a.png", vec![1, 2]);
        assert!(shared.is_empty(), "clones are not changed");
        assert_eq!(f.get("images/a.png"), Some(&[1u8, 2][..]));
        assert_eq!(f.free_path("images", "a.png"), "images/a-2.png");
        assert_eq!(f.free_path("images", "b.png"), "images/b.png");
        let hash = |f: &Files| {
            let mut h = DefaultHasher::new();
            f.hash(&mut h);
            h.finish()
        };
        let mut g = f.clone();
        g.insert("images/a.png", vec![1, 3]);
        assert_ne!(hash(&f), hash(&g));
        g.remove("images/a.png");
        assert!(g.is_empty());
    }
}
