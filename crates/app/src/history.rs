//! A document's version history: a git repository stored in the `.sem`
//! zip under `.git/` (one pack, packed refs, a non-bare config, so an
//! unzipped project is an ordinary git checkout). While a document is open
//! the repository lives in a private temporary folder. Every save that
//! changes something is a commit on `main`; tags name versions ("arXiv
//! submission"). No GTK here.

use std::collections::HashMap;
use std::path::PathBuf;

use git2::{ObjectType, Oid, Repository, Signature, Time};

/// The folder in the zip holding the repository.
pub const GIT_DIR: &str = ".git/";
const BRANCH: &str = "refs/heads/main";

pub struct History {
    repo: Repository,
    // Deleted (with the repository) when the history is dropped.
    _dir: tempfile::TempDir,
}

/// One saved version: a commit.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub id: Oid,
    /// Seconds since the epoch, and the offset of its time zone in minutes.
    pub time: i64,
    pub offset: i32,
    pub message: String,
    /// Its tags' names, as typed.
    pub tags: Vec<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("version history: {e}")
}

impl History {
    /// An empty history.
    pub fn new() -> Result<Self, String> {
        let dir = tempfile::TempDir::new().map_err(err)?;
        let git = dir.path().join("git");
        let mut opts = git2::RepositoryInitOptions::new();
        opts.bare(true).initial_head("main");
        let repo = Repository::init_opts(&git, &opts).map_err(err)?;
        Ok(Self { repo, _dir: dir })
    }

    /// The history stored in a zip: its `.git/` entries (paths without the
    /// prefix).
    pub fn from_entries(entries: &[(String, Vec<u8>)]) -> Result<Self, String> {
        let dir = tempfile::TempDir::new().map_err(err)?;
        let git = dir.path().join("git");
        for sub in ["objects/pack", "refs/heads", "refs/tags"] {
            std::fs::create_dir_all(git.join(sub)).map_err(err)?;
        }
        for (path, bytes) in entries {
            let target = git.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
            std::fs::write(target, bytes).map_err(err)?;
        }
        let repo = Repository::open_bare(&git).map_err(err)?;
        Ok(Self { repo, _dir: dir })
    }

    /// Commit `files` (path, contents) as the new version, unless they are
    /// the same as the last one. Returns the new version's id.
    pub fn commit(
        &self,
        files: &[(String, Vec<u8>)],
        author: &str,
        message: &str,
    ) -> Result<Option<Oid>, String> {
        let tree = self.tree(files)?;
        let parent = self.head();
        if parent.as_ref().map(|c| c.tree_id()) == Some(tree) {
            return Ok(None);
        }
        let tree = self.repo.find_tree(tree).map_err(err)?;
        let sig = signature(author)?;
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let id = self
            .repo
            .commit(Some(BRANCH), &sig, &sig, message, &tree, &parents)
            .map_err(err)?;
        Ok(Some(id))
    }

    fn head(&self) -> Option<git2::Commit<'_>> {
        self.repo.find_reference(BRANCH).ok()?.peel_to_commit().ok()
    }

    /// The tree of `files`, folders made from their paths.
    fn tree(&self, files: &[(String, Vec<u8>)]) -> Result<Oid, String> {
        #[derive(Default)]
        struct Dir {
            files: Vec<(String, Oid)>,
            dirs: HashMap<String, Dir>,
        }
        fn write(repo: &Repository, dir: &Dir) -> Result<Oid, git2::Error> {
            let mut builder = repo.treebuilder(None)?;
            for (name, blob) in &dir.files {
                builder.insert(name, *blob, 0o100644)?;
            }
            for (name, sub) in &dir.dirs {
                builder.insert(name, write(repo, sub)?, 0o040000)?;
            }
            builder.write()
        }
        let mut root = Dir::default();
        for (path, bytes) in files {
            let blob = self.repo.blob(bytes).map_err(err)?;
            let mut parts: Vec<&str> = path.split('/').collect();
            let name = parts.pop().unwrap_or_default();
            let mut dir = &mut root;
            for part in parts {
                dir = dir.dirs.entry(part.to_string()).or_default();
            }
            dir.files.push((name.to_string(), blob));
        }
        write(&self.repo, &root).map_err(err)
    }

    /// The repository as zip entries (paths under `.git/`, without the
    /// prefix): one pack of every version and tag, packed refs, `HEAD` and
    /// a config for a checkout (the unzipped folder is its work tree).
    pub fn entries(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let mut out = vec![
            ("HEAD".to_string(), b"ref: refs/heads/main\n".to_vec()),
            (
                "config".to_string(),
                b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n".to_vec(),
            ),
        ];
        let Some(head) = self.head() else {
            return Ok(out);
        };
        let mut refs = vec![(BRANCH.to_string(), head.id())];
        let mut pack = self.repo.packbuilder().map_err(err)?;
        let mut walk = self.repo.revwalk().map_err(err)?;
        walk.push(head.id()).map_err(err)?;
        for (name, tag) in self.tag_refs()? {
            walk.push(self.target_of(tag)?).map_err(err)?;
            pack.insert_object(tag, None).map_err(err)?;
            refs.push((name, tag));
        }
        pack.insert_walk(&mut walk).map_err(err)?;
        let dir = tempfile::TempDir::new().map_err(err)?;
        pack.write(dir.path(), 0o644).map_err(err)?;
        for entry in std::fs::read_dir(dir.path()).map_err(err)? {
            let path: PathBuf = entry.map_err(err)?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            out.push((
                format!("objects/pack/{name}"),
                std::fs::read(&path).map_err(err)?,
            ));
        }
        refs.sort();
        let packed: String = refs
            .iter()
            .map(|(name, id)| format!("{id} {name}\n"))
            .collect();
        out.push(("packed-refs".to_string(), packed.into_bytes()));
        Ok(out)
    }

    /// The tags' refs and tag objects.
    fn tag_refs(&self) -> Result<Vec<(String, Oid)>, String> {
        let mut out = vec![];
        for r in self.repo.references_glob("refs/tags/*").map_err(err)? {
            let r = r.map_err(err)?;
            if let (Ok(name), Some(id)) = (r.name(), r.target()) {
                out.push((name.to_string(), id));
            }
        }
        Ok(out)
    }

    /// The commit a tag (or commit) id points to.
    fn target_of(&self, id: Oid) -> Result<Oid, String> {
        let object = self.repo.find_object(id, None).map_err(err)?;
        Ok(object.peel(ObjectType::Commit).map_err(err)?.id())
    }

    /// The saved versions, newest first, with their tags.
    pub fn versions(&self) -> Result<Vec<Version>, String> {
        let Some(head) = self.head() else {
            return Ok(vec![]);
        };
        let mut tags: HashMap<Oid, Vec<String>> = HashMap::new();
        for (name, id) in self.tag_refs()? {
            let shown = match self.repo.find_tag(id) {
                Ok(tag) => tag
                    .message()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                Err(_) => name.trim_start_matches("refs/tags/").to_string(),
            };
            tags.entry(self.target_of(id)?).or_default().push(shown);
        }
        let mut walk = self.repo.revwalk().map_err(err)?;
        walk.push(head.id()).map_err(err)?;
        let mut out = vec![];
        for id in walk {
            let commit = self.repo.find_commit(id.map_err(err)?).map_err(err)?;
            let time: Time = commit.time();
            out.push(Version {
                id: commit.id(),
                time: time.seconds(),
                offset: time.offset_minutes(),
                message: commit.message().unwrap_or_default().trim().to_string(),
                tags: tags.remove(&commit.id()).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Tag version `id` with `name` (as typed; the ref gets a safe form of
    /// it). An existing tag of the same name moves.
    pub fn tag(&self, id: Oid, name: &str, author: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(err("a tag needs a name"));
        }
        let object = self.repo.find_object(id, None).map_err(err)?;
        let sig = signature(author)?;
        self.repo
            .tag(&ref_name(name), &object, &sig, name, true)
            .map_err(err)?;
        Ok(())
    }

    /// The files of version `id` (path, contents).
    pub fn files_at(&self, id: Oid) -> Result<Vec<(String, Vec<u8>)>, String> {
        let tree = self
            .repo
            .find_commit(id)
            .map_err(err)?
            .tree()
            .map_err(err)?;
        let mut out = vec![];
        let mut failed = None;
        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            if entry.kind() == Some(ObjectType::Blob) {
                let path = format!("{dir}{}", entry.name().unwrap_or_default());
                match self.repo.find_blob(entry.id()) {
                    Ok(blob) => out.push((path, blob.content().to_vec())),
                    Err(e) => failed = Some(e),
                }
            }
            git2::TreeWalkResult::Ok
        })
        .map_err(err)?;
        match failed {
            Some(e) => Err(err(e)),
            None => Ok(out),
        }
    }

    /// Where the repository is (tests).
    #[cfg(test)]
    fn path(&self) -> &std::path::Path {
        self.repo.path()
    }
}

/// An author with `name` and no email address (user decision; libgit2
/// needs one, so the placeholder `none`).
fn signature(name: &str) -> Result<Signature<'static>, String> {
    let name = if name.trim().is_empty() {
        "Author"
    } else {
        name.trim()
    };
    Signature::now(name, "none").map_err(err)
}

/// A git ref name for a tag called `name`: letters, digits, `.`, `_`, `-`;
/// other chars become `-`.
fn ref_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| match c {
            c if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') => c,
            _ => '-',
        })
        .collect();
    let safe = safe.trim_matches(|c| c == '-' || c == '.').to_string();
    if safe.is_empty() { "tag".into() } else { safe }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(text: &str) -> Vec<(String, Vec<u8>)> {
        vec![
            ("document.json".into(), text.as_bytes().to_vec()),
            ("images/a.png".into(), vec![1, 2, 3]),
        ]
    }

    /// Commits on change only; tags with their typed names; stored as zip
    /// entries and read back with every version and tag.
    #[test]
    fn versions_and_tags() {
        let h = History::new().unwrap();
        assert!(h.versions().unwrap().is_empty());
        let first = h.commit(&files("one"), "Ada", "Saved").unwrap().unwrap();
        assert_eq!(
            h.commit(&files("one"), "Ada", "Saved").unwrap(),
            None,
            "unchanged"
        );
        let second = h.commit(&files("two"), "Ada", "Saved").unwrap().unwrap();
        h.tag(first, "arXiv submission", "Ada").unwrap();
        let versions = h.versions().unwrap();
        let ids: Vec<Oid> = versions.iter().map(|v| v.id).collect();
        assert_eq!(ids, [second, first]);
        assert_eq!(versions[1].tags, ["arXiv submission"]);
        assert_eq!(h.files_at(first).unwrap(), files("one"));

        let entries = h.entries().unwrap();
        let names: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
        assert!(names.contains(&"packed-refs") && names.contains(&"HEAD"));
        assert_eq!(names.iter().filter(|n| n.ends_with(".pack")).count(), 1);
        let back = History::from_entries(&entries).unwrap();
        assert_eq!(back.versions().unwrap(), versions);
        assert_eq!(back.files_at(second).unwrap(), files("two"));
        // And it goes on from there.
        let third = back
            .commit(&files("three"), "Ada", "Saved")
            .unwrap()
            .unwrap();
        assert_eq!(back.versions().unwrap()[0].id, third);
        assert!(back.path().exists());
    }

    #[test]
    fn tag_ref_names() {
        assert_eq!(ref_name("arXiv submission"), "arXiv-submission");
        assert_eq!(ref_name("v1.0"), "v1.0");
        assert_eq!(ref_name("  "), "tag");
        assert_eq!(ref_name("Journal: PRL / 2nd"), "Journal--PRL---2nd");
    }
}
