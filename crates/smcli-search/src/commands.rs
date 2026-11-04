//! The command-completion index behind `@@`: examples and vectors per program,
//! built from smcli's program set (OS-shipped ∪ history-seen, ∩ PATH) and pluggable sources.
//! Kept beside the history search index and named by model identity + source versions.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use smcli_core::command_source::{CommandExample, CommandSource};

use crate::{ModelId, Potion};

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

/// The bundled example sources in priority order: tldr first, cheat/cheatsheets as the fallback
/// for programs tldr has no page for. `None` when the required tldr data is missing or unreadable
/// (cheat is optional: its absence just leaves tldr alone). One place builds this list so the CLI
/// and the service agree on sources and therefore on the index name.
pub fn bundled_sources() -> Option<Vec<Box<dyn CommandSource>>> {
    let platform = crate::tldr::current_platform();
    let dir = crate::tldr::bundled_dir()?;
    sources_from(&dir, platform, crate::cheat::bundled_dir())
}

/// tldr (required) plus the cheat fallback when its data loads; `None` only when tldr fails.
fn sources_from(
    tldr_dir: &Path,
    platform: &str,
    cheat_dir: Option<PathBuf>,
) -> Option<Vec<Box<dyn CommandSource>>> {
    let mut sources: Vec<Box<dyn CommandSource>> = vec![Box::new(crate::tldr::TldrSource::new(tldr_dir, platform).ok()?)];
    if let Some(dir) = cheat_dir {
        if let Ok(cheat) = crate::cheat::CheatSource::new(&dir) {
            sources.push(Box::new(cheat));
        }
    }
    Some(sources)
}

/// Borrow the boxed sources as the trait-object slices `CommandIndex` takes.
pub fn source_refs(sources: &[Box<dyn CommandSource>]) -> Vec<&dyn CommandSource> {
    sources.iter().map(|source| &**source).collect()
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
    pub model: ModelId,
    pub potion: Option<Potion>,
    /// Sorted `name@version` of every source, part of the index name.
    sources: Vec<String>,
}

#[cfg(test)]
thread_local! {
    static BEFORE_INSERT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

impl CommandIndex {
    pub fn new(directory: impl Into<PathBuf>, potion: Potion, sources: &[&dyn CommandSource]) -> Self {
        let mut index = Self::reader(directory, potion.model_id(), sources);
        index.potion = Some(potion);
        index
    }

    pub fn reader(directory: impl Into<PathBuf>, model: ModelId, sources: &[&dyn CommandSource]) -> Self {
        let sources = sources
            .iter()
            .map(|source| format!("{}@{}", source.name(), source.version()))
            .collect::<Vec<_>>();
        Self { directory: directory.into(), model, potion: None, sources }
    }

    fn writable(&self) -> Result<&Potion> {
        self.potion.as_ref().ok_or_else(|| anyhow::anyhow!("command index is read-only here"))
    }

    pub fn index_name(&self) -> String {
        format!(
            "rs-commands-{}-{}-{}-d{}",
            Potion::NAME,
            self.model.identity,
            self.sources.join("_"),
            self.model.dimension
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
        examples.create_vector_index("embedding", self.model.dimension, None, None)?;
        Ok(examples)
    }

    /// Examples for one program, `None` when the program is not indexed.
    pub fn examples(&self, program: &str) -> Result<Option<Vec<IndexedExample>>> {
        if !self.path().exists() {
            return Ok(None);
        }
        let db = self.open()?;
        let examples = db.typed::<IndexedExample>("examples")?;
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
        let mut found = db.typed::<IndexedExample>("examples")?.find(json!({}))?;
        found.sort_by(|a, b| a.program.cmp(&b.program).then_with(|| a.description.cmp(&b.description)).then_with(|| a.source.cmp(&b.source)));
        Ok(found)
    }

    /// Delete every example for `program` (e.g. it no longer resolves on PATH).
    pub fn remove(&self, program: &str) -> Result<()> {
        self.writable()?;
        if !self.path().exists() {
            return Ok(());
        }
        let db = self.open()?;
        db.typed::<IndexedExample>("examples")?.delete_many(json!({"program": program}))?;
        Ok(())
    }

    /// Programs currently in the index.
    pub fn programs(&self) -> Result<HashSet<String>> {
        Ok(self.all()?.into_iter().map(|example| example.program).collect())
    }

    /// Ask sources in priority order for `program`'s examples; the first source with any wins.
    /// `None` when no source has any. `platform` is forwarded for sources that keep a per-platform corpus.
    pub fn gather(sources: &[&dyn CommandSource], program: &str, platform: &str) -> Option<Vec<CommandExample>> {
        sources.iter().find_map(|source| {
            let examples = source.examples(program, platform);
            (!examples.is_empty()).then_some(examples)
        })
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
        let potion = self.writable()?;
        create_private_dir(&self.directory)?;
        let lock = crate::index::lock_index_directory(&self.directory, 20)?
            .ok_or_else(|| anyhow::anyhow!("command index is busy; try again"))?;
        let _ = lock;
        let path = self.path();
        // A new index becomes visible only once its schema and rows have committed.
        let target = if path.exists() { path.clone() } else { path.with_extension("building") };
        if target != path && target.exists() { std::fs::remove_file(&target)?; }
        let result = (|| -> Result<()> {
            let db = taladb::open(&target)?;
            let collection = self.collection(&db)?;
            if let Some(program) = replace {
                collection.delete_many(json!({"program": program}))?;
            }
            #[cfg(test)]
            BEFORE_INSERT.with(|hook| { if let Some(hook) = hook.borrow_mut().take() { hook(); } });
            let vectors = potion.embed(&examples.iter().map(|example| example.description.clone()).collect::<Vec<_>>());
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
            db.flush()?;
            Ok(())
        })();
        if target != path {
            if result.is_ok() { std::fs::rename(&target, &path)?; }
            else { let _ = std::fs::remove_file(&target); }
        }
        result
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
    index.writable()?;
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
    index.writable()?;
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

/// Add `program` when a source has examples for it and the index lacks it, so an interrupted build
/// heals on use. Returns whether it was added.
pub fn ensure(index: &CommandIndex, sources: &[&dyn CommandSource], platform: &str, program: &str) -> Result<bool> {
    index.writable()?;
    if index.programs()?.contains(program) {
        return Ok(false);
    }
    let Some(examples) = CommandIndex::gather(sources, program, platform) else { return Ok(false) };
    index.add(program, &examples)?;
    Ok(true)
}

/// Build a missing index and heal an installed requested program, using the caller's PATH.
pub fn prepare(index: &CommandIndex, sources: &[&dyn CommandSource], platform: &str, path: &str,
               captured_programs: &HashSet<String>, rebuild: bool, program: Option<&str>) -> Result<()> {
    index.writable()?;
    if rebuild && index.path().exists() {
        std::fs::remove_file(index.path())?;
    }
    if !index.path().exists() {
        let mut programs = os_programs(&system_dirs(), path);
        programs.extend(crate::tldr::installed(captured_programs, path));
        build(index, sources, platform, &programs)?;
    }
    if let Some(program) = program.filter(|p| !crate::tldr::installed(&HashSet::from([p.to_string()]), path).is_empty()) {
        ensure(index, sources, platform, program)?;
    }
    Ok(())
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

    /// Lower-priority stub covering `tar` differently and `onlyfallback`, which PlatformSource lacks.
    struct FallbackSource;
    impl CommandSource for FallbackSource {
        fn name(&self) -> &str {
            "fallback"
        }
        fn version(&self) -> &str {
            "2"
        }
        fn examples(&self, program: &str, _platform: &str) -> Vec<CommandExample> {
            (program == "tar" || program == "onlyfallback")
                .then(|| {
                    vec![CommandExample {
                        program: program.into(),
                        description: format!("{program} fallback"),
                        template: format!("{program} --fallback"),
                        source: self.name().into(),
                    }]
                })
                .unwrap_or_default()
        }
    }

    #[test]
    fn gather_uses_the_first_source_with_examples() {
        let first = PlatformSource;
        let second = FallbackSource;
        let sources: Vec<&dyn CommandSource> = vec![&first, &second];
        // Both have tar: only the higher-priority source's examples count.
        let tar = CommandIndex::gather(&sources, "tar", "osx").unwrap();
        assert_eq!(tar.len(), 1);
        assert_eq!(tar[0].source, "stub");
        // Missing from the first source: falls back to the second.
        let fallback = CommandIndex::gather(&sources, "onlyfallback", "osx").unwrap();
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].source, "fallback");
        // No source has it.
        assert!(CommandIndex::gather(&sources, "noexamples", "osx").is_none());
    }

    /// Selects a PATH candidate with production filtering and indexes that selected set.
    #[cfg(unix)]
    fn build_path_program(sources: &[&dyn CommandSource], program: &str) -> (String, HashSet<String>, Vec<IndexedExample>) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join(program);
        std::fs::write(&executable, "#!/bin/sh\\nexit 0\\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let candidates = os_programs(&[dir.path().to_path_buf()], &dir.path().to_string_lossy());
        let wanted = HashSet::from([program.to_owned()]).intersection(&candidates).cloned().collect::<HashSet<_>>();
        assert_eq!(wanted, HashSet::from([program.to_owned()]));
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let index = CommandIndex::new(root.path(), Potion::bundled().unwrap(), sources);
        build(&index, sources, crate::tldr::current_platform(), &wanted).unwrap();
        (index.index_name(), wanted, index.examples(program).unwrap().unwrap())
    }

    /// A cheat-only program's examples land in the index naming `cheat` as their source.
    #[cfg(unix)]
    #[test]
    fn build_indexes_cheat_only_program_from_the_cheat_source() {
        let tldr_dir = crate::tldr::bundled_dir().expect("bundled tldr data");
        let cheat_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            cheat_dir.path().join("cheat.jsonl"),
            "{\"version\": 1, \"cheat_commit\": \"testcommit\", \"license\": \"CC0-1.0\"}\n\n            {\"program\": \"zzz-cheatexample\", \"platform\": \"common\", \"description\": \"Run it\", \"template\": \"zzz-cheatexample --flag\"}\n",
        )
        .unwrap();
        let sources = super::sources_from(&tldr_dir, "osx", Some(cheat_dir.path().to_path_buf())).unwrap();
        let refs = super::source_refs(&sources);
        let (name, selected, indexed) = build_path_program(&refs, "zzz-cheatexample");
        assert!(selected.contains("zzz-cheatexample"));
        assert!(indexed.iter().all(|example| example.source == "cheat"));
        assert!(name.contains("cheat@testcommit"));
    }

    /// Missing or corrupt cheat data leaves a tldr-only source list: the index name has no cheat part.
    #[test]
    fn build_falls_back_to_tldr_alone_when_cheat_data_is_missing_or_corrupt() {
        let tldr_dir = crate::tldr::bundled_dir().expect("bundled tldr data");
        let platform = crate::tldr::current_platform();
        for corrupt in [false, true] {
            let cheat_dir = tempfile::tempdir().unwrap();
            if corrupt {
                std::fs::write(cheat_dir.path().join("cheat.jsonl"), "not json\n{{{\n").unwrap();
            }
            let sources = super::sources_from(&tldr_dir, platform, Some(cheat_dir.path().to_path_buf())).unwrap();
            assert_eq!(sources.len(), 1);
            assert_eq!(sources[0].name(), "tldr");
            let refs = super::source_refs(&sources);
            let (name, selected, examples) = build_path_program(&refs, "tar");
            assert!(selected.contains("tar"));
            assert!(!name.contains("cheat"));
            assert!(name.contains("tldr@"));
            assert!(!examples.is_empty());
            assert!(examples.iter().all(|example| example.source == "tldr"));
        }
        // No cheat directory at all: same tldr-only outcome.
        let sources = super::sources_from(&tldr_dir, platform, None).unwrap();
        assert_eq!(sources.len(), 1);
    }

    /// The bundled sources in priority order against the real bundled data: tldr first,
    /// tar comes only from tldr, and a program tldr lacks (acl) falls back to cheat.
    #[test]
    fn bundled_sources_are_ordered_and_fallback() {
        let Some(sources) = super::bundled_sources() else {
            eprintln!("bundled tldr data not available; skipping");
            return;
        };
        assert_eq!(sources[0].name(), "tldr");
        let refs = super::source_refs(&sources);
        if crate::cheat::bundled_dir().is_some() {
            assert_eq!(sources[1].name(), "cheat");
            let tar = CommandIndex::gather(&refs, "tar", "osx").expect("tar examples");
            assert!(tar.iter().all(|example| example.source == "tldr"));
            let acl = CommandIndex::gather(&refs, "acl", "osx").expect("acl cheat examples");
            assert!(acl.iter().all(|example| example.source == "cheat"));
        } else {
            assert_eq!(sources.len(), 1);
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

    #[test]
    fn command_index_publishes_only_after_build_finishes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let potion = Potion::bundled().unwrap();
        let reader = CommandIndex::reader(root.path(), potion.model_id(), &[]);
        let index = CommandIndex::new(root.path(), potion, &[]);
        let (staged, waiting) = std::sync::mpsc::channel();
        let (resume, continued) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            BEFORE_INSERT.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
                staged.send(()).unwrap();
                continued.recv().unwrap();
            })));
            index.add_all(&[CommandExample { program: "tar".into(), description: "Extract archive".into(),
                template: "tar xf @".into(), source: "test".into() }]).unwrap();
        });
        waiting.recv().unwrap();
        assert!(reader.path().with_extension("building").exists());
        assert!(!reader.path().exists(), "a concurrent picker must still request prepare");
        assert!(reader.all().unwrap().is_empty());
        resume.send(()).unwrap();
        writer.join().unwrap();
        assert!(!reader.path().with_extension("building").exists());
        assert_eq!(reader.all().unwrap().len(), 1);
        assert!(reader.examples("tar").unwrap().is_some());
    }

    #[test]
    fn command_index_reader_does_not_create_indexes() {
        let root = tempfile::tempdir().unwrap();
        let reader = CommandIndex::reader(root.path(), ModelId { identity: "test".into(), dimension: 2 }, &[]);
        // Rows without secondary/vector indexes expose any accidental schema creation by reads.
        let db = taladb::open(reader.path()).unwrap();
        db.typed::<IndexedExample>("examples").unwrap().insert_many(&[IndexedExample {
            program: "tar".into(), description: "Extract archive".into(), template: "tar xf @".into(),
            source: "test".into(), embedding: vec![1.0, 0.0],
        }]).unwrap();
        drop(db);
        assert!(reader.examples("tar").unwrap().is_some());
        assert_eq!(reader.all().unwrap().len(), 1);
        let db = taladb::open(reader.path()).unwrap();
        let indexes = db.collection("examples").unwrap().list_indexes().unwrap();
        assert!(indexes.btree.is_empty() && indexes.vector.is_empty() && indexes.fts.is_empty(),
            "reads must not create schema indexes");
    }

    #[test]
    fn command_index_reader_reads_and_refuses_writes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let source = PlatformSource;
        let sources: Vec<&dyn CommandSource> = vec![&source];
        let potion = Potion::bundled().unwrap();
        let model = potion.model_id();
        let index = CommandIndex::new(root.path(), potion, &sources);
        ensure(&index, &sources, "osx", "tar").unwrap();
        let reader = CommandIndex::reader(root.path(), model, &sources);
        assert_eq!(reader.all().unwrap().len(), 1);
        assert!(reader.examples("tar").unwrap().is_some());
        let examples = CommandIndex::gather(&sources, "tar", "osx").unwrap();
        for result in [reader.add("tar", &examples), reader.add_all(&examples), reader.remove("tar"),
            sync(&reader, &sources, "osx", &HashSet::new()).map(|_| ()),
            build(&reader, &sources, "osx", &HashSet::new()).map(|_| ()),
            ensure(&reader, &sources, "osx", "tar").map(|_| ()),
            prepare(&reader, &sources, "osx", "", &HashSet::new(), true, None)] {
            assert_eq!(result.unwrap_err().to_string(), "command index is read-only here");
        }
        assert_eq!(reader.all().unwrap().len(), 1);
    }

    #[test]
    fn prepare_builds_missing_index_and_ensures_program() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let source = PlatformSource;
        let sources: Vec<&dyn CommandSource> = vec![&source];
        let index = CommandIndex::new(root.path(), Potion::bundled().unwrap(), &sources);
        let bin = tempfile::tempdir().unwrap();
        exe(bin.path(), "onlylinux");
        let path = bin.path().to_str().unwrap();
        prepare(&index, &sources, "linux", path, &HashSet::from(["onlylinux".into()]), false, None).unwrap();
        assert_eq!(index.programs().unwrap(), HashSet::from(["onlylinux".into()]));
        exe(bin.path(), "tar");
        prepare(&index, &sources, "linux", path, &HashSet::new(), false, Some("tar")).unwrap();
        assert!(index.examples("tar").unwrap().is_some());
        prepare(&index, &sources, "linux", path, &HashSet::new(), true, Some("tar")).unwrap();
        assert!(!ensure(&index, &sources, "osx", "tar").unwrap(), "already indexed: nothing to do");
        assert!(!ensure(&index, &sources, "osx", "noexamples").unwrap());
        assert_eq!(index.programs().unwrap(), HashSet::from(["tar".to_owned()]));
    }
}
