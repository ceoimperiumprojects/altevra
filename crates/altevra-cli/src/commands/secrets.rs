use altevra_secrets::SecretStore;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store a secret (prompts for value)
    Set(SecretsSetArgs),
    /// Retrieve a secret
    Get(SecretsGetArgs),
    /// List stored secret keys
    List(SecretsListArgs),
    /// Delete a secret
    Delete(SecretsDeleteArgs),
}

#[derive(Args)]
pub struct SecretsSetArgs {
    /// Key name
    pub key: String,
    /// Value (if omitted, prompt securely)
    #[arg(long)]
    pub value: Option<String>,
    /// Use encrypted file backend at this path (default: OS keyring)
    #[arg(long)]
    pub file: Option<PathBuf>,
    /// Env var holding encryption passphrase (only with --file)
    #[arg(long, default_value = "ALTEVRA_SECRETS_KEY")]
    pub key_env: String,
}

#[derive(Args)]
pub struct SecretsGetArgs {
    pub key: String,
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[arg(long, default_value = "ALTEVRA_SECRETS_KEY")]
    pub key_env: String,
}

#[derive(Args)]
pub struct SecretsListArgs {
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[arg(long, default_value = "ALTEVRA_SECRETS_KEY")]
    pub key_env: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SecretsDeleteArgs {
    pub key: String,
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[arg(long, default_value = "ALTEVRA_SECRETS_KEY")]
    pub key_env: String,
}

pub async fn run(cmd: SecretsCommands) -> anyhow::Result<()> {
    match cmd {
        SecretsCommands::Set(args) => run_set(args),
        SecretsCommands::Get(args) => run_get(args),
        SecretsCommands::List(args) => run_list(args),
        SecretsCommands::Delete(args) => run_delete(args),
    }
}

fn make_store(file: Option<PathBuf>, key_env: &str) -> SecretStore {
    match file {
        Some(p) => SecretStore::new_encrypted_file("altevra", p, key_env),
        None => SecretStore::new_keyring("altevra"),
    }
}

fn run_set(args: SecretsSetArgs) -> anyhow::Result<()> {
    let value = match args.value {
        Some(v) => v,
        None => rpassword::prompt_password(format!("Enter value for {}: ", args.key))?,
    };
    let store = make_store(args.file, &args.key_env);
    store.set(&args.key, &value)?;
    println!("Stored: {}", args.key);
    Ok(())
}

fn run_get(args: SecretsGetArgs) -> anyhow::Result<()> {
    let store = make_store(args.file, &args.key_env);
    match store.get(&args.key)? {
        Some(value) => {
            println!("{value}");
            Ok(())
        }
        None => anyhow::bail!("Secret not found: {}", args.key),
    }
}

fn run_list(args: SecretsListArgs) -> anyhow::Result<()> {
    let store = make_store(args.file, &args.key_env);
    let keys = store.list_keys()?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"keys": keys, "count": keys.len()}))?
        );
    } else if keys.is_empty() {
        println!("No secrets stored.");
    } else {
        println!("Stored secrets ({}):", keys.len());
        for k in &keys {
            println!("  {k}");
        }
    }
    Ok(())
}

fn run_delete(args: SecretsDeleteArgs) -> anyhow::Result<()> {
    let store = make_store(args.file, &args.key_env);
    store.delete(&args.key)?;
    println!("Deleted: {}", args.key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn set_get_list_delete_with_encrypted_file() {
        let tmp = TempDir::new().unwrap();
        let secrets_path = tmp.path().join("secrets.enc");
        std::env::set_var("ALTEVRA_TEST_KEY_ENV", "testpassphrase");

        let store = SecretStore::new_encrypted_file(
            "altevra-test",
            secrets_path.clone(),
            "ALTEVRA_TEST_KEY_ENV",
        );
        store.set("API_KEY", "secret-value-123").unwrap();
        assert_eq!(
            store.get("API_KEY").unwrap().as_deref(),
            Some("secret-value-123")
        );
        let keys = store.list_keys().unwrap();
        assert!(keys.contains(&"API_KEY".to_string()));
        store.delete("API_KEY").unwrap();
        assert!(store.get("API_KEY").unwrap().is_none());
    }
}
