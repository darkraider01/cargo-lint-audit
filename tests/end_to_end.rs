use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "audit-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        copy_dir(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace"),
            &root,
        );
        Self(root)
    }

    fn audit(&self, args: &[&str]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-lint-audit"))
            .arg("lint-audit")
            .args(["--json", "--offline"])
            .args(args)
            .current_dir(&self.0)
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let dest = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn finding<'a>(report: &'a Value, source_fragment: &str, lint: &str) -> &'a Value {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| {
            finding["source"]
                .as_str()
                .unwrap()
                .contains(source_fragment)
                && finding["lint"] == lint
        })
        .unwrap()
}

fn observed_function(report: &Value, name: &str) -> bool {
    report["newly_observed_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["message"].as_str().unwrap().contains(name))
}

#[test]
fn audits_actual_targets_and_keeps_attribution_conservative() {
    let fixture = Fixture::new();
    let report = fixture.audit(&["--workspace", "--all-targets"]);
    assert!(observed_function(&report, "active_allow"));
    assert!(!observed_function(&report, "used_allow"));
    assert!(observed_function(&report, "expected_unused"));
    assert_eq!(
        finding(&report, "macro scope", "dead_code")["status"],
        "UNKNOWN"
    );
    assert!(observed_function(&report, "external_unused"));
    assert!(observed_function(&report, "build_unused"));
    assert!(observed_function(&report, "integration_unused"));
    assert!(observed_function(&report, "example_unused"));
    assert!(observed_function(&report, "bench_unused"));
    assert!(observed_function(&report, "beta_unused"));
    assert!(!observed_function(&report, "dependency_unused"));
    let findings = report["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f["source"] == "#[allow(unused)]"
        && f["status"] == "DIAGNOSTIC_OBSERVED"
        && f["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["lint"] == "unused_mut")));
    assert!(findings
        .iter()
        .any(|f| f["source"] == "#![allow(dead_code)]" && f["status"] == "DIAGNOSTIC_OBSERVED"));
    assert!(findings
        .iter()
        .any(|f| f["source"] == "#[allow(dead_code)]" && f["status"] == "NO_DIAGNOSTIC_OBSERVED"));
    assert!(findings
        .iter()
        .any(|f| f["lint"] == "trivial_casts" && f["status"] == "DIAGNOSTIC_OBSERVED"));
    assert_eq!(
        report["newly_observed_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["lint"] == "trivial_casts")
            .count(),
        4
    );
    assert_eq!(finding(&report, "compat", "dead_code")["reason"], "compat");
    assert_eq!(
        finding(&report, "expected compatibility stub", "dead_code")["kind"],
        "expect"
    );
    assert_eq!(
        finding(&report, "feature-dependent", "dead_code")["status"],
        "UNKNOWN"
    );
    assert_eq!(
        finding(&report, "lint_audit_nonexistent", "lint_audit_nonexistent")["status"],
        "UNKNOWN"
    );
    assert_eq!(
        finding(&report, "private_in_public", "private_in_public")["status"],
        "UNKNOWN"
    );
    assert_eq!(
        finding(
            &report,
            "unused_tuple_struct_fields",
            "unused_tuple_struct_fields"
        )["status"],
        "UNKNOWN"
    );
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["attribution"] == "unproven"));
    let expectations = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "expect" && f["lint"] == "dead_code")
        .collect::<Vec<_>>();
    assert!(expectations
        .iter()
        .any(|f| f["status"] == "DIAGNOSTIC_OBSERVED"
            && f["baseline_unfulfilled_expectations"]
                .as_array()
                .unwrap()
                .is_empty()));
    assert!(expectations
        .iter()
        .any(|f| f["status"] == "NO_DIAGNOSTIC_OBSERVED"
            && !f["baseline_unfulfilled_expectations"]
                .as_array()
                .unwrap()
                .is_empty()));
    let repeated = fixture.audit(&["--workspace", "--all-targets"]);
    assert_eq!(report["findings"], repeated["findings"]);
    assert_eq!(
        report["newly_observed_diagnostics"],
        repeated["newly_observed_diagnostics"]
    );
    assert_eq!(repeated["forced"]["compiled_selected_units"], 0);
    assert!(repeated["forced"]["fresh_selected_units"].as_u64().unwrap() > 0);
    let source_path = fixture.0.join("alpha/src/main.rs");
    let source = fs::read_to_string(&source_path).unwrap();
    fs::write(
        &source_path,
        source.replace("    used_allow();", "    active_allow(); used_allow();"),
    )
    .unwrap();
    let changed = fixture.audit(&["--workspace", "--all-targets"]);
    assert!(!observed_function(&changed, "active_allow"));
    assert!(
        changed["forced"]["compiled_selected_units"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn default_members_and_unbuilt_targets_are_distinguished() {
    let fixture = Fixture::new();
    let report = fixture.audit(&[]);
    assert_eq!(report["selected_packages"], serde_json::json!(["alpha"]));
    assert!(!observed_function(&report, "beta_unused"));
    assert_eq!(
        finding(&report, "integration target", "dead_code")["status"],
        "UNKNOWN"
    );
    assert_eq!(
        finding(&report, "example target", "dead_code")["status"],
        "UNKNOWN"
    );
    assert_eq!(
        finding(&report, "bench target", "dead_code")["status"],
        "UNKNOWN"
    );
    let feature = fixture.audit(&["--package", "alpha", "--features", "extra"]);
    assert!(observed_function(&feature, "conditional_allow"));
    assert!(observed_function(&feature, "feature_only"));
}

#[test]
fn clippy_and_rustc_modes_use_different_lint_catalogs() {
    let fixture = Fixture::new();
    let rustc = fixture.audit(&[]);
    assert_eq!(
        finding(
            &rustc,
            "#[allow(clippy::needless_return)]",
            "clippy::needless_return"
        )["status"],
        "UNKNOWN"
    );
    let clippy = fixture.audit(&["--clippy", "--workspace"]);
    let allowed = finding(
        &clippy,
        "#[allow(clippy::needless_return)]",
        "clippy::needless_return",
    );
    assert_eq!(allowed["status"], "DIAGNOSTIC_OBSERVED");
    assert!(!allowed["diagnostics"].as_array().unwrap().is_empty());
    let expected = finding(
        &clippy,
        "#[expect(clippy::needless_return)]",
        "clippy::needless_return",
    );
    assert_eq!(expected["status"], "DIAGNOSTIC_OBSERVED");
    assert!(expected["baseline_unfulfilled_expectations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!observed_function(&clippy, "dependency_unused"));
}

#[test]
fn failures_never_produce_an_absence_claim() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("alpha/src/main.rs"),
        "#[allow(dead_code)] fn unused() {} fn main() { missing(); }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-lint-audit"))
        .args(["--json", "--offline"])
        .current_dir(&fixture.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["status"] == "UNKNOWN"));
    assert!(!report["baseline"]["success"].as_bool().unwrap());
}
