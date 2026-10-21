use anyhow::Result;
use smcli_core::{
    context::ProjectContext,
    service_client::ServiceClient,
    service_protocol::{Operation, ServiceRequest},
};
use std::time::Duration;

pub fn directory_context(directory: &str, service: &ServiceClient) -> ProjectContext {
    let mut request = ServiceRequest::new(Operation::Context);
    request.directory = Some(directory.into());
    service
        .send(&request, None, Duration::from_secs(10))
        .filter(|r| r.ok)
        .and_then(|r| r.context)
        .unwrap_or_else(|| {
            let mut context = ProjectContext::discover(directory);
            context.status = context
                .repository
                .as_ref()
                .and_then(|r| smcli_core::service_protocol::GitStatus::read(&r.root, 2.0));
            context
        })
}
pub fn run(directory: &str, json: bool, service: &ServiceClient) -> Result<()> {
    let c = directory_context(directory, service);
    if json {
        return crate::cli::print_json(&c);
    }
    println!("directory  {}", c.directory);
    if let Some(r) = &c.repository {
        println!(
            "repo       {}{}",
            r.root,
            if r.linked_worktree {
                format!(" (linked worktree of {})", r.common_dir)
            } else {
                String::new()
            }
        );
        println!(
            "branch     {}{}",
            r.branch
                .clone()
                .unwrap_or_else(|| format!("detached at {}", r.head.as_deref().unwrap_or("?"))),
            r.operation
                .as_ref()
                .map(|o| format!("  [{o} in progress]"))
                .unwrap_or_default()
        );
        if let Some(s) = &c.status {
            let mut parts = vec![];
            for (n, label) in [
                (s.staged, "staged"),
                (s.unstaged, "unstaged"),
                (s.untracked, "untracked"),
                (s.conflicted, "conflicted"),
            ] {
                if n > 0 {
                    parts.push(format!("{n} {label}"));
                }
            }
            if parts.is_empty() {
                parts.push("clean".into());
            }
            if s.ahead > 0 || s.behind > 0 {
                parts.push(format!("ahead {}, behind {}", s.ahead, s.behind));
            }
            println!(
                "status     {}{}",
                parts.join(", "),
                if c.status_pending {
                    " (refreshing)"
                } else {
                    ""
                }
            );
        } else {
            println!(
                "status     {}",
                if c.status_pending {
                    "refreshing"
                } else {
                    "unavailable"
                }
            );
        }
        println!("in repo    {}", c.repo_relative.as_deref().unwrap_or("."));
    } else {
        println!("repo       none");
    }
    for p in &c.projects {
        println!(
            "project    {} ({}{}){}",
            p.root,
            p.kind,
            p.package_manager
                .as_ref()
                .map(|m| format!(", {m}"))
                .unwrap_or_default(),
            p.name
                .as_ref()
                .map(|n| format!(" \"{n}\""))
                .unwrap_or_default()
        );
        for (file, names) in &p.scripts {
            println!(
                "           {file}: {}{}",
                names.iter().take(12).cloned().collect::<Vec<_>>().join(" "),
                if names.len() > 12 { " ..." } else { "" }
            );
        }
    }
    if let Some(relative) = c.project_relative {
        println!("in project {relative}");
    }
    Ok(())
}
