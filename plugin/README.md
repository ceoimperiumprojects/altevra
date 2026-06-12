# Altevra Claude Plugin (marketplace-ready prep)

Prerequisite: the `altevra` binary on PATH (clone repo → `cargo build --release` →
symlink/copy to `~/.local/bin/altevra` → `altevra auth codex` → `altevra service install --apply`).

Install as plugin:
  claude plugin marketplace add ceoimperiumprojects/altevra
  claude plugin install altevra@altevra

NOTE: hooks are intentionally NOT shipped in this plugin — live capture is wired
globally by `altevra install-hooks` (shipping them here too would double-capture).
