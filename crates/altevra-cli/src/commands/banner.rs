use clap::Args;
use std::io::IsTerminal;

const ALTEVRA_RED: &str = "\x1b[1;38;5;160m";
const ALTEVRA_RED_DIM: &str = "\x1b[38;5;124m";
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";

const LOGO: &str = r#"    █████╗ ██╗  ████████╗███████╗██╗   ██╗██████╗  █████╗
   ██╔══██╗██║  ╚══██╔══╝██╔════╝██║   ██║██╔══██╗██╔══██╗
   ███████║██║     ██║   █████╗  ██║   ██║██████╔╝███████║
   ██╔══██║██║     ██║   ██╔══╝  ╚██╗ ██╔╝██╔══██╗██╔══██║
   ██║  ██║███████╗██║   ███████╗ ╚████╔╝ ██║  ██║██║  ██║
   ╚═╝  ╚═╝╚══════╝╚═╝   ╚══════╝  ╚═══╝  ╚═╝  ╚═╝╚═╝  ╚═╝"#;

#[derive(Args)]
pub struct BannerArgs {
    /// Disable ANSI colors (useful for piping or non-TTY output)
    #[arg(long)]
    pub plain: bool,

    /// Short one-line banner instead of full logo
    #[arg(long)]
    pub mini: bool,
}

pub async fn run(args: BannerArgs) -> anyhow::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let use_color = !args.plain && std::io::stdout().is_terminal();

    if args.mini {
        if use_color {
            println!(
                "{red}▌ ALTEVRA{reset} {bold}{version}{reset}  {dim}•  local-first AI memory{reset}",
                red = ALTEVRA_RED,
                bold = BOLD,
                dim = DIM,
                reset = RESET,
            );
        } else {
            println!("▌ ALTEVRA {version}  •  local-first AI memory");
        }
        return Ok(());
    }

    if use_color {
        println!("{ALTEVRA_RED}{LOGO}{RESET}");
        println!();
        println!(
            "   {bold}The omniscient brain layer for your AI tools.{reset}",
            bold = BOLD,
            reset = RESET,
        );
        println!(
            "   {dim}v{version}  •  local-first  •  source-available  •  built in Rust{reset}",
            dim = DIM,
            reset = RESET,
        );
        println!();
        println!(
            "   {red_dim}License:{reset}    PolyForm Strict 1.0.0  ({dim}see LICENSE{reset})",
            red_dim = ALTEVRA_RED_DIM,
            dim = DIM,
            reset = RESET,
        );
        println!(
            "   {red_dim}Commercial:{reset} ceoimperiumprojects@gmail.com",
            red_dim = ALTEVRA_RED_DIM,
            reset = RESET,
        );
        println!(
            "   {red_dim}Repo:{reset}       https://github.com/ceoimperiumprojects/Altevra",
            red_dim = ALTEVRA_RED_DIM,
            reset = RESET,
        );
    } else {
        println!("{LOGO}");
        println!();
        println!("   The omniscient brain layer for your AI tools.");
        println!("   v{version}  •  local-first  •  source-available  •  built in Rust");
        println!();
        println!("   License:    PolyForm Strict 1.0.0  (see LICENSE)");
        println!("   Commercial: ceoimperiumprojects@gmail.com");
        println!("   Repo:       https://github.com/ceoimperiumprojects/Altevra");
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn banner_plain_mini_runs() {
        let args = BannerArgs {
            plain: true,
            mini: true,
        };
        assert!(run(args).await.is_ok());
    }

    #[tokio::test]
    async fn banner_plain_full_runs() {
        let args = BannerArgs {
            plain: true,
            mini: false,
        };
        assert!(run(args).await.is_ok());
    }
}
