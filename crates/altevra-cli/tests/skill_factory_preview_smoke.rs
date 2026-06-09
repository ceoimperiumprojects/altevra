//! P3a CLI smoke (PLAN-ALIVE §P3 gate): `altevra skill-factory edits-preview`
//! on a temp skill file — applies in memory, prints the outcome, and NEVER
//! writes the file. No DB, no network, no real ~/.altevra.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn write_fixture(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let skill = dir.path().join("SKILL.md");
    fs::write(
        &skill,
        "---\nslug: demo-skill\nversion: 1.0.0\ntitle: Demo\n---\n\n# Demo\n\n## Usage\nrun it\n\n<!-- ALTEVRA_SLOW_UPDATE_START -->\nprotected longitudinal rule\n<!-- ALTEVRA_SLOW_UPDATE_END -->\n",
    )
    .unwrap();
    let edits = dir.path().join("edits.json");
    fs::write(
        &edits,
        r###"[
            {"op":"replace","from":"run it","to":"run it CAREFULLY"},
            {"op":"delete","text":"protected longitudinal rule"},
            {"op":"insert_after","anchor":"## Missing Section","text":"x"}
        ]"###,
    )
    .unwrap();
    (skill, edits)
}

#[test]
fn edits_preview_smoke_never_writes() {
    let dir = tempfile::TempDir::new().unwrap();
    let (skill, edits) = write_fixture(&dir);
    let before = fs::read_to_string(&skill).unwrap();

    let mut cmd = Command::cargo_bin("altevra").unwrap();
    cmd.args([
        "skill-factory",
        "edits-preview",
        "--skill",
        skill.to_str().unwrap(),
        "--edits",
        edits.to_str().unwrap(),
        "--budget",
        "3",
    ])
    .assert()
    .success()
    // The good edit applied, the protected + hallucinated edits skipped.
    .stdout(predicate::str::contains("1 applied, 2 skipped"))
    .stdout(predicate::str::contains("run it CAREFULLY"))
    .stdout(predicate::str::contains(
        "target inside protected slow-update region",
    ))
    .stdout(predicate::str::contains("anchor not found"))
    .stdout(predicate::str::contains("nothing was written"));

    // Preview is read-only: the file on disk is byte-identical.
    let after = fs::read_to_string(&skill).unwrap();
    assert_eq!(before, after, "edits-preview must never write");
}

#[test]
fn edits_preview_json_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let (skill, edits) = write_fixture(&dir);

    let output = Command::cargo_bin("altevra")
        .unwrap()
        .args([
            "skill-factory",
            "edits-preview",
            "--skill",
            skill.to_str().unwrap(),
            "--edits",
            edits.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["outcome"]["applied"].as_array().unwrap().len(), 1);
    assert_eq!(doc["outcome"]["skipped"].as_array().unwrap().len(), 2);
    assert_eq!(doc["outcome"]["changed"], true);
    assert_eq!(doc["fingerprint"].as_str().unwrap().len(), 64);
}
