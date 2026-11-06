//! A minimal in-memory Typst [`World`]: one main source, embedded fonts.

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

/// Long-lived compilation environment. Fonts and the standard library are
/// loaded once; the main source is swapped for every compile.
pub struct BridgeWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
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
        }
    }

    /// Replace the main source text. Reusing the same `Source` lets Typst
    /// reparse incrementally and keeps memoized results warm.
    pub fn set_main_text(&mut self, text: &str) {
        self.main.replace(text);
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
        } else {
            Err(FileError::NotFound(id.vpath().get_without_slash().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.source(id)
            .map(|s| Bytes::from_string(s.text().to_owned()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}
