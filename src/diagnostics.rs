use crate::{cargo, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct Span {
    pub file: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub line: u64,
    pub column: u64,
    pub expansion: bool,
    pub file_resolved: bool,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct Diagnostic {
    pub package_id: String,
    pub target: String,
    pub target_root: String,
    pub target_kind: Vec<String>,
    pub lint: String,
    pub message: String,
    pub primary_spans: Vec<Span>,
}

#[derive(Default)]
pub struct Messages {
    pub diagnostics: Vec<Diagnostic>,
    pub roots: BTreeSet<(String, String)>,
    pub fresh: usize,
    pub compiled: usize,
    pub all_fresh: usize,
    pub all_compiled: usize,
    pub errors: Vec<Value>,
    pub finished: bool,
}

pub fn parse(
    stdout: &[u8],
    selected: &BTreeSet<String>,
    workspace: &std::path::Path,
) -> Result<Messages> {
    let mut output = Messages::default();
    for line in String::from_utf8_lossy(stdout).lines() {
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let reason = value["reason"].as_str().unwrap_or("");
        if reason == "build-finished" {
            output.finished = value["success"].as_bool().unwrap_or(false);
        }
        if reason == "compiler-artifact" {
            if value["fresh"].as_bool() == Some(true) {
                output.all_fresh += 1;
            } else {
                output.all_compiled += 1;
            }
        }
        let package = value["package_id"].as_str().unwrap_or("");
        if !selected.contains(package) {
            continue;
        }
        if reason == "compiler-artifact" {
            output.roots.insert((
                package.into(),
                value["target"]["src_path"].as_str().unwrap_or("").into(),
            ));
            if value["fresh"].as_bool() == Some(true) {
                output.fresh += 1;
            } else {
                output.compiled += 1;
            }
        }
        if reason != "compiler-message" {
            continue;
        }
        let message = &value["message"];
        if message["level"] == "error" {
            output.errors.push(value.clone());
        }
        let Some(lint) = message["code"]["code"].as_str() else {
            continue;
        };
        let mut spans = Vec::new();
        for span in message["spans"].as_array().into_iter().flatten() {
            if span["is_primary"] != true {
                continue;
            }
            let path = PathBuf::from(span["file_name"].as_str().unwrap_or(""));
            // Cargo reports member source paths relative to the workspace root.
            let absolute = if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            };
            let resolved = cargo::canonical(&absolute);
            let file_resolved = resolved.is_ok();
            let file = resolved.unwrap_or(absolute).to_string_lossy().into_owned();
            spans.push(Span {
                file,
                byte_start: span["byte_start"].as_u64().unwrap_or(0),
                byte_end: span["byte_end"].as_u64().unwrap_or(0),
                line: span["line_start"].as_u64().unwrap_or(0),
                column: span["column_start"].as_u64().unwrap_or(0),
                expansion: !span["expansion"].is_null(),
                file_resolved,
            });
        }
        spans.sort();
        output.diagnostics.push(Diagnostic {
            package_id: package.into(),
            target: value["target"]["name"].as_str().unwrap_or("").into(),
            target_root: value["target"]["src_path"].as_str().unwrap_or("").into(),
            target_kind: serde_json::from_value(value["target"]["kind"].clone())?,
            lint: normalize(lint),
            message: message["message"].as_str().unwrap_or("").into(),
            primary_spans: spans,
        });
    }
    Ok(output)
}

pub fn difference(baseline: &[Diagnostic], forced: &[Diagnostic]) -> Vec<Diagnostic> {
    let mut counts = BTreeMap::new();
    for diagnostic in baseline {
        *counts.entry(diagnostic).or_insert(0usize) += 1;
    }
    let mut new = Vec::new();
    for diagnostic in forced {
        let count = counts.entry(diagnostic).or_default();
        if *count > 0 {
            *count -= 1;
        } else {
            new.push(diagnostic.clone());
        }
    }
    new.sort();
    new
}

pub fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

#[derive(Default)]
pub struct Catalog {
    pub lints: BTreeSet<String>,
    pub groups: BTreeMap<String, BTreeSet<String>>,
}

impl Catalog {
    pub fn parse(help: &str) -> Self {
        let mut catalog = Self::default();
        for line in help.lines() {
            let mut words = line.split_whitespace();
            let Some(name) = words.next() else {
                continue;
            };
            let Some(second) = words.next() else {
                continue;
            };
            let name = normalize(name);
            if matches!(second, "allow" | "warn" | "deny" | "forbid") {
                catalog.lints.insert(name);
            } else if second.ends_with(',') || catalog.lints.contains(&normalize(second)) {
                let members = std::iter::once(second)
                    .chain(words)
                    .map(|name| normalize(name.trim_end_matches(',')))
                    .collect();
                catalog.groups.insert(name, members);
            }
        }
        catalog
    }

    pub fn members(&self, name: &str) -> Option<BTreeSet<String>> {
        if name == "warnings" {
            return None;
        }
        if self.lints.contains(name) {
            Some(BTreeSet::from([name.into()]))
        } else {
            self.groups.get(name).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_distinguishes_groups_and_special_warnings() {
        let catalog = Catalog::parse("dead-code warn unused items\nunused-variables warn unused bindings\nunused dead-code, unused-variables\nwarnings all lints that are set to issue warnings");
        assert_eq!(catalog.members("unused").unwrap().len(), 2);
        assert!(catalog.members("warnings").is_none());
        assert!(catalog.members("nonexistent").is_none());
    }

    #[test]
    fn diagnostic_difference_counts_occurrences_and_ignores_order() {
        let d = Diagnostic {
            package_id: "p".into(),
            target: "lib".into(),
            target_root: "src/lib.rs".into(),
            target_kind: vec!["lib".into()],
            lint: "dead_code".into(),
            message: "unused".into(),
            primary_spans: vec![],
        };
        assert!(difference(&[d.clone(), d.clone()], std::slice::from_ref(&d)).is_empty());
        assert_eq!(
            difference(std::slice::from_ref(&d), &[d.clone(), d.clone()]).len(),
            1
        );
    }
}
