//! A Typst [`World`]: one in-memory main source, the project's files
//! (pictures, bibliography files; see [`model::Files`]), embedded fonts, and
//! packages (`@preview/cetz`, …) served like the Typst CLI does: from the
//! local package data directory, the cache, or downloaded from Typst
//! Universe into the cache on first use.

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::downloader::SystemDownloader;
use typst_kit::files::{FileLoader, FileStore};
use typst_kit::packages::SystemPackages;

/// Loads files of packages (the project's own come from `BridgeWorld::files`).
struct PackageFiles {
    packages: SystemPackages,
}

impl FileLoader for PackageFiles {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        match id.root() {
            VirtualRoot::Package(spec) => self.packages.obtain(spec)?.load(id.vpath()),
            VirtualRoot::Project => Err(FileError::NotFound(id.vpath().get_without_slash().into())),
        }
    }
}

/// Long-lived compilation environment. Fonts and the standard library are
/// loaded once; the main source is swapped for every compile.
pub struct BridgeWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
    /// The project's files besides the main source.
    project: model::Files,
    files: FileStore<PackageFiles>,
}

impl BridgeWorld {
    pub fn new() -> Self {
        let fonts: Vec<Font> = typst_kit::fonts::embedded().map(|(font, _)| font).collect();
        let book = FontBook::from_fonts(&fonts);
        let main_id =
            RootedPath::new(VirtualRoot::Project, VirtualPath::new("/main.typ").unwrap()).intern();
        Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(book),
            fonts,
            main: Source::new(main_id, String::new()),
            project: model::Files::default(),
            files: FileStore::new(PackageFiles {
                packages: SystemPackages::new(SystemDownloader::new(concat!(
                    "semantic-editor/",
                    env!("CARGO_PKG_VERSION")
                ))),
            }),
        }
    }

    /// Replace the main source text. Reusing the same `Source` lets Typst
    /// reparse incrementally and keeps memoized results warm.
    pub fn set_main_text(&mut self, text: &str) {
        self.main.replace(text);
    }

    /// Replace the project's files.
    pub fn set_files(&mut self, files: model::Files) {
        self.project = files;
    }

    /// A project file by its id, if the project has it.
    fn project_file(&self, id: FileId) -> Option<&[u8]> {
        match id.root() {
            VirtualRoot::Project => self.project.get(id.vpath().get_without_slash()),
            VirtualRoot::Package(_) => None,
        }
    }

    pub fn main_source(&self) -> &Source {
        &self.main
    }
}

impl Default for BridgeWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl World for BridgeWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else if let Some(bytes) = self.project_file(id) {
            let text = std::str::from_utf8(bytes).map_err(|_| FileError::InvalidUtf8)?;
            Ok(Source::new(id, text.to_owned()))
        } else {
            self.files.source(id)
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main.id() {
            Ok(Bytes::from_string(self.main.text().to_owned()))
        } else if let Some(bytes) = self.project_file(id) {
            Ok(Bytes::new(bytes.to_vec()))
        } else {
            self.files.file(id)
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}
