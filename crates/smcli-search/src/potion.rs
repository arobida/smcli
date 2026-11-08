use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelId {
    pub identity: String,
    pub dimension: usize,
}

impl Potion {
    pub fn model_id(&self) -> ModelId {
        ModelId {
            identity: self.identity.clone(),
            dimension: self.dimension,
        }
    }

    /// Resolved config, tokenizer and weights, in identity-hash order; also used by the service's stat memo.
    pub fn model_files(potion_path: Option<&Path>) -> Result<[PathBuf; 3]> {
        let bundled;
        let directory = match potion_path {
            Some(directory) => directory,
            None => {
                bundled = Self::bundled_dir().ok_or_else(|| anyhow::anyhow!("{} model not found beside the smcli executable; reinstall, or point `smcli init --potion DIR` at a model", Self::NAME))?;
                &bundled
            }
        };
        let (config, model_dir) = layout(directory)?;
        Ok([
            config,
            model_dir.join("tokenizer.json"),
            model_dir.join("model.safetensors"),
        ])
    }

    /// Stream the model's content identity and inspect its header without constructing the embedding matrix.
    pub fn describe(potion_path: Option<&Path>) -> Result<ModelId> {
        let [config, tokenizer, weights] = Self::model_files(potion_path)?;
        Self::describe_open_files([
            std::fs::File::open(config)?,
            std::fs::File::open(tokenizer)?,
            std::fs::File::open(weights)?,
        ])
    }

    fn describe_open_files(files: [std::fs::File; 3]) -> Result<ModelId> {
        use std::io::Read;
        let mut header = Vec::new();
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0; 1024 * 1024];
        for (index, mut file) in files.into_iter().enumerate() {
            let length = file.metadata()?.len();
            hasher.update(&length.to_le_bytes());
            let mut read = 0u64;
            if index == 2 {
                let mut prefix = [0; 8];
                file.read_exact(&mut prefix)
                    .context("missing safetensors header length")?;
                let header_length = u64::from_le_bytes(prefix);
                anyhow::ensure!(
                    header_length <= 1024 * 1024,
                    "safetensors header exceeds 1 MiB"
                );
                header.resize(header_length as usize, 0);
                file.read_exact(&mut header)
                    .context("missing safetensors header")?;
                hasher.update(&prefix);
                hasher.update(&header);
                read = 8 + header_length;
            }
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
                read += count as u64;
            }
            anyhow::ensure!(read == length, "model file changed while hashing");
        }
        let header: serde_json::Value = serde_json::from_slice(&header)?;
        let tensor = ["embeddings", "0", "embedding.weight"]
            .into_iter()
            .find_map(|name| header.get(name))
            .ok_or_else(|| anyhow::anyhow!("embeddings tensor not found"))?;
        let shape = tensor["shape"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid embeddings shape"))?;
        anyhow::ensure!(shape.len() == 2, "invalid embeddings shape");
        let dimension = shape[1]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| anyhow::anyhow!("invalid embedding dimension"))?;
        let identity = finish_identity(hasher);
        Ok(ModelId {
            identity,
            dimension,
        })
    }

    pub const NAME: &str = "potion-shell-16m";

    /// The default model: a `potion-shell-16m` folder beside the executable (dist/ or the install prefix),
    /// else the checkout's `Models/` for dev builds and tests. One copy serves both binaries.
    pub fn bundled() -> Result<Self> {
        let directory = Self::bundled_dir().ok_or_else(|| {
            anyhow::anyhow!("{} model not found beside the smcli executable; reinstall, or point `smcli init --potion DIR` at a model", Self::NAME)
        })?;
        Self::load(&directory)
    }

    fn bundled_dir() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok();
        let beside = |p: Option<PathBuf>| p.and_then(|e| e.parent().map(|d| d.join(Self::NAME)));
        [
            beside(exe.as_ref().and_then(|e| std::fs::canonicalize(e).ok())),
            beside(exe),
            Some(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../Models")
                    .join(Self::NAME),
            ),
        ]
        .into_iter()
        .flatten()
        .find(|d| d.join("model.safetensors").is_file())
    }

    /// A model directory (`potionPath` in the config).
    pub fn from_dir(directory: &Path) -> Result<Self> {
        Self::load(directory)
    }

    fn load(directory: &Path) -> Result<Self> {
        let [config_path, tokenizer_path, weights_path] = Self::model_files(Some(directory))?;
        let config = std::fs::read(config_path).context("failed to read model config")?;
        let tokenizer = std::fs::read(tokenizer_path).context("failed to read tokenizer.json")?;
        let weights = std::fs::read(weights_path).context("failed to read model.safetensors")?;
        let mut hasher = blake3::Hasher::new();
        for bytes in [&config, &tokenizer, &weights] {
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        let identity = finish_identity(hasher);
        let json: serde_json::Value = serde_json::from_slice(&tokenizer)?;
        let vocabulary = match &json["model"]["vocab"] {
            serde_json::Value::Object(words) => words.keys().cloned().collect(),
            serde_json::Value::Array(words) => words
                .iter()
                .filter_map(|word| word[0].as_str().map(str::to_owned))
                .collect(),
            _ => HashSet::new(),
        };
        let model = StaticModel::from_bytes(tokenizer, weights, config, None)?;
        let dimension = model.encode_single("smcli").len();
        Ok(Self {
            model,
            vocabulary,
            identity,
            dimension,
        })
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
                if v.iter().any(|x| *x != 0.0) {
                    v
                } else {
                    vec![1.0 / (v.len() as f32).sqrt(); v.len()]
                }
            })
            .collect()
    }
}

/// Preserve the local layouts accepted by StaticModel::from_pretrained.
fn layout(directory: &Path) -> Result<(PathBuf, PathBuf)> {
    let nested = directory.join("0_StaticEmbedding");
    [
        (directory.join("config.json"), directory.to_path_buf()),
        (
            directory.join("config_sentence_transformers.json"),
            directory.to_path_buf(),
        ),
        (directory.join("config_sentence_transformers.json"), nested),
        (
            directory
                .parent()
                .unwrap_or(directory)
                .join("config_sentence_transformers.json"),
            directory.to_path_buf(),
        ),
    ]
    .into_iter()
    .find(|(config, dir)| {
        config.is_file()
            && dir.join("tokenizer.json").is_file()
            && dir.join("model.safetensors").is_file()
    })
    .ok_or_else(|| anyhow::anyhow!("no valid model layout found in '{}'", directory.display()))
}

fn finish_identity(hasher: blake3::Hasher) -> String {
    format!("b3{}", &hasher.finalize().to_hex()[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_identity_matches_describe() {
        let potion = Potion::bundled().unwrap();
        let model = Potion::describe(None).unwrap();
        assert_eq!(model, potion.model_id());
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            crate::index_name(&model),
            crate::HistorySearch::new(root.path(), potion.clone()).index_name()
        );
        assert_eq!(
            crate::commands::CommandIndex::reader(root.path(), model, &[]).index_name(),
            crate::commands::CommandIndex::new(root.path(), potion, &[]).index_name()
        );
        let directory = Potion::bundled_dir().unwrap();
        assert_eq!(
            Potion::describe(Some(&directory)).unwrap(),
            Potion::from_dir(&directory).unwrap().model_id()
        );
    }

    #[test]
    fn describe_reads_header_without_weights() {
        let root = tempfile::tempdir().unwrap();
        let source = Potion::bundled_dir().unwrap();
        let (config, model_dir) = layout(&source).unwrap();
        std::fs::copy(config, root.path().join("config.json")).unwrap();
        std::fs::copy(
            model_dir.join("tokenizer.json"),
            root.path().join("tokenizer.json"),
        )
        .unwrap();
        let bytes = std::fs::read(model_dir.join("model.safetensors")).unwrap();
        let length = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
        std::fs::write(root.path().join("model.safetensors"), &bytes[..8 + length]).unwrap();
        assert_eq!(
            Potion::describe(Some(root.path())).unwrap().dimension,
            Potion::bundled().unwrap().dimension()
        );
        assert!(Potion::from_dir(root.path()).is_err());
    }

    #[test]
    fn describe_rejects_oversized_or_missing_header() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.json"), "{}").unwrap();
        std::fs::write(root.path().join("tokenizer.json"), "{}").unwrap();
        for bytes in [
            vec![],
            (1024 * 1024 + 1u64).to_le_bytes().to_vec(),
            20u64.to_le_bytes().to_vec(),
            [2u64.to_le_bytes().as_slice(), b"{}"].concat(),
        ] {
            std::fs::write(root.path().join("model.safetensors"), bytes).unwrap();
            assert!(Potion::describe(Some(root.path())).is_err());
        }
    }

    #[test]
    fn tokenizer_words_are_protected_from_typo_correction() {
        let potion = Potion::bundled().unwrap();
        let history = ["site", "install"].map(str::to_owned).into_iter().collect();
        let prepare =
            |query| crate::corpus::prepare_intent_query(query, &history, Some(&potion.vocabulary));
        assert!(potion.vocabulary.contains("suite"));
        assert!(!potion.vocabulary.contains("insall"));
        assert_eq!(
            prepare("run the project test suite").embedding_query(),
            "run the project test suite"
        );
        assert_eq!(
            prepare("build and insall locally").embedding_query(),
            "build and install locally"
        );
        assert_eq!(
            crate::corpus::prepare_intent_query("suite", &history, None).embedding_query(),
            "site"
        );
    }

    fn copy_model(destination: &Path) {
        let files = Potion::model_files(None).unwrap();
        for (source, name) in
            files
                .iter()
                .zip(["config.json", "tokenizer.json", "model.safetensors"])
        {
            std::fs::copy(source, destination.join(name)).unwrap();
        }
    }

    #[test]
    fn identity_changes_with_any_model_file() {
        let root = tempfile::tempdir().unwrap();
        copy_model(root.path());
        let original = Potion::describe(Some(root.path())).unwrap().identity;
        let mut identities = HashSet::from([original]);
        for file in ["config.json", "tokenizer.json", "model.safetensors"] {
            let path = root.path().join(file);
            let mut bytes = std::fs::read(&path).unwrap();
            let offset = bytes.len() - 1;
            bytes[offset] ^= 1;
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                identities.insert(Potion::describe(Some(root.path())).unwrap().identity),
                "{file}"
            );
            bytes[offset] ^= 1;
            std::fs::write(path, bytes).unwrap();
        }
        assert_eq!(identities.len(), 4);
    }

    #[test]
    fn identity_is_path_independent() {
        let root = tempfile::tempdir().unwrap();
        copy_model(root.path());
        assert_eq!(
            Potion::describe(None).unwrap(),
            Potion::describe(Some(root.path())).unwrap()
        );
        assert_eq!(
            Potion::bundled().unwrap().model_id(),
            Potion::from_dir(root.path()).unwrap().model_id()
        );
    }

    #[test]
    fn describe_keeps_header_and_hash_on_the_same_open_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.json"), "{}").unwrap();
        std::fs::write(root.path().join("tokenizer.json"), "{}").unwrap();
        let weights = |dimension| {
            let header =
                serde_json::to_vec(&serde_json::json!({"embeddings": {"shape": [1, dimension]}}))
                    .unwrap();
            [(header.len() as u64).to_le_bytes().as_slice(), &header].concat()
        };
        let path = root.path().join("model.safetensors");
        std::fs::write(&path, weights(2)).unwrap();
        let original = Potion::describe(Some(root.path())).unwrap();
        let files = Potion::model_files(Some(root.path()))
            .unwrap()
            .map(|path| std::fs::File::open(path).unwrap());
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, weights(3)).unwrap();
        std::fs::rename(replacement, path).unwrap();
        assert_eq!(Potion::describe_open_files(files).unwrap(), original);
        let replaced = Potion::describe(Some(root.path())).unwrap();
        assert_eq!(replaced.dimension, 3);
        assert_ne!(replaced.identity, original.identity);
    }

    #[test]
    fn identity_is_stable() {
        assert_eq!(
            Potion::describe(None).unwrap().identity,
            "b328dc29a5ea20ee6c"
        );
    }

    #[test]
    #[ignore = "manual release measurement of streaming model identity"]
    fn describe_timing() {
        let start = std::time::Instant::now();
        let model = Potion::describe(None).unwrap();
        eprintln!(
            "bundled describe: {:.3} ms ({})",
            start.elapsed().as_secs_f64() * 1000.0,
            model.identity
        );
    }
}
