mod audit;
mod cargo;
mod diagnostics;
mod inventory;

use std::{env, error::Error, process};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    if env::var_os("LINT_AUDIT_WRAPPER_CONFIG").is_some() {
        match cargo::wrap_compiler() {
            Ok(code) => process::exit(code),
            Err(error) => {
                eprintln!("cargo-lint-audit wrapper: {error}");
                process::exit(1);
            }
        }
    }
    match run() {
        Ok(success) => process::exit(if success { 0 } else { 1 }),
        Err(error) => {
            eprintln!("cargo-lint-audit: {error}");
            process::exit(1);
        }
    }
}

fn run() -> Result<bool> {
    let Some(options) = cargo::Options::parse(env::args().skip(1))? else {
        return Ok(true);
    };
    let report = audit::run(&options)?;
    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report.print();
    }
    Ok(report.baseline.success && report.forced.success)
}
