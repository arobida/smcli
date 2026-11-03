use std::{collections::HashSet, hash::{Hash, Hasher}, path::{Path, PathBuf}};

use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;

/// potion-shell-16m (fine-tuned potion-code-16M-v2), shipped as a folder beside the binaries or loaded from a directory. Load once per process; embed in batches.
#[derive(Clone)]
pub struct Potion {
    model: StaticModel,
    pub(crate) vocabulary: HashSet<String>,
    /// Names the model in the index file name, so another model never reuses these vectors.
    pub identity: String,
    dimension: usize,
}

impl Potion {
    pub const NAME: &str = "potion-shell-16m";

    /// The default model: a `potion-shell-16m` folder beside the executable (dist/ or the install prefix),
    /// else the checkout's `Models/` for dev builds and tests. One copy serves both binaries.
    pub fn bundled() -> Result<Self> {
        let directory = Self::bundled_dir().ok_or_else(|| {
            anyhow::anyhow!("{} model not found beside the smcli executable; reinstall, or point `smcli init --potion DIR` at a model", Self::NAME)
        })?;
        Self::load(&directory, "bundled".into())
    }

    fn bundled_dir() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok();
        let beside = |p: Option<PathBuf>| p.and_then(|e| e.parent().map(|d| d.join(Self::NAME)));
        [
            beside(exe.as_ref().and_then(|e| std::fs::canonicalize(e).ok())),
            beside(exe),
            Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Models").join(Self::NAME)),
        ]
        .into_iter()
        .flatten()
        .find(|d| d.join("model.safetensors").is_file())
    }

    /// A model directory (`potionPath` in the config).
    pub fn from_dir(directory: &Path) -> Result<Self> {
        let canonical = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        Self::load(directory, model_identity(&canonical))
    }

    fn load(directory: &Path, identity: String) -> Result<Self> {
        // Preserve the local layouts accepted by StaticModel::from_pretrained.
        let nested = directory.join("0_StaticEmbedding");
        let (config, model_dir) = [
            (directory.join("config.json"), directory),
            (directory.join("config_sentence_transformers.json"), directory),
            (directory.join("config_sentence_transformers.json"), nested.as_path()),
            (directory.parent().unwrap_or(directory).join("config_sentence_transformers.json"), directory),
        ].into_iter().find(|(config, dir)| config.is_file() && dir.join("tokenizer.json").is_file()
            && dir.join("model.safetensors").is_file())
            .ok_or_else(|| anyhow::anyhow!("no valid model layout found in '{}'", directory.display()))?;
        let tokenizer = std::fs::read(model_dir.join("tokenizer.json")).context("failed to read tokenizer.json")?;
        let json: serde_json::Value = serde_json::from_slice(&tokenizer)?;
        let vocabulary = match &json["model"]["vocab"] {
            serde_json::Value::Object(words) => words.keys().cloned().collect(),
            serde_json::Value::Array(words) => words.iter().filter_map(|word| word[0].as_str().map(str::to_owned)).collect(),
            _ => HashSet::new(),
        };
        let model = StaticModel::from_bytes(tokenizer,
            std::fs::read(model_dir.join("model.safetensors")).context("failed to read model.safetensors")?,
            std::fs::read(config).context("failed to read model config")?, None)?;
        let dimension = model.encode_single("smcli").len();
        Ok(Self { model, vocabulary, identity, dimension })
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Normalized vectors. Text with no known tokens gets a fixed vector, leaving its ranking to the BM25 leg.
    pub fn embed(&self, texts: &[String]) -> Vec<Vec<f32>> {
        self.model
            .encode(texts)
            .into_iter()
            .map(|v| {
                if v.iter().any(|x| *x != 0.0) { v } else { vec![1.0 / (v.len() as f32).sqrt(); v.len()] }
            })
            .collect()
    }
}

/// Last path component (short) plus a hash of the full canonical path, so index filenames stay bounded.
fn model_identity(canonical: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    let hash = format!("{:016x}", hasher.finish());
    let slug: String = canonical.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
        .chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(32).collect();
    format!("{}-{hash}", if slug.is_empty() { "model" } else { &slug })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_words_are_protected_from_typo_correction() {
        let potion = Potion::bundled().unwrap();
        let history = ["site", "install"].map(str::to_owned).into_iter().collect();
        let prepare = |query| crate::corpus::prepare_intent_query(query, &history, Some(&potion.vocabulary));
        assert!(potion.vocabulary.contains("suite"));
        assert!(!potion.vocabulary.contains("insall"));
        assert_eq!(prepare("run the project test suite").embedding_query(), "run the project test suite");
        assert_eq!(prepare("build and insall locally").embedding_query(), "build and install locally");
        assert_eq!(crate::corpus::prepare_intent_query("suite", &history, None).embedding_query(), "site");
    }

    #[test]
    fn identity_stays_short_for_a_long_model_path() {
        let last = "m".repeat(245);
        let path = PathBuf::from("/").join("a".repeat(200)).join(&last);
        let id = model_identity(&path);
        assert_eq!(id.len(), 32 + 1 + 16);
        assert!(id.starts_with(&"m".repeat(32)));
        let name = format!("rs-commands-v3-potion-shell-16m-{id}-d256-typed");
        assert!(name.len() <= 255, "{name}");
        assert_ne!(id, model_identity(&PathBuf::from("/").join("b".repeat(200)).join(&last)));
    }
}
