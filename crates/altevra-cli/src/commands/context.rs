use altevra_vault::scan_vault;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct ContextArgs {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: ContextArgs) -> anyhow::Result<()> {
    let files = scan_vault(&args.vault)?;
    let project = args.project.clone();
    let project_label = project.as_deref().unwrap_or("(all)");

    let relevant: Vec<_> = files
        .into_iter()
        .filter(|f| {
            project
                .as_deref()
                .map(|p| f.path.to_string_lossy().contains(p))
                .unwrap_or(true)
        })
        .collect();

    let sections: std::collections::BTreeMap<String, usize> =
        relevant.iter().fold(Default::default(), |mut acc, f| {
            let s = f.section.clone().unwrap_or_else(|| "root".into());
            *acc.entry(s).or_insert(0) += 1;
            acc
        });

    if args.json {
        let out = serde_json::json!({
            "project": project_label,
            "vault_root": args.vault,
            "files": relevant.len(),
            "sections": sections,
            "recent_files": relevant.iter().take(10).map(|f| serde_json::json!({
                "path": f.path,
                "section": f.section,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Context for: {project_label}");
        println!("Vault: {}", args.vault.display());
        println!("Total files: {}", relevant.len());
        println!("\nSections:");
        for (section, count) in &sections {
            println!("  {section}: {count}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn empty_vault_runs() {
        let tmp = TempDir::new().unwrap();
        run(ContextArgs {
            project: None,
            vault: tmp.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();
    }
}
