//! The command-completion index behind `@@`: examples and vectors per program,
//! built from smcli's program set (OS-shipped ∪ history-seen, ∩ PATH) and pluggable sources.
//! Kept beside the history search index and named by model identity + source versions.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use smcli_core::command_source::{CommandExample, CommandSource};

use crate::Potion;

/// Directories whose executables the OS ships. macOS: sealed system volume only
/// (`/usr/libexec` tools are reached by absolute path, not as programs on PATH).
/// Linux is best-effort: same dirs, which includes distro-package tools.
const SYSTEM_DIRS: &[&str] = if cfg!(target_os = "macos") {
    &["/bin", "/sbin", "/usr/bin", "/usr/sbin"]
} else {
    &["/bin", "/sbin", "/usr/bin", "/usr/sbin", "/usr/local/bin", "/usr/local/sbin"]
};

/// The index directory is private (0700), matching the history search index.
fn create_private_dir(directory: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(directory)?;
    Ok(())
}

/// Executables directly in `system_dirs` that also resolve on `path`
/// (`tldr::installed` semantics: executable file or symlink to one).
/// `system_dirs` is injectable so tests can stand in temp dirs.
pub fn os_programs(system_dirs: &[PathBuf], path: &str) -> HashSet<String> {
    use std::os::unix::fs::PermissionsExt;
    let names: HashSet<String> = system_dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            // Follows symlinks: macOS ships /usr/bin/tar -> bsdtar.
            std::fs::metadata(entry.path())
                .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    crate::tldr::installed(&names, path)
}

pub fn system_dirs() -> Vec<PathBuf> {
    SYSTEM_DIRS.iter().map(PathBuf::from).collect()
}

/// One indexed example; `description` is embedded, `template` is what `@@` inserts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedExample {
    pub program: String,
    pub description: String,
    pub template: String,
    pub source: String,
    pub embedding: Vec<f32>,
}

/// The persistent index of command examples, one TalaDB file named
/// `rs-commands-{model}-{sources}-d{dim}` next to the history search index.
pub struct CommandIndex {
    pub directory: PathBuf,
    pub potion: Potion,
    /// Sorted `name@version` of every source, part of the index name.
    sources: Vec<String>,
}

impl CommandIndex {
    pub fn new(directory: impl Into<PathBuf>, potion: Potion, sources: &[&dyn CommandSource]) -> Self {
        let sources = sources
            .iter()
            .map(|source| format!("{}@{}", source.name(), source.version()))
            .collect::<Vec<_>>();
        Self { directory: directory.into(), potion, sources }
    }

    pub fn index_name(&self) -> String {
        format!(
            "rs-commands-{}-{}-{}-d{}",
            Potion::NAME,
            self.potion.identity,
            self.sources.join("_"),
            self.potion.dimension()
        )
    }

    /// The index file; callers gate a full build on its absence.
    pub fn path(&self) -> PathBuf {
        self.directory.join(format!("{}.taladb", self.index_name()))
    }

    fn open(&self) -> Result<taladb::Database> {
        if self.path().exists() {
            return Ok(taladb::open(self.path())?);
        }
        create_private_dir(&self.directory)?;
        Ok(taladb::open(self.path())?)
    }

    fn collection(&self, db: &taladb::Database) -> Result<taladb::TypedCollection<IndexedExample>> {
        let examples = db.typed::<IndexedExample>("examples")?;
        examples.create_index("program")?;
        examples.create_vector_index("embedding", self.potion.dimension(), None, None)?;
        Ok(examples)
    }

    /// Examples for one program, `None` when the program is not indexed.
    pub fn examples(&self, program: &str) -> Result<Option<Vec<IndexedExample>>> {
        if !self.path().exists() {
            return Ok(None);
        }
        let db = self.open()?;
        let examples = self.collection(&db)?;
        let mut found = examples.find(json!({"program": program}))?;
        if found.is_empty() {
            return Ok(None);
        }
        found.sort_by(|a, b| a.description.cmp(&b.description).then_with(|| a.source.cmp(&b.source)));
        Ok(Some(found))
    }

    /// Every indexed example in one read, sorted like `examples`.
    pub fn all(&self) -> Result<Vec<IndexedExample>> {
        if !self.path().exists() {
            return Ok(Vec::new());
        }
        let db = self.open()?;
        let mut found = self.collection(&db)?.find(json!({}))?;
        found.sort_by(|a, b| a.program.cmp(&b.program).then_with(|| a.description.cmp(&b.description)).then_with(|| a.source.cmp(&b.source)));
        Ok(found)
    }

    /// Delete every example for `program` (e.g. it no longer resolves on PATH).
    pub fn remove(&self, program: &str) -> Result<()> {
        if !self.path().exists() {
            return Ok(());
        }
        let db = self.open()?;
        self.collection(&db)?.delete_many(json!({"program": program}))?;
        Ok(())
    }

    /// Programs currently in the index.
    pub fn programs(&self) -> Result<HashSet<String>> {
        Ok(self.all()?.into_iter().map(|example| example.program).collect())
    }

    /// Ask every source for `program`'s examples; `None` when no source has any.
    /// `platform` is forwarded for sources that keep a per-platform corpus.
    pub fn gather(sources: &[&dyn CommandSource], program: &str, platform: &str) -> Option<Vec<CommandExample>> {
        let examples: Vec<CommandExample> = sources
            .iter()
            .flat_map(|source| source.examples(program, platform))
            .collect();
        (!examples.is_empty()).then_some(examples)
    }

    /// Embed and add one program's examples to the index. Existing rows for the
    /// program are replaced first, so a capture can safely retry.
    pub fn add(&self, program: &str, examples: &[CommandExample]) -> Result<()> {
        self.write(Some(program), examples)
    }

    /// Insert examples for a fresh build (no existing rows to replace).
    pub fn add_all(&self, examples: &[CommandExample]) -> Result<()> {
        self.write(None, examples)
    }

    fn write(&self, replace: Option<&str>, examples: &[CommandExample]) -> Result<()> {
        create_private_dir(&self.directory)?;
        let lock = crate::index::lock_index_directory(&self.directory, 20)?
            .ok_or_else(|| anyhow::anyhow!("command index is busy; try again"))?;
        let _ = lock;
        let db = self.open()?;
        let collection = self.collection(&db)?;
        if let Some(program) = replace {
            collection.delete_many(json!({"program": program}))?;
        }
        let vectors = self.potion.embed(&examples.iter().map(|example| example.description.clone()).collect::<Vec<_>>());
        collection.insert_many(
            &examples
                .iter()
                .zip(vectors)
                .map(|(example, embedding)| IndexedExample {
                    program: example.program.clone(),
                    description: example.description.clone(),
                    template: example.template.clone(),
                    source: example.source.clone(),
                    embedding,
                })
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }
}

/// Sync the index to the wanted program set: add programs with examples that are missing,
/// delete indexed programs no longer wanted. Returns (indexed, example-less) wanted programs,
/// so the caller can remember example-less programs without another lookup.
pub fn sync(
    index: &CommandIndex,
    sources: &[&dyn CommandSource],
    platform: &str,
    wanted: &HashSet<String>,
) -> Result<(HashSet<String>, HashSet<String>)> {
    let current = index.programs()?;
    let mut indexed = HashSet::new();
    let mut empty = HashSet::new();
    for program in wanted {
        if current.contains(program) {
            indexed.insert(program.clone());
            continue;
        }
        match CommandIndex::gather(sources, program, platform) {
            Some(examples) => {
                index.add(program, &examples)?;
                indexed.insert(program.clone());
            }
            None => {
                empty.insert(program.clone());
            }
        }
    }
    for program in current.difference(wanted) {
        index.remove(program)?;
    }
    Ok((indexed, empty))
}

/// Build the whole index from a program set: every program any source has
/// examples for gets embedded once. Returns indexed programs and programs
/// with no examples from any source (callers may remember the latter).
pub fn build(
    index: &CommandIndex,
    sources: &[&dyn CommandSource],
    platform: &str,
    programs: &HashSet<String>,
) -> Result<(HashSet<String>, HashSet<String>)> {
    let mut indexed = HashSet::new();
    let mut empty = HashSet::new();
    // Embed in batches across programs so ~1000 binaries do not queue one giant call.
    let mut pending: Vec<(String, Vec<CommandExample>)> = Vec::new();
    for program in programs {
        match CommandIndex::gather(sources, program, platform) {
            Some(examples) => pending.push((program.clone(), examples)),
            None => {
                empty.insert(program.clone());
            }
        }
    }
    // One lock, one embed call and one write: per-program commits made the build I/O-bound (~20 s).
    let all: Vec<CommandExample> = pending.iter().flat_map(|(_, examples)| examples.iter().cloned()).collect();
    index.add_all(&all)?;
    indexed.extend(pending.into_iter().map(|(program, _)| program));
    Ok((indexed, empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn exe(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "").unwrap();
        std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Platform filter inside the source, like tldr.
    struct PlatformSource;
    impl CommandSource for PlatformSource {
        fn name(&self) -> &str {
            "stub"
        }
        fn version(&self) -> &str {
            "1"
        }
        fn examples(&self, program: &str, platform: &str) -> Vec<CommandExample> {
            if program == "onlylinux" && platform != "linux" {
                return Vec::new();
            }
            (program == "tar" || program == "onlylinux")
                .then(|| {
                    vec![CommandExample {
                        program: program.into(),
                        description: format!("{program} example"),
                        template: format!("{program} @"),
                        source: self.name().into(),
                    }]
                })
                .unwrap_or_default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn os_programs_keeps_system_dirs_intersected_with_path() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let other = root.path().join("brew");
        std::fs::create_dir(&system).unwrap();
        std::fs::create_dir(&other).unwrap();
        exe(&system, "tar");
        exe(&system, "zsh");
        std::fs::write(other.join("jq"), "").unwrap();
        std::fs::set_permissions(other.join("jq"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let system_dirs = [system.clone()];
        let both = std::env::join_paths([&system, &other]).unwrap();
        let found = os_programs(&system_dirs, &both.to_string_lossy());
        assert_eq!(found, HashSet::from(["tar".to_owned(), "zsh".to_owned()]));
        // PATH without the system dir: nothing resolves.
        let found = os_programs(&system_dirs, &other.to_string_lossy());
        assert!(found.is_empty());
    }

    #[test]
    fn sync_adds_missing_and_removes_unwanted_programs() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let potion = Potion::bundled().unwrap();
        let source = PlatformSource;
        let sources: Vec<&dyn CommandSource> = vec![&source];
        let index = CommandIndex::new(root.path(), potion, &sources);
        // Pre-existing program that will no longer be wanted.
        index.add("tar", &CommandIndex::gather(&sources, "tar", "osx").unwrap()).unwrap();
        let wanted = HashSet::from(["tar".to_owned(), "noexamples".to_owned()]);
        let (indexed, empty) = sync(&index, &sources, "osx", &wanted).unwrap();
        assert_eq!(indexed, HashSet::from(["tar".to_owned()]));
        assert_eq!(empty, HashSet::from(["noexamples".to_owned()]));
        assert!(index.examples("tar").unwrap().is_some());
        // tar falls out of the wanted set: its examples are deleted.
        let (indexed, empty) = sync(&index, &sources, "osx", &HashSet::new()).unwrap();
        assert!(indexed.is_empty() && empty.is_empty());
        assert!(index.examples("tar").unwrap().is_none());
        assert!(index.programs().unwrap().is_empty());
    }

    #[test]
    fn build_indexes_only_programs_with_examples_and_honors_platform() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let potion = Potion::bundled().unwrap();
        let source = PlatformSource;
        let sources: Vec<&dyn CommandSource> = vec![&source];
        let index = CommandIndex::new(root.path(), potion, &sources);
        assert!(index.index_name().contains("stub@1"));
        let programs = HashSet::from(["tar".to_owned(), "noexamples".to_owned(), "onlylinux".to_owned()]);
        let (indexed, empty) = build(&index, &sources, "osx", &programs).unwrap();
        assert_eq!(indexed, HashSet::from(["tar".to_owned()]));
        assert_eq!(empty, HashSet::from(["noexamples".to_owned(), "onlylinux".to_owned()]));
        let tar = index.examples("tar").unwrap().unwrap();
        assert_eq!(tar.len(), 1);
        assert_eq!(tar[0].source, "stub");
        assert!(index.examples("noexamples").unwrap().is_none());
        assert!(index.examples("onlylinux").unwrap().is_none());
        assert_eq!(index.programs().unwrap(), HashSet::from(["tar".to_owned()]));
    }
}
