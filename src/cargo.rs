use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Default)]
pub struct Options {
    pub workspace: bool,
    pub packages: Vec<String>,
    pub all_targets: bool,
    pub clippy: bool,
    pub json: bool,
    pub manifest: Option<String>,
    pub build_args: Vec<String>,
}

impl Options {
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Option<Self>> {
        let mut args = args.peekable();
        if args.peek().is_some_and(|arg| arg == "lint-audit") {
            args.next();
        }
        let mut options = Self::default();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--workspace" => options.workspace = true,
                "--all-targets" => options.all_targets = true,
                "--clippy" => options.clippy = true,
                "--json" => options.json = true,
                "-p" | "--package" => options.packages.push(next_value(&mut args, &arg)?),
                "--manifest-path" => options.manifest = Some(next_value(&mut args, &arg)?),
                "--features" | "--target" => {
                    options.build_args.push(arg.clone());
                    options.build_args.push(next_value(&mut args, &arg)?);
                }
                "--all-features" | "--no-default-features" | "--offline" | "--locked" => {
                    options.build_args.push(arg);
                }
                "-h" | "--help" => {
                    println!(
                        "Usage: cargo lint-audit [--workspace | --package NAME] [--all-targets]\n\
                        [--clippy] [--json] [--manifest-path PATH]\n\
                        [--features FEATURES] [--all-features] [--no-default-features]\n\
                        [--target TRIPLE] [--offline] [--locked]\n\n\
                        Compares baseline and forced lint diagnostics without editing sources.\n\
                        A diagnostic in an acknowledgement's scope does not prove responsibility.\n\
                        NO_DIAGNOSTIC_OBSERVED applies only to this build configuration."
                    );
                    return Ok(None);
                }
                _ => return Err(format!("unsupported argument: {arg}").into()),
            }
        }
        if options.workspace && !options.packages.is_empty() {
            return Err("choose either --workspace or --package".into());
        }
        Ok(Some(options))
    }

    pub fn manifest_args(&self) -> Vec<String> {
        self.manifest
            .as_ref()
            .map(|path| vec!["--manifest-path".into(), path.clone()])
            .unwrap_or_default()
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}

#[derive(Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_members: Vec<String>,
    pub workspace_default_members: Vec<String>,
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
}

#[derive(Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub manifest_path: PathBuf,
    pub targets: Vec<Target>,
}

#[derive(Deserialize)]
pub struct Target {
    pub src_path: PathBuf,
}

pub fn command() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")))
}

pub fn metadata(options: &Options) -> Result<Metadata> {
    let output = command()
        .args(["metadata", "--no-deps", "--format-version=1"])
        .args(options.manifest_args())
        .args(
            options
                .build_args
                .iter()
                .filter(|arg| matches!(arg.as_str(), "--offline" | "--locked")),
        )
        .output()?;
    ensure_success(&output, "cargo metadata")?;
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn selected<'a>(metadata: &'a Metadata, options: &Options) -> Result<Vec<&'a Package>> {
    let mut ids = if options.workspace {
        metadata.workspace_members.clone()
    } else {
        metadata.workspace_default_members.clone()
    };
    if !options.packages.is_empty() {
        ids.clear();
        for name in &options.packages {
            let matches: Vec<_> = metadata
                .packages
                .iter()
                .filter(|p| {
                    metadata.workspace_members.contains(&p.id) && (&p.name == name || &p.id == name)
                })
                .collect();
            if matches.len() != 1 {
                return Err(format!(
                    "package selection must identify one workspace member: {name}"
                )
                .into());
            }
            ids.push(matches[0].id.clone());
        }
    }
    Ok(metadata
        .packages
        .iter()
        .filter(|p| ids.contains(&p.id))
        .collect())
}

pub fn ensure_success(output: &Output, operation: &str) -> Result<()> {
    if !output.status.success() {
        return Err(format!(
            "{operation} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

pub fn canonical(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()).into())
}

#[derive(Serialize, Deserialize)]
pub struct WrapperConfig {
    pub packages: BTreeMap<PathBuf, Vec<String>>,
    pub previous_wrapper: Option<OsString>,
}

pub fn wrap_compiler() -> Result<i32> {
    let config: WrapperConfig = serde_json::from_slice(&fs::read(
        env::var_os("LINT_AUDIT_WRAPPER_CONFIG").ok_or("missing wrapper configuration")?,
    )?)?;
    let mut args = env::args_os().skip(1);
    let compiler = args.next().ok_or("missing compiler path")?;
    let mut args: Vec<_> = args.collect();
    // Cargo also calls wrappers to query compiler capabilities. Those queries must stay unchanged.
    if args.iter().any(|arg| arg == "--crate-name") {
        if let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") {
            let manifest_dir = canonical(Path::new(&manifest_dir))?;
            if let Some(flags) = config.packages.get(&manifest_dir) {
                args.extend(flags.iter().map(OsString::from));
            }
        }
    }
    let mut command = if let Some(previous) = &config.previous_wrapper {
        let mut command = Command::new(previous);
        command.arg(compiler);
        command
    } else {
        Command::new(compiler)
    };
    // Compiler subprocesses that launch Cargo should use the original wrapper configuration.
    command.env_remove("LINT_AUDIT_WRAPPER_CONFIG");
    if let Some(previous) = &config.previous_wrapper {
        command.env("RUSTC_WRAPPER", previous);
    } else {
        command.env_remove("RUSTC_WRAPPER");
    }
    Ok(command.args(args).status()?.code().unwrap_or(1))
}
