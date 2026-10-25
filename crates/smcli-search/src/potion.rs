use std::{hash::{Hash, Hasher}, path::{Path, PathBuf}};

use anyhow::Result;
use model2vec_rs::model::StaticModel;

/// potion-shell-16m (fine-tuned potion-code-16M-v2), shipped as a folder beside the binaries or loaded from a directory. Load once per process; embed in batches.
pub struct Potion {
    model: StaticModel,
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
        let model = StaticModel::from_pretrained(&directory, None, None, None)?;
        Ok(Self::with(model, "bundled".into()))
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
        let model = StaticModel::from_pretrained(directory, None, None, None)?;
        let canonical = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        let slug: String = canonical.to_string_lossy().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut hasher);
        Ok(Self::with(model, format!("{slug}-{:016x}", hasher.finish())))
    }

    fn with(model: StaticModel, identity: String) -> Self {
        let dimension = model.encode_single("smcli").len();
        Self { model, identity, dimension }
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
