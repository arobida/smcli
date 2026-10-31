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
pub fn run(args: Init, path: &Path, service: &ServiceClient) -> Result<()> {
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
                .any(|s| !["up", "ctrl-r", "left", "right", "ctrl-t", "ctrl-g", "none"].contains(&s.as_str()))
                || (names.iter().any(|s| s == "none") && names.len() > 1)
            {
                bail!("--keys takes up,ctrl-r,left,right,ctrl-t,ctrl-g or none.");
            }
            config.up_key = Some(names.iter().any(|s| s == "up"));
            config.ctrl_r_key = Some(names.iter().any(|s| s == "ctrl-r"));
            config.left_key = Some(names.iter().any(|s| s == "left"));
            config.right_key = Some(names.iter().any(|s| s == "right"));
            config.ctrl_t_key = Some(names.iter().any(|s| s == "ctrl-t"));
            config.ctrl_g_key = Some(names.iter().any(|s| s == "ctrl-g"));
        } else {
            if !io::stdin().is_terminal() {
                bail!("smcli init --keys needs a terminal; pass --keys up,ctrl-r,left,right,ctrl-t,ctrl-g.");
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
        // The service holds the old model; stop it before loading the new one so they never share memory.
        if wanted != config.potion_path && crate::cli::stop_service(service) {
            println!("Stopped smcli-service to free the old model.");
        }
        let model = match wanted.as_deref() {
            Some(p) => smcli_search::Potion::from_dir(Path::new(p))?,
            None => smcli_search::Potion::bundled()?,
        };
        anyhow::ensure!(
            !model.embed(&["smcli".into()])[0].is_empty(),
            "Potion returned an empty embedding"
        );
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
            model.dimension(),
            path.display()
        );
    }
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
    c.up_key = Some(up);
    c.ctrl_r_key = Some(r);
    c.left_key = Some(l);
    c.right_key = Some(right);
    c.ctrl_t_key = Some(t);
    c.ctrl_g_key = Some(g);
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
        run(init(Some(&abs)), &abs_cfg, &service).unwrap();
        run(init(None), &abs_cfg, &service).unwrap();
        assert_eq!(SmcliConfig::load(&abs_cfg).unwrap().potion_path.as_deref(), Some(abs.as_str()));

        let rel = relative_from_cwd(&model);
        let rel_cfg = home.join("rel.json");
        run(init(Some(&rel)), &rel_cfg, &service).unwrap();
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
