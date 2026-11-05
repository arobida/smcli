use crate::cli::Init;
use anyhow::{Result, bail};
use smcli_core::{config::SmcliConfig, service_client::ServiceClient};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

fn ask(question: &str, default: bool) -> Result<bool> {
    loop {
        print!("{question} [{}]: ", if default { "Y/n" } else { "y/N" });
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            bail!("No choice made; key bindings unchanged.");
        }
        match line.trim().to_lowercase().as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => {}
        }
    }
}
pub fn run(args: Init, path: &Path, service: &ServiceClient, captured: &smcli_core::history::CapturedHistory) -> Result<()> {
    let mut config = SmcliConfig::load(path)?;
    if let Some(keys) = &args.keys {
        if !keys.is_empty() {
            let names: Vec<_> = keys
                .split(',')
                .map(str::trim)
                .map(str::to_lowercase)
                .collect();
            if names
                .iter()
                .any(|s| !["up", "ctrl-r", "left", "right", "ctrl-t", "ctrl-g", "ctrl-h", "none"].contains(&s.as_str()))
                || (names.iter().any(|s| s == "none") && names.len() > 1)
            {
                bail!("--keys takes up,ctrl-r,left,right,ctrl-t,ctrl-g,ctrl-h or none.");
            }
            config.up_key = Some(names.iter().any(|s| s == "up"));
            config.ctrl_r_key = Some(names.iter().any(|s| s == "ctrl-r"));
            config.left_key = Some(names.iter().any(|s| s == "left"));
            config.right_key = Some(names.iter().any(|s| s == "right"));
            config.ctrl_t_key = Some(names.iter().any(|s| s == "ctrl-t"));
            config.ctrl_g_key = Some(names.iter().any(|s| s == "ctrl-g"));
            config.ctrl_h_key = Some(names.iter().any(|s| s == "ctrl-h"));
        } else {
            if !io::stdin().is_terminal() {
                bail!("smcli init --keys needs a terminal; pass --keys up,ctrl-r,left,right,ctrl-t,ctrl-g,ctrl-h.");
            }
            choose(&mut config)?;
        }
        config.save(path)?;
        println!("Saved key bindings to {}.", path.display());
    } else if !args.has_model() && io::stdin().is_terminal() {
        choose(&mut config)?;
        config.save(path)?;
    }
    if args.model.is_some() || args.builtin {
        bail!("Rust supports Potion only; use --default or --potion [DIR].");
    }
    if args.has_model() || args.keys.is_none() {
        let wanted = if args.has_model() {
            args.potion.as_deref().filter(|s| !s.is_empty()).map(persisted_potion).transpose()?
        } else {
            config.potion_path.clone()
        };
        let model = if wanted == config.potion_path {
            smcli_search::Potion::describe(wanted.as_deref().map(Path::new))?
        } else {
            if crate::cli::stop_service(service) {
                println!("Stopped smcli-service to free the old model.");
            }
            let _lock = model_lock(captured)?;
            let potion = match wanted.as_deref() {
                Some(p) => smcli_search::Potion::from_dir(Path::new(p))?,
                None => smcli_search::Potion::bundled()?,
            };
            anyhow::ensure!(!potion.embed(&["smcli".into()])[0].is_empty(), "Potion returned an empty embedding");
            potion.model_id()
        };
        if wanted != config.potion_path {
            println!("Changing the model rebuilds the search index with it on the next search; that search can take a while on a large history.");
            println!("The old model's index stays on disk; `smcli indexes --prune` deletes it.");
        }
        config.potion_path = wanted;
        config.ollama_model = None;
        config.ollama_dimension = None;
        config.builtin = None;
        config.save(path)?;
        println!(
            "Search will use {} ({} dimensions). Saved {}.",
            smcli_search::Potion::NAME,
            model.dimension,
            path.display()
        );
    }
    Ok(())
}
fn model_lock(captured: &smcli_core::history::CapturedHistory) -> Result<std::fs::File> {
    smcli_search::model_lock(captured, std::time::Duration::from_secs(5))?
        .ok_or_else(|| anyhow::anyhow!("smcli-service still holds the model; run `smcli service stop` and retry."))
}

pub fn reindex(captured: &smcli_core::history::CapturedHistory, home: &Path,
               env: &std::collections::HashMap<String, String>, service: &ServiceClient) -> Result<()> {
    let stopped = crate::cli::stop_service(service);
    let _model_lock = model_lock(captured)?;
    let engine = smcli_search::setup::search_engine(captured, home, env)?;
    let lock = engine.lock_index(u32::MAX)?.ok_or_else(|| anyhow::anyhow!("Another smcli is updating the index."))?;
    let entries = smcli_search::setup::corpus(captured, home, env, None, None, smcli_core::time::now())?;
    let count = engine.reindex_using_lock(&entries, &lock)?;
    drop(lock); // CommandIndex takes the same directory lock.
    let sources = smcli_search::commands::bundled_sources()
        .ok_or_else(|| anyhow::anyhow!("tldr data not found beside the smcli executable or in Data/tldr"))?;
    let refs = smcli_search::commands::source_refs(&sources);
    let index = smcli_search::commands::CommandIndex::new(engine.directory.clone(), engine.potion.clone(), &refs);
    let path = smcli_core::service_protocol::shell_path_snapshot(env.get("PATH").map(String::as_str)).unwrap_or_default();
    smcli_search::commands::prepare(&index, &refs, smcli_search::tldr::current_platform(), &path,
        &crate::cli::captured_programs(captured)?, true, None)?;
    let programs = index.programs()?.len();
    println!("{}Reindexed {count} commands and {programs} programs' examples.",
        if stopped { "Stopped smcli-service to free its model. " } else { "" });
    Ok(())
}

/// A local directory is stored canonical, so a later command from another cwd finds it. An id is not a directory and is kept as given.
fn persisted_potion(value: &str) -> std::io::Result<String> {
    if Path::new(value).is_dir() {
        std::fs::canonicalize(value).map(|path| path.to_string_lossy().into_owned())
    } else {
        Ok(value.to_owned())
    }
}

fn choose(c: &mut SmcliConfig) -> Result<()> {
    let up = ask("Open history with Up?", c.up_key.unwrap_or(true))?;
    let r = ask("Open history with Ctrl-R?", c.ctrl_r_key.unwrap_or(true))?;
    let l = ask(
        "Open history with Left on an empty line?",
        c.left_key.unwrap_or(false),
    )?;
    let right = ask(
        "Open suggestions with Right on an empty line?",
        c.right_key.unwrap_or(false),
    )?;
    let t = ask("Open files with Ctrl-T?", c.ctrl_t_key.unwrap_or(true))?;
    let g = ask("Open grep with Ctrl-G?", c.ctrl_g_key.unwrap_or(true))?;
    let h = ask("Open command examples with Ctrl-H?", c.ctrl_h_key.unwrap_or(true))?;
    c.up_key = Some(up);
    c.ctrl_r_key = Some(r);
    c.left_key = Some(l);
    c.right_key = Some(right);
    c.ctrl_t_key = Some(t);
    c.ctrl_g_key = Some(g);
    c.ctrl_h_key = Some(h);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Init, persisted_potion, run};
    use smcli_core::config::SmcliConfig;
    use smcli_core::history::CapturedHistory;
    use smcli_core::service_client::ServiceClient;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    #[test]
    fn init_unchanged_model_does_not_load() -> anyhow::Result<()> {
        let root = Path::new("/tmp").join(format!("smcli-init-header-{}", uuid::Uuid::new_v4()));
        let _cleanup = Tmp(root.clone());
        let model = root.join("model");
        std::fs::create_dir_all(&model)?;
        let model = std::fs::canonicalize(model)?;
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Models/potion-shell-16m");
        for file in ["config.json", "tokenizer.json"] { std::fs::copy(source.join(file), model.join(file))?; }
        let bytes = std::fs::read(source.join("model.safetensors"))?;
        let length = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
        std::fs::write(model.join("model.safetensors"), &bytes[..8 + length])?;
        assert!(smcli_search::Potion::from_dir(&model).is_err());
        let config_path = root.join("config.json");
        SmcliConfig { potion_path: Some(model.to_string_lossy().into_owned()), ..Default::default() }.save(&config_path)?;
        let captured = CapturedHistory::new(root.join("history.db"));
        let env = HashMap::from([("SMCLI_SERVICE".into(), "0".into())]);
        let service = ServiceClient::new(&captured.path, &env, Path::new("/missing/smcli"));
        // Holding the lock also proves an unchanged model never tries to acquire it.
        let _lock = smcli_search::model_lock(&captured, std::time::Duration::ZERO)?.unwrap();
        run(init(Some(model.to_str().unwrap())), &config_path, &service, &captured)?;
        Ok(())
    }

    #[test]
    fn reindex_refuses_while_model_lock_held() -> anyhow::Result<()> {
        let root = Path::new("/tmp").join(format!("smcli-init-lock-{}", uuid::Uuid::new_v4()));
        let _cleanup = Tmp(root.clone());
        std::fs::create_dir_all(&root)?;
        let captured = CapturedHistory::new(root.join("history.db"));
        let env = HashMap::from([("SMCLI_SERVICE".into(), "0".into())]);
        let service = ServiceClient::new(&captured.path, &env, Path::new("/missing/smcli"));
        let _lock = smcli_search::model_lock(&captured, std::time::Duration::ZERO)?.unwrap();
        let error = super::reindex(&captured, &root, &env, &service).unwrap_err();
        assert_eq!(error.to_string(), "smcli-service still holds the model; run `smcli service stop` and retry.");
        Ok(())
    }

    #[test]
    fn init_keeps_custom_model_and_stores_absolute_or_id() {
        let root = std::env::temp_dir().join(format!("smcli-init-{}", uuid::Uuid::new_v4()));
        let _cleanup = Tmp(root.clone());
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let db = root.join("history.db");
        let env = HashMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("SMCLI_DB".into(), db.to_string_lossy().into_owned()),
            ("SMCLI_RUNTIME_DIR".into(), root.join("runtime").to_string_lossy().into_owned()),
        ]);
        let database = CapturedHistory::location(&home, &env);
        assert_eq!(database, db);
        let service = ServiceClient::new(&database, &env, Path::new("/missing/smcli"));
        let model = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Models/potion-shell-16m")).unwrap();
        let abs = model.to_string_lossy().into_owned();

        let abs_cfg = home.join("abs.json");
        run(init(Some(&abs)), &abs_cfg, &service, &CapturedHistory::new(&database)).unwrap();
        // Re-init without a model flag must keep the custom path; pass --keys so a TTY cannot enter the questionnaire.
        run(
            Init {
                keys: Some("up,ctrl-r,ctrl-t,ctrl-g,ctrl-h".into()),
                ..init(None)
            },
            &abs_cfg,
            &service,
            &CapturedHistory::new(&database),
        )
        .unwrap();
        assert_eq!(SmcliConfig::load(&abs_cfg).unwrap().potion_path.as_deref(), Some(abs.as_str()));

        let rel = relative_from_cwd(&model);
        let rel_cfg = home.join("rel.json");
        run(init(Some(&rel)), &rel_cfg, &service, &CapturedHistory::new(&database)).unwrap();
        let saved = SmcliConfig::load(&rel_cfg).unwrap().potion_path.unwrap();
        assert_eq!(saved, std::fs::canonicalize(&rel).unwrap().to_string_lossy());
        assert_ne!(saved, rel);
        assert!(Path::new(&saved).is_absolute());

        let id = "minishlab/potion-code-16M-v2";
        assert!(!Path::new(id).is_dir());
        assert_eq!(persisted_potion(id).unwrap(), id);
    }

    fn init(potion: Option<&str>) -> Init {
        Init { model: None, potion: potion.map(str::to_owned), builtin: false, default: false, keys: None }
    }

    fn relative_from_cwd(target: &Path) -> String {
        let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let mut ups = 0;
        let mut base = cwd.as_path();
        while !target.starts_with(base) {
            base = base.parent().expect("model and cwd share a prefix");
            ups += 1;
        }
        let mut rel = PathBuf::new();
        for _ in 0..ups { rel.push(".."); }
        rel.push(target.strip_prefix(base).unwrap());
        let rel = rel.to_string_lossy().into_owned();
        assert!(!Path::new(&rel).is_absolute() && Path::new(&rel).is_dir(), "{rel}");
        rel
    }

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
}
