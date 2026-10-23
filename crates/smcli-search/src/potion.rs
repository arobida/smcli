use std::path::Path;

use anyhow::Result;
use model2vec_rs::model::StaticModel;

macro_rules! model_file {
    ($name:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Models/potion-shell-16m/", $name))
    };
}

/// potion-shell-16m (fine-tuned potion-code-16M-v2), embedded in the binary or loaded from a directory. Load once per process; embed in batches.
pub struct Potion {
    model: StaticModel,
    /// Names the model in the index file name, so another model never reuses these vectors.
    pub identity: String,
    dimension: usize,
}

impl Potion {
    pub const NAME: &str = "potion-shell-16m";

    // Loads straight from the embedded bytes, so nothing is written to the runtime dir.
    pub fn bundled() -> Result<Self> {
        let model = StaticModel::from_bytes(
            model_file!("tokenizer.json"),
            model_file!("model.safetensors"),
            model_file!("config.json"),
            None,
        )?;
        Ok(Self::with(model, "bundled".into()))
    }

    /// A model directory (`potionPath` in the config).
    pub fn from_dir(directory: &Path) -> Result<Self> {
        let model = StaticModel::from_pretrained(directory, None, None, None)?;
        let slug: String = directory.to_string_lossy().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        Ok(Self::with(model, slug))
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
