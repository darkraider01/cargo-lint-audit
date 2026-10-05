# cargo-lint-audit

An experimental external Cargo subcommand that inventories explicit Rust lint acknowledgements and compares baseline diagnostics with a check that forces the discovered lints.

It does not edit source files. A newly observed diagnostic is evidence about the checked compilation, **not proof that a particular attribute suppressed it**. Read [REPORT.md](REPORT.md) before using the results to remove acknowledgements.

```sh
cargo build --locked
cargo install --path . --locked
cd /path/to/project
cargo lint-audit
cargo lint-audit --workspace --all-targets --json
cargo lint-audit --package my-package --features extra
cargo lint-audit --clippy --workspace
```

To run without installing, invoke the built `target/debug/cargo-lint-audit` executable from the project being audited, or supply `--manifest-path /path/to/project/Cargo.toml`.

The default package selection follows Cargo metadata's default workspace members. `--package` accepts an exact workspace package name or package ID and can be repeated. `--workspace` selects all members. `--all-targets` includes test, example, and bench compilation; it does not execute them or run doctests. Build scripts still execute as part of Cargo check.

Supported build arguments are `--features`, `--all-features`, `--no-default-features`, `--target`, `--offline`, and `--locked`. This prototype accepts the separate-value spelling of options. The Clippy mode also checks acknowledged rustc lints, using a separate Clippy baseline and cache.

Results:

| Status | Meaning |
| --- | --- |
| `DIAGNOSTIC_OBSERVED` | A new diagnostic overlaps the candidate's static source scope. Responsibility and effective cfg remain unproven. |
| `NO_DIAGNOSTIC_OBSERVED` | No new matching diagnostic was observed in that scope for a successfully checked, statically associated target. This does not establish redundancy. |
| `UNKNOWN` | The name, compilation, cfg, target coverage, or diagnostic span cannot be assessed safely. |

The JSON output includes all newly observed diagnostics, even those with no attribute match; baseline unfulfilled-expectation diagnostics; candidate reasons and scopes; package/target identity; timing and artifact counts; build errors; and limitations. Findings and diagnostics are sorted; timings and Cargo stderr are variable. The schema is experimental (`schema_version: 1`). Failed builds still produce a JSON report and return exit code 1.

The auditor uses `target/lint-audit/` under Cargo's configured target directory. Baseline and forced caches are separate because Cargo cannot fingerprint flags injected inside a compiler wrapper. Cold passes each build dependencies; subsequent identical audits reuse both caches. It preserves an inherited `RUSTC_WRAPPER` environment wrapper, but supersedes a wrapper set only through `build.rustc-wrapper` configuration. Custom compiler drivers, path remapping, and wrapper-dependent lint policies need separate validation.

Verification and experiments:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

The integration tests copy the fixture to temporary directories under `target/`, compile actual Cargo projects, and remove those copies. Clippy must be installed to run the Clippy integration test. The dependency-free fixture includes two workspace members and an excluded path dependency.

On Windows, the recorded compiler and timing experiments can be repeated with:

```powershell
cargo build --locked
& .\investigation\experiments\probe-rustc.ps1
& .\investigation\experiments\probe-rustc.ps1 -Toolchain nightly
& .\investigation\experiments\measure-audit.ps1
```

The measurement script writes compact evidence to `investigation/measurements.json` and full diagnostic snapshots to a new directory under `target/`. Its timings are measurements of a small fixture, not large-workspace benchmarks.
