use crate::{
    cargo::{self, Options, Package, WrapperConfig},
    diagnostics::{self, Catalog, Diagnostic, Messages},
    inventory::{self, Candidate},
    Result,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize)]
pub struct Pass {
    pub success: bool,
    pub milliseconds: u128,
    pub fresh_selected_units: usize,
    pub compiled_selected_units: usize,
    pub fresh_all_units: usize,
    pub compiled_all_units: usize,
    pub stderr: String,
    pub errors: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub struct Finding {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub status: &'static str,
    pub explanation: String,
    pub attribution: &'static str,
    pub diagnostics: Vec<Diagnostic>,
    pub baseline_unfulfilled_expectations: Vec<Diagnostic>,
}

#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub mode: &'static str,
    pub rustc_version: String,
    pub selected_packages: Vec<String>,
    pub configuration: Vec<String>,
    pub forced_lints: BTreeMap<String, Vec<String>>,
    pub baseline: Pass,
    pub forced: Pass,
    pub findings: Vec<Finding>,
    pub newly_observed_diagnostics: Vec<Diagnostic>,
    pub limitations: BTreeSet<String>,
}

pub fn run(options: &Options) -> Result<Report> {
    let metadata = cargo::metadata(options)?;
    let packages = cargo::selected(&metadata, options)?;
    if packages.is_empty() {
        return Err("no selected workspace packages".into());
    }
    let inventory = inventory::discover(&packages, &metadata.workspace_root)?;
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = Command::new(&rustc).arg("--version").output()?;
    cargo::ensure_success(&version, "rustc --version")?;
    let mut catalog_command = if options.clippy {
        Command::new("clippy-driver")
    } else {
        Command::new(&rustc)
    };
    let help = catalog_command
        .args(["-W", "help"])
        .env_remove("CLIPPY_ARGS")
        .output()?;
    cargo::ensure_success(&help, "compiler lint catalog query")?;
    let catalog = Catalog::parse(&String::from_utf8_lossy(&help.stdout));
    if !catalog.lints.contains("dead_code") {
        return Err("unrecognized compiler lint catalog format".into());
    }
    let mut flags: BTreeMap<_, BTreeSet<_>> = packages
        .iter()
        .map(|package| {
            Ok((
                cargo::canonical(
                    package
                        .manifest_path
                        .parent()
                        .ok_or("manifest has no directory")?,
                )?,
                BTreeSet::new(),
            ))
        })
        .collect::<Result<_>>()?;
    for candidate in &inventory.candidates {
        if catalog.members(&candidate.lint).is_some() {
            let package = packages
                .iter()
                .find(|p| p.id == candidate.package_id)
                .ok_or("candidate package missing")?;
            let directory = cargo::canonical(
                package
                    .manifest_path
                    .parent()
                    .ok_or("manifest has no directory")?,
            )?;
            flags
                .get_mut(&directory)
                .ok_or("wrapper package missing")?
                .insert(format!("--force-warn={}", candidate.lint));
        }
    }
    let config = WrapperConfig {
        packages: flags
            .into_iter()
            .map(|(p, f)| (p, f.into_iter().collect()))
            .collect(),
        previous_wrapper: env::var_os("RUSTC_WRAPPER").filter(|p| !p.is_empty()),
    };
    let selected: BTreeSet<_> = packages.iter().map(|p| p.id.clone()).collect();
    let cache = metadata.target_directory.join("lint-audit");
    fs::create_dir_all(&cache)?;
    let (baseline, baseline_messages) = check(
        options,
        &packages,
        &selected,
        &metadata.workspace_root,
        &cache,
        &config,
        false,
    )?;
    let (forced, forced_messages) = check(
        options,
        &packages,
        &selected,
        &metadata.workspace_root,
        &cache,
        &config,
        true,
    )?;
    let new = diagnostics::difference(&baseline_messages.diagnostics, &forced_messages.diagnostics);
    let findings = inventory
        .candidates
        .into_iter()
        .map(|candidate| {
            classify(
                candidate,
                &catalog,
                &baseline,
                &forced,
                &baseline_messages,
                &forced_messages,
                &new,
            )
        })
        .collect();
    let mut limitations = inventory.limitations;
    limitations.extend([
        "Scope matches do not establish the effective lint-level source or prove that removing an acknowledgement would expose a diagnostic.".into(),
        "Only the selected target/feature configuration was checked; NO_DIAGNOSTIC_OBSERVED is not a recommendation to remove an acknowledgement.".into(),
        "warnings is a dynamic pseudo-group and cannot be forced; its acknowledgements remain UNKNOWN.".into(),
        "The stable lint catalog comes from compiler -W help text; unknown, renamed, and removed names remain UNKNOWN.".into(),
        "The wrapper supersedes build.rustc-wrapper configuration; an inherited RUSTC_WRAPPER environment setting is chained.".into(),
        "Cargo compiler messages omit the compilation unit's cfg/test/features; duplicate diagnostic occurrences are compared as a multiset.".into(),
        "Procedural macro output, include!, and macro-generated attributes are not inventoried. Macro expansion spans are not attributed.".into(),
    ]);
    let forced_lints = packages
        .iter()
        .map(|package| {
            let directory =
                cargo::canonical(package.manifest_path.parent().unwrap_or(Path::new(".")));
            let names = directory
                .ok()
                .and_then(|p| config.packages.get(&p).cloned())
                .unwrap_or_default();
            (package.name.clone(), names)
        })
        .collect();
    Ok(Report {
        schema_version: 1,
        mode: if options.clippy { "clippy" } else { "rustc" },
        rustc_version: String::from_utf8_lossy(&version.stdout).trim().into(),
        selected_packages: packages.iter().map(|p| p.name.clone()).collect(),
        configuration: options.build_args.clone(),
        forced_lints,
        baseline,
        forced,
        findings,
        newly_observed_diagnostics: new,
        limitations,
    })
}

fn check(
    options: &Options,
    packages: &[&Package],
    selected: &BTreeSet<String>,
    workspace: &Path,
    cache: &Path,
    config: &WrapperConfig,
    forced: bool,
) -> Result<(Pass, Messages)> {
    let config = WrapperConfig {
        packages: config
            .packages
            .iter()
            .map(|(p, flags)| (p.clone(), if forced { flags.clone() } else { Vec::new() }))
            .collect(),
        previous_wrapper: config.previous_wrapper.clone(),
    };
    let bytes = serde_json::to_vec(&config)?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    options.clippy.hash(&mut hasher);
    // Wrapper-injected flags are invisible to Cargo's ordinary flag fingerprint.
    // Separate caches prevent a fresh baseline artifact from skipping the forced compile.
    let directory = cache.join(format!(
        "{}-{:016x}",
        if forced { "forced" } else { "baseline" },
        hasher.finish()
    ));
    fs::create_dir_all(&directory)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let config_path = directory.join(format!("wrapper-{}-{nonce}.json", std::process::id()));
    fs::write(&config_path, bytes)?;
    let mut command = cargo::command();
    command
        .arg(if options.clippy { "clippy" } else { "check" })
        .arg("--message-format=json")
        .arg("--target-dir")
        .arg(&directory)
        .args(options.manifest_args())
        .args(&options.build_args)
        .env("RUSTC_WRAPPER", env::current_exe()?)
        .env("LINT_AUDIT_WRAPPER_CONFIG", &config_path);
    for package in packages {
        command.args(["--package", &package.id]);
    }
    if options.all_targets {
        command.arg("--all-targets");
    }
    if options.clippy {
        command.arg("--no-deps");
    }
    let start = Instant::now();
    let output = command.output();
    let elapsed = start.elapsed().as_millis();
    fs::remove_file(&config_path)?;
    let output = output?;
    let messages = diagnostics::parse(&output.stdout, selected, workspace)?;
    Ok((
        Pass {
            success: output.status.success() && messages.finished,
            milliseconds: elapsed,
            fresh_selected_units: messages.fresh,
            compiled_selected_units: messages.compiled,
            fresh_all_units: messages.all_fresh,
            compiled_all_units: messages.all_compiled,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            errors: messages.errors.clone(),
        },
        messages,
    ))
}

fn classify(
    candidate: Candidate,
    catalog: &Catalog,
    baseline: &Pass,
    forced: &Pass,
    baseline_messages: &Messages,
    forced_messages: &Messages,
    new: &[Diagnostic],
) -> Finding {
    let members = catalog.members(&candidate.lint);
    let matched: Vec<_> = new
        .iter()
        .filter(|d| {
            d.package_id == candidate.package_id
                && members.as_ref().is_some_and(|m| m.contains(&d.lint))
                && in_scope(&candidate, d)
        })
        .cloned()
        .collect();
    let expectations = baseline_messages
        .diagnostics
        .iter()
        .filter(|d| {
            candidate.kind == "expect"
                && d.package_id == candidate.package_id
                && d.lint == "unfulfilled_lint_expectations"
                && d.primary_spans
                    .iter()
                    .any(|span| span.file == candidate.file && span.line as usize == candidate.line)
        })
        .cloned()
        .collect();
    let covered = candidate.target_roots.iter().any(|root| {
        let key = (candidate.package_id.clone(), root.clone());
        baseline_messages.roots.contains(&key) && forced_messages.roots.contains(&key)
    });
    let unmappable = new.iter().any(|diagnostic| {
        diagnostic.package_id == candidate.package_id
            && members
                .as_ref()
                .is_some_and(|m| m.contains(&diagnostic.lint))
            && (diagnostic.primary_spans.is_empty()
                || diagnostic.primary_spans.iter().any(|span| {
                    !span.file_resolved
                        || span.expansion
                            && (candidate.kind == "manifest_allow"
                                || candidate.scopes.iter().any(|scope| {
                                    scope.file == span.file
                                        && scope.start as u64 <= span.byte_start
                                        && scope.end as u64 >= span.byte_end
                                }))
                }))
    });
    let (status, explanation) = if !baseline.success || !forced.success {
        (
            "UNKNOWN",
            "A Cargo pass failed; absence and attribution are inconclusive.",
        )
    } else if members.is_none() {
        ("UNKNOWN", "This name cannot be safely forced in the selected compiler mode (special group, other tool, unknown, renamed, or removed lint).")
    } else if !matched.is_empty() {
        ("DIAGNOSTIC_OBSERVED", "New diagnostics overlap this candidate's source scope. This does not prove that the candidate is active or responsible for suppression.")
    } else if unmappable {
        ("UNKNOWN", "A matching diagnostic has no primary span, an unresolved source path, or a macro expansion that cannot be safely attributed.")
    } else if candidate.conditional {
        ("UNKNOWN", "Conditional syntax is inventoried but its effective cfg/lint level was not established.")
    } else if !covered {
        ("UNKNOWN", "No successful compilation of a target statically associated with this candidate was observed in both passes.")
    } else {
        ("NO_DIAGNOSTIC_OBSERVED", "No new matching diagnostic was observed in this static scope for the checked configuration. This does not prove redundancy.")
    };
    Finding {
        candidate,
        status,
        explanation: explanation.into(),
        attribution: "unproven",
        diagnostics: matched,
        baseline_unfulfilled_expectations: expectations,
    }
}

fn in_scope(candidate: &Candidate, diagnostic: &Diagnostic) -> bool {
    diagnostic.primary_spans.iter().any(|span| {
        !span.expansion
            && (candidate.kind == "manifest_allow"
                || candidate.scopes.iter().any(|scope| {
                    scope.file == span.file
                        && scope.start as u64 <= span.byte_start
                        && scope.end as u64 >= span.byte_end
                }))
    })
}

impl Report {
    pub fn print(&self) {
        println!(
            "Lint audit ({}) for {}",
            self.mode,
            self.selected_packages.join(", ")
        );
        println!(
            "Baseline: {} ms; forced: {} ms; {} new diagnostic occurrences\n",
            self.baseline.milliseconds,
            self.forced.milliseconds,
            self.newly_observed_diagnostics.len()
        );
        for finding in &self.findings {
            let candidate = &finding.candidate;
            println!(
                "{}:{}:{} [{}]\n  {}\n  lint: {} ({})",
                candidate.file,
                candidate.line,
                candidate.column,
                candidate.package,
                candidate.source,
                candidate.lint,
                candidate.kind
            );
            if let Some(reason) = &candidate.reason {
                println!("  reason: {reason}");
            }
            println!("  status: {}\n  {}", finding.status, finding.explanation);
            for diagnostic in &finding.diagnostics {
                println!(
                    "  newly observed: {} [{} / {}]",
                    diagnostic.message, diagnostic.lint, diagnostic.target
                );
                for span in &diagnostic.primary_spans {
                    println!("    {}:{}:{}", span.file, span.line, span.column);
                }
            }
            if !finding.baseline_unfulfilled_expectations.is_empty() {
                println!("  rustc reported an unfulfilled expectation in the baseline.");
            }
            println!();
        }
        if !self.baseline.success {
            eprintln!("Baseline failed:\n{}", self.baseline.stderr);
        }
        if !self.forced.success {
            eprintln!("Forced pass failed:\n{}", self.forced.stderr);
        }
        println!("Limits:");
        for limitation in &self.limitations {
            println!("  {limitation}");
        }
        if !self.newly_observed_diagnostics.is_empty() {
            println!("Use --json to inspect all new diagnostics, including those without a source-scope match.");
        }
    }
}
