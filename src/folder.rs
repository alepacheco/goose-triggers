//! Folder sources: a new file whose name matches the rule's pattern is an event.
//!
//! The folder is read every second. A file counts once its size and modification time have
//! not changed between two reads, so a file still being written is not taken half-done.
//! The files a folder holds when a rule first watches it are not new. Files are tracked by
//! name in the store, so files that arrive while goose-triggers is stopped are picked up when
//! it starts, and a name that is deleted and comes again counts again.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};

use crate::store::Store;

pub struct FolderWatch {
    pub rule: String,
    pub folder: PathBuf,
    key: String,
    pattern: glob::Pattern,
    /// Files seen in the last read and not yet taken: name -> (size, mtime in ns).
    pending: HashMap<String, (u64, i128)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFile {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i128,
}

impl NewFile {
    /// The event's fields, passed to the recipe as parameters of the same names.
    pub fn fields(&self, folder: &Path) -> BTreeMap<String, String> {
        let mut f = BTreeMap::new();
        f.insert("file_path".into(), self.path.to_string_lossy().into_owned());
        f.insert("file_name".into(), self.name.clone());
        f.insert("folder".into(), folder.to_string_lossy().into_owned());
        f
    }

    pub fn dedupe_key(&self, rule: &str) -> String {
        format!(
            "folder:{rule}:{}:{}:{}",
            self.path.display(),
            self.size,
            self.mtime
        )
    }
}

impl FolderWatch {
    pub fn new(rule: &str, folder: &Path, pattern: &str) -> Result<FolderWatch> {
        let folder = std::fs::canonicalize(folder)
            .with_context(|| format!("rule {rule}: folder {}", folder.display()))?;
        Ok(FolderWatch {
            rule: rule.to_string(),
            // Keyed by folder and pattern: files there when a pattern is first watched are
            // not new, whatever the rule watched before.
            key: format!("{}\n{pattern}", folder.display()),
            folder,
            pattern: glob::Pattern::new(pattern)?,
            pending: HashMap::new(),
        })
    }

    fn list(&self) -> Result<HashMap<String, (u64, i128)>> {
        let mut out = HashMap::new();
        let entries = std::fs::read_dir(&self.folder)
            .with_context(|| format!("reading {}", self.folder.display()))?;
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') || !self.pattern.matches(&name) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i128)
                .unwrap_or(0);
            out.insert(name, (meta.len(), mtime));
        }
        Ok(out)
    }

    /// Reads the folder once; returns the files that are new and settled. The caller queues
    /// an event for each and then calls `mark_seen`.
    pub fn poll(&mut self, store: &Store) -> Result<Vec<NewFile>> {
        let current = self.list()?;
        if !store.baseline_done(&self.rule, &self.key)? {
            let names: Vec<String> = current.keys().cloned().collect();
            store.set_baseline(&self.rule, &self.key, &names)?;
            return Ok(Vec::new());
        }
        let seen = store.seen_names(&self.rule, &self.key)?;
        for gone in seen.iter().filter(|n| !current.contains_key(*n)) {
            store.remove_seen(&self.rule, &self.key, gone)?;
        }
        let mut new = Vec::new();
        for (name, meta) in &current {
            if seen.contains(name) {
                continue;
            }
            if self.pending.get(name) == Some(meta) {
                new.push(NewFile {
                    name: name.clone(),
                    path: self.folder.join(name),
                    size: meta.0,
                    mtime: meta.1,
                });
            }
        }
        self.pending = current
            .into_iter()
            .filter(|(n, _)| !seen.contains(n))
            .collect();
        for f in &new {
            self.pending.remove(&f.name);
        }
        new.sort_by(|a, b| a.mtime.cmp(&b.mtime).then(a.name.cmp(&b.name)));
        Ok(new)
    }

    pub fn mark_seen(&self, store: &Store, name: &str) -> Result<()> {
        store.add_seen(&self.rule, &self.key, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::temp_store;

    #[test]
    fn new_settled_files_only_once() {
        let (s, path) = temp_store("folder");
        let dir = path.parent().unwrap().join("inbox");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.pdf"), "x").unwrap();
        let mut w = FolderWatch::new("r", &dir, "*.pdf").unwrap();
        assert!(
            w.poll(&s).unwrap().is_empty(),
            "files there at the start are not new"
        );

        std::fs::write(dir.join("a.pdf"), "1").unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        std::fs::write(dir.join(".b.pdf"), "1").unwrap();
        assert!(
            w.poll(&s).unwrap().is_empty(),
            "first sight: not settled yet"
        );
        let new = w.poll(&s).unwrap();
        assert_eq!(
            new.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["a.pdf"]
        );
        let fields = new[0].fields(&w.folder);
        assert_eq!(fields["file_name"], "a.pdf");
        assert!(fields["file_path"].ends_with("/inbox/a.pdf"));
        w.mark_seen(&s, "a.pdf").unwrap();
        assert!(w.poll(&s).unwrap().is_empty());

        // A new watcher (a restart) on the same store: nothing comes again.
        let mut w2 = FolderWatch::new("r", &dir, "*.pdf").unwrap();
        assert!(w2.poll(&s).unwrap().is_empty());
        assert!(w2.poll(&s).unwrap().is_empty());

        // Deleted and written again: it counts again.
        std::fs::remove_file(dir.join("a.pdf")).unwrap();
        assert!(w2.poll(&s).unwrap().is_empty());
        std::fs::write(dir.join("a.pdf"), "2").unwrap();
        w2.poll(&s).unwrap();
        assert_eq!(w2.poll(&s).unwrap().len(), 1);
    }

    #[test]
    fn a_growing_file_waits_until_it_settles() {
        let (s, path) = temp_store("folder-grow");
        let dir = path.parent().unwrap().join("drop");
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = FolderWatch::new("r", &dir, "*").unwrap();
        w.poll(&s).unwrap();
        std::fs::write(dir.join("big.bin"), "12").unwrap();
        assert!(w.poll(&s).unwrap().is_empty());
        std::fs::write(dir.join("big.bin"), "1234").unwrap();
        assert!(
            w.poll(&s).unwrap().is_empty(),
            "size changed: still being written"
        );
        assert_eq!(w.poll(&s).unwrap().len(), 1);
    }
}
