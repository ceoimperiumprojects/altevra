//! `altevra huge-ask "<question>"` — the heavy artillery. Unlike `ask` (which
//! reads Altevra only), huge-ask fans out MULTIPLE Sonnet agents in parallel,
//! each with full shell, to search a different surface — the Altevra brain, the
//! whole computer (filesystem), and the internet — then a final OPUS pass
//! synthesizes everything into one definitive answer. Slow + powerful; use rarely.

use clap::Args;
use tokio::process::Command;

#[derive(Args)]
pub struct HugeAskArgs {
    /// The question to answer across brain + computer + web.
    pub question: String,
    /// Skip the web-search agent (brain + filesystem only).
    #[arg(long)]
    pub no_web: bool,
    /// Model alias for the parallel search agents.
    #[arg(long, default_value = "sonnet")]
    pub search_model: String,
    /// Model alias for the final synthesis.
    #[arg(long, default_value = "opus")]
    pub synth_model: String,
}

/// Run one `claude -p` search agent with full shell over a named surface.
async fn search_agent(model: &str, label: &str, prompt: String) -> (String, String) {
    let out = Command::new("claude")
        .args([
            "-p",
            &prompt,
            "--dangerously-skip-permissions",
            "--model",
            model,
            "--output-format",
            "text",
        ])
        .current_dir(altevra_core::home_dir())
        .output()
        .await;
    let body = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => format!("(agent error: {})", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => format!("(agent failed to launch: {e})"),
    };
    (label.to_string(), body)
}

pub async fn run(args: HugeAskArgs) -> anyhow::Result<()> {
    if args.question.trim().is_empty() {
        anyhow::bail!("huge-ask what? provide a question");
    }
    let q = &args.question;
    eprintln!("🛰️  huge-ask: fanning out parallel Sonnet searchers (brain · computer{}) → Opus synthesis. This is heavy; give it a minute…",
        if args.no_web { "" } else { " · web" });

    // --- Surface-scoped search prompts (each agent owns ONE surface) ---
    let brain = search_agent(
        &args.search_model,
        "ALTEVRA BRAIN",
        format!(
            "You are searching Pavle's Altevra second-brain for: \"{q}\". Use the `altevra` CLI \
             only — run `altevra ask \"...\"`, `altevra recall \"...\" --semantic`, and \
             `altevra recall \"...\" --window last_month` with several phrasings — to gather \
             everything relevant (decisions, turns, notes, files he's worked on). Report concise \
             findings with their sources. Do NOT synthesize beyond what the brain holds."
        ),
    );

    let fs = search_agent(
        &args.search_model,
        "COMPUTER (filesystem)",
        format!(
            "You are searching Pavle's WHOLE computer for: \"{q}\". Use rg/grep/find/ls across \
             ~/projekti, ~/Desktop, ~/Documents, ~/Obsidian, ~/.altevra and other relevant dirs. \
             Open the most promising files and read them. Report concise findings with EXACT \
             file paths. Stay inside $HOME; read-only — never modify or delete anything."
        ),
    );

    // Fan out. Web agent is optional.
    let (brain_r, fs_r, web_r) = if args.no_web {
        let (b, f) = tokio::join!(brain, fs);
        (b, f, None)
    } else {
        let web = search_agent(
            &args.search_model,
            "WEB",
            format!(
                "You are researching the INTERNET for: \"{q}\". Use web search / fetch to find \
                 current, credible external information that helps answer it. Report concise \
                 findings WITH URLs. If the question is purely about Pavle's own data and the \
                 web adds nothing, say so briefly."
            ),
        );
        let (b, f, w) = tokio::join!(brain, fs, web);
        (b, f, Some(w))
    };

    // --- Assemble the dossier ---
    let mut dossier = format!(
        "## {} findings\n{}\n\n## {} findings\n{}\n",
        brain_r.0, brain_r.1, fs_r.0, fs_r.1
    );
    if let Some(w) = &web_r {
        dossier.push_str(&format!("\n## {} findings\n{}\n", w.0, w.1));
    }

    // --- OPUS synthesis ---
    eprintln!("🧠  synthesizing with {}…", args.synth_model);
    let synth_prompt = format!(
        "You are Pavle's chief analyst. Three agents searched his Altevra second-brain, his \
         computer, and the web to answer:\n\n\"{q}\"\n\nHere is their raw dossier:\n\n{dossier}\n\n\
         Synthesize ONE definitive, well-structured answer. Resolve conflicts between sources, \
         attribute key facts to where they came from (brain / file path / web URL), and flag any \
         gaps or uncertainty honestly. Match Pavle's language. Be thorough but tight."
    );
    let out = Command::new("claude")
        .args([
            "-p",
            &synth_prompt,
            "--model",
            &args.synth_model,
            "--output-format",
            "text",
        ])
        .current_dir(altevra_core::home_dir())
        .output()
        .await;

    match out {
        Ok(o) if o.status.success() => println!("{}", String::from_utf8_lossy(&o.stdout).trim()),
        Ok(o) => {
            // Synthesis failed — still hand back the raw dossier so nothing is lost.
            eprintln!("(synthesis failed: {})", String::from_utf8_lossy(&o.stderr).trim());
            println!("Synthesis unavailable — raw findings:\n\n{dossier}");
        }
        Err(e) => {
            eprintln!("(synthesis launch failed: {e})");
            println!("Raw findings:\n\n{dossier}");
        }
    }
    Ok(())
}
