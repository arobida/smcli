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
                .any(|s| !["ctrl-r", "left", "right", "ctrl-t", "ctrl-g", "none"].contains(&s.as_str()))
                || (names.iter().any(|s| s == "none") && names.len() > 1)
            {
                bail!("--keys takes ctrl-r,left,right,ctrl-t,ctrl-g or none.");
            }
            config.ctrl_r_key = Some(names.iter().any(|s| s == "ctrl-r"));
            config.left_key = Some(names.iter().any(|s| s == "left"));
            config.right_key = Some(names.iter().any(|s| s == "right"));
            config.ctrl_t_key = Some(names.iter().any(|s| s == "ctrl-t"));
            config.ctrl_g_key = Some(names.iter().any(|s| s == "ctrl-g"));
        } else {
            if !io::stdin().is_terminal() {
                bail!("smcli init --keys needs a terminal; pass --keys ctrl-r,left,right,ctrl-t,ctrl-g.");
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
        let wanted = args.potion.clone().filter(|s| !s.is_empty());
        // The service holds the old model; stop it before loading the new one so they never share memory.
        if wanted != config.potion_path && crate::cli::stop_service(service) {
            println!("Stopped smcli-service to free the old model.");
        }
        let model = match args.potion.as_deref().filter(|s| !s.is_empty()) {
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
fn choose(c: &mut SmcliConfig) -> Result<()> {
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
    c.ctrl_r_key = Some(r);
    c.left_key = Some(l);
    c.right_key = Some(right);
    c.ctrl_t_key = Some(t);
    c.ctrl_g_key = Some(g);
    Ok(())
}
