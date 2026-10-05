# cargo-lint-audit investigation

The external prototype can inventory acknowledgements and surface diagnostics that appear when their lint names are forced. It cannot reliably identify the exact acknowledgement responsible for suppressing a diagnostic. The missing causal information is inside rustc's lint-level handling, before JSON serialization. The prototype therefore makes no `ACTIVE` or `STALE` claims and performs no removal.

Investigation date: 2026-10-05. No upstream source was modified, and no issue, PR, commit, or maintainer message was created.

## Upstream validation

Current upstream source was fetched through GitHub's contents API at these pinned revisions, separately from the available local checkouts:

| Repository | Upstream revision inspected | Local checkout inspected |
| --- | --- | --- |
| Cargo | `bb2126cffae48394a37db8728dc50a17bbfe54d4` | `472de443d99835fe76c3dbe27f233eba2a8144ed` |
| rustc | `cc9a14f721fac5226338c61dcec7d5ab785bde82` | `cf8cf61f5ed9b2b314a2992629ad646c49869f36` |
| Clippy | `9066dbb89ca01eddd591d03a2b4504ab28d7c26b` | Rust checkout's `src/tools/clippy` |

The executable experiments used stable `rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo `1.98.1 (797e8a9bc 2026-08-05)`, and Clippy `0.1.98`, on `x86_64-pc-windows-msvc`. The direct rustc experiment matrix was also run on installed nightly `1.100.0-nightly (a36d05efa 2026-09-09)`. Upstream source inspection is not a claim that these exact upstream revisions were built. No older MSRV, Linux/macOS runtime, cross-compilation target, or large workspace was tested.

Verified source paths:

- [rustc lint-level builder](https://github.com/rust-lang/rust/blob/cc9a14f721fac5226338c61dcec7d5ab785bde82/compiler/rustc_lint/src/levels.rs): command-line lint/group resolution, explicit rejection of forcing `warnings`, renamed/removed handling, and `ForceWarn`/expectation precedence.
- [rustc effective lint levels and emission](https://github.com/rust-lang/rust/blob/cc9a14f721fac5226338c61dcec7d5ab785bde82/compiler/rustc_middle/src/lint.rs): dynamic `warnings` handling, cap exception, expectation IDs, and `EmissionOverride::Forced`.
- [rustc JSON serialization](https://github.com/rust-lang/rust/blob/cc9a14f721fac5226338c61dcec7d5ab785bde82/compiler/rustc_errors/src/json.rs): diagnostic codes, primary spans and macro expansion information, without effective lint-level source or expectation identity.
- [rustc print implementation](https://github.com/rust-lang/rust/blob/cc9a14f721fac5226338c61dcec7d5ab785bde82/compiler/rustc_driver_impl/src/lib.rs): `CrateRootLintLevels` builds a crate-root lint-level provider, rather than visiting local scopes.
- [Cargo compiler process selection](https://github.com/rust-lang/cargo/blob/bb2126cffae48394a37db8728dc50a17bbfe54d4/src/compiler/compilation.rs), [extra rustc argument handling](https://github.com/rust-lang/cargo/blob/bb2126cffae48394a37db8728dc50a17bbfe54d4/src/ops/cargo_compile/mod.rs), and [manifest lint conversion](https://github.com/rust-lang/cargo/blob/bb2126cffae48394a37db8728dc50a17bbfe54d4/src/workspace/parser/mod.rs): workspace wrapper selection, the single-target restriction on extra rustc arguments, and namespace/priority conversion of manifest lint settings.
- [Clippy Cargo launcher](https://github.com/rust-lang/rust-clippy/blob/9066dbb89ca01eddd591d03a2b4504ab28d7c26b/src/main.rs) and [driver](https://github.com/rust-lang/rust-clippy/blob/9066dbb89ca01eddd591d03a2b4504ab28d7c26b/src/driver.rs): Clippy occupies `RUSTC_WORKSPACE_WRAPPER`; `--no-deps` uses `CARGO_PRIMARY_PACKAGE`; its cap check specially recognizes forced Clippy lints.
- [Clippy allow-attribute check](https://github.com/rust-lang/rust-clippy/blob/9066dbb89ca01eddd591d03a2b4504ab28d7c26b/clippy_lints/src/attrs/allow_attributes.rs) and [reason checker](https://github.com/rust-lang/rust-clippy/blob/9066dbb89ca01eddd591d03a2b4504ab28d7c26b/clippy_lints/src/attrs/allow_attributes_without_reason.rs): structural attribute inspection, a reason-presence check, and an `expect` replacement suggestion, rather than testing whether the acknowledgement hides a diagnostic.

The current discussion in [cargo#17526](https://github.com/rust-lang/cargo/issues/17526) recommends third-party exploration of an allow auditor and asks for a clearer use case. [cargo#17442](https://github.com/rust-lang/cargo/issues/17442) concerns separating autofix selection from lint visibility; [RFC#3926](https://github.com/rust-lang/rfcs/pull/3926) proposes lint profiles. Neither is a causal suppression audit. [rust#142610](https://github.com/rust-lang/rust/issues/142610) tracks the need to specify lint-level interactions.

GitHub issue/PR searches across `org:rust-lang` included `"lint" "audit"`, `"suppressed" "lint"`, `"allow auditor"`, `"lint acknowledgement auditor"`, `"suppressed lint audit"`, `"suppression" "audit"`, `"unfix"`, and unnecessary/unused/redundant allow terms. The specific auditor phrases returned only #17526 or no results. Broader searches found unrelated lint cleanups and adjacent features. No equivalent implementation or active implementation claim was found in the results inspected. This is a bounded search result, not proof that no external tool exists anywhere. The implementation claim in #17442 refers to skipping autofixes, a different task.

## Existing mechanisms

Stable `--force-warn` accepts individual lints and ordinary groups. Cargo already passes edition migration groups through this mechanism; [the current source](https://github.com/rust-lang/cargo/blob/bb2126cffae48394a37db8728dc50a17bbfe54d4/src/workspace/features.rs) uses both edition-future-compatibility and edition-specific flags. [Lint-level documentation](https://doc.rust-lang.org/rustc/lints/levels.html) describes forced warning precedence.

`rustc --print=crate-root-lint-levels` was rejected without `-Zunstable-options`. With nightly and that flag, the probe containing an outer `#[allow(dead_code)]` still printed crate-root `dead_code=warn`. This confirms the lexical inventory gap experimentally, in addition to the print implementation's crate-root provider.

`unfulfilled_lint_expectations` already identifies unfulfilled expectations in the configuration actually compiled. It is valuable baseline evidence, but does not audit allows or prove that removing a fulfilled expectation will expose a warning under the surrounding policy.

## Exact tooling gap

Three distinct questions must stay separate:

1. Which explicit acknowledgements exist, and what are their reasons?
2. Which diagnostic occurrences appear under forced lint evaluation?
3. Which acknowledgement actually supplied the effective suppressing level, and would removing it change observable diagnostics?

The prototype answers the first two for supported source syntax and build configurations. Stable forced-diagnostic JSON does not answer the third.

The direct probes give a concrete counterexample: `#[allow(trivial_casts)]` around a reference-to-pointer cast produces no baseline diagnostic and a forced diagnostic. Removing the attribute still produces no baseline diagnostic and the same kind of forced diagnostic, because `trivial_casts` is allow-by-default. A forced-only diagnostic therefore does not even prove that an explicit acknowledgement was needed.

Nested allows give another counterexample: an outer module allow and an inner function allow both cover one forced `dead_code` diagnostic. Removing the inner allow alone would leave the outer allow suppressing it. A scope match cannot establish redundancy, necessity, or exact responsibility.

## Prototype architecture

The implementation is an external stable Rust binary, with five modules: entry point, Cargo invocation/wrapper, source inventory, diagnostics, and audit/reporting. No Cargo fork, rustc_private, nightly dependency, unsafe code, source rewriting, or per-acknowledgement rebuild is used.

`syn` parses Rust attributes and syntax; `proc-macro2` provides source byte spans. This handles comments, strings, multiple lints, reasons, and nested metadata without regex matching. It does not expand macros or resolve compiler cfg semantics. Compiler-private parsing would add toolchain coupling without supplying the missing causal information. `serde`/`serde_json` parse structured Cargo output and serialize reports; `toml` parses lint configuration. No CLI framework or Cargo library dependency was added.

Workflow:

1. Query `cargo metadata --no-deps --format-version=1`, select local members, and inventory their discovered target roots and ordinary module graphs.
2. Read explicit package lint settings or inherited `[workspace.lints]` when `[lints] workspace=true`.
3. Read the active compiler's `-W help` catalog. Validate lint/group names and obtain group members. This is help-text parsing, not parsing rendered diagnostic messages; there is no stable structured catalog in use here.
4. Run Cargo check, or Cargo Clippy with `--no-deps`, with an outer compiler wrapper. The wrapper forwards unchanged compiler queries and injects flags only when `CARGO_MANIFEST_DIR` identifies a selected package.
5. Force all recognized acknowledged names for each selected package together in a second pass. Keep baseline and forced caches separate and key the forced cache by the injected configuration.
6. Compare diagnostics as a multiset of package ID, target name/kind/root, lint code, sorted primary source spans, and message. Severity is excluded so a forced warning is not counted as new solely because it was formerly an error.
7. Associate new diagnostics with static scopes, retain unassigned diagnostics, and report uncertainty explicitly.

[Cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html) and [Cargo JSON messages](https://doc.rust-lang.org/cargo/reference/external-tools.html) supply package/target identity and successful/fresh artifact records. Rustc's [JSON format](https://doc.rust-lang.org/rustc/json.html) supplies diagnostic spans. Paths are canonicalized when possible; compiler path remapping remains a limitation.

`cargo rustc -- ...` was rejected as the main architecture because Cargo requires a single selected target for extra arguments. Per-target invocations would add orchestration and repeat work. `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` have broader propagation and would force dependency lints too. `RUSTC_WORKSPACE_WRAPPER` is a good rustc-only boundary, but Clippy sets that variable itself. The outer `RUSTC_WRAPPER`, with a selected-manifest allowlist, accommodates both modes. [Cargo's environment-variable documentation](https://doc.rust-lang.org/cargo/reference/environment-variables.html) describes the wrapper nesting contract.

An inherited environment `RUSTC_WRAPPER` is chained. A wrapper configured only in `build.rustc-wrapper` is superseded in both passes; the report discloses this. Unusual compiler/wrapper policies need dedicated validation. The auditor performs Cargo compilation, including build-script execution, and may create the usual Cargo lockfile and cache artifacts.

## What works

Seven automated tests passed: three focused parsing/difference tests and four end-to-end tests that compile copied Cargo fixtures. The integration coverage includes active and unused allows, reasons, fulfilled/unfulfilled expectations, multiple lints, inner attributes, ordinary groups, external modules, package and inherited manifest settings, Clippy, selected/default workspace members, an excluded path dependency, unbuilt targets, feature cfgs, macro uncertainty, build failure, repeated cached audits, and a source edit invalidating cached results.

The fixture exercises binaries, a library, a build script, integration tests, an example, and a bench. `--all-targets` checks these; it does not execute tests/benches or run doctests. Known acknowledged lints are batched per compilation. A repeat audit produced identical findings and diagnostic evidence with zero compiled units.

The additional direct compiler script exercised 23 cases, each baseline and forced, on stable and nightly. [Recorded compiler outcomes](investigation/compiler-results.json) preserve their exit codes and diagnostic codes. Formatting and Clippy with `-D warnings` passed for the tool itself.

## What does not work

- Exact responsible-acknowledgement attribution, necessity/removability, and a trustworthy global `STALE` result.
- Forcing the dynamic `warnings` pseudo-group; these candidates stay `UNKNOWN`.
- Complete macro/generated-source inventory, arbitrary `include!` graphs, procedural expansion, or exact lexical semantics for every Rust construct.
- Effective cfg resolution, including cfg produced by build scripts and conditional module paths. Missing/unresolved module files and parse failures are disclosed.
- A full feature/platform/profile matrix, doctests, rustdoc/Cargo lints, or nonstandard compiler drivers.
- Structured stable lint catalog discovery. The help format is checked for basic recognition, but ordinary group enumeration still depends on that format.
- Avoiding duplicate cold dependency compilation with the current deliberately separate caches.

The report uses `DIAGNOSTIC_OBSERVED` for new diagnostic scope overlaps, `NO_DIAGNOSTIC_OBSERVED` for scoped absence in a successful statically associated compilation, and `UNKNOWN` for unsupported names, failed passes, conditional/unbuilt scopes, and unmappable expansion evidence. Every candidate has `attribution: "unproven"`. These are evidence classifications, not proof of an effective allow/expect decision.

## allow semantics

The stable and nightly probes both showed:

| Existing setting | Baseline | With `--force-warn=dead_code` |
| --- | --- | --- |
| Outer allow / inner allow | No dead-code warning | Warning |
| Allow with reason | No warning | Warning; force source shown instead of the acknowledgement's reason |
| Used function with allow | No warning | No warning |
| Deny | Error, exit 1 | Warning, exit 0 |
| Forbid | Error, exit 1 | Warning, exit 0 |
| Allow plus `--cap-lints=allow` | No warning | Warning |
| `allow(warnings)` | No warning | Warning when the individual leaf lint is forced |

The auditor preserves reasons by parsing source, not by expecting forced diagnostics to retain them. It leaves baseline errors visible and returns failure if either Cargo pass fails; forcing a deny/forbid is not treated as repairing the baseline.

Command-line policy, manifest policy, defaults, ancestor attributes, and overlapping groups can all explain baseline silence. No assumption that the nearest parsed allow is responsible is made.

## expect semantics

A fulfilled `#[expect(dead_code)]` remained fulfilled when the lint was forced, and the previously hidden dead-code diagnostic appeared. A used function with `expect(dead_code)` emitted an unfulfilled-expectation warning in both passes. An expected `unused` group was fulfilled by forced constituent diagnostics.

An outer expectation shadowed by an inner allow remained unfulfilled, even though forcing the leaf produced a warning inside its static scope. The forced warning alone therefore cannot be used to mark the outer expectation fulfilled. Baseline expectation diagnostics are reported separately.

In `LintLevelsBuilder::insert_spec`, an existing `ForceWarn` combined with a new `Expect` retains the force source and saves the expectation ID. Combining `ForceWarn` with another level retains the force source and drops the expectation ID. Emission carries that ID internally through `EmissionOverride::Forced`; JSON does not serialize it. The available upstream force-warning expectation UI tests were inspected and the executable probes matched their relevant semantics.

## lint groups

`--force-warn=unused` emitted concrete `dead_code`, `unused_variables`, and `unused_mut` codes in the probe. The diagnostic code is the constituent lint, not the group acknowledgement. The catalog supplies group membership for scope associations; overlapping group/leaf attributes can receive the same evidence without establishing responsibility.

`warnings` is special: it applies to lints whose effective level would be `Warn` at the point of final level resolution. It is not a fixed membership list equivalent to all warn-by-default lints. The command-line builder explicitly rejects `--force-warn warnings`; the probe returned `E0602` and exit 1. Forcing a leaf bypasses `allow(warnings)` successfully.

Enumerating every possible lint as a workaround would additionally enable allow-by-default and deny-by-default checks, producing a different operation from auditing acknowledged warnings. The prototype does not silently substitute that behavior. This limitation affects coverage, but does not explain the loss of causal attribution for ordinary leaf lints.

## Clippy

Rustc mode leaves Clippy names `UNKNOWN` because rustc alone does not evaluate those tool lints. Clippy mode uses the Clippy lint catalog and runs both passes through `cargo clippy --no-deps`; recognized acknowledged rustc lints are included too. [Clippy usage](https://doc.rust-lang.org/clippy/usage.html) describes its Cargo interface.

Both `allow(clippy::needless_return)` and `expect(clippy::needless_return)` produced new `clippy::needless_return` diagnostics; the expectation had no baseline unfulfilled warning. The selected-package outer wrapper nests around Clippy's own workspace driver, leaving the driver installed by Cargo Clippy intact. No Clippy diagnostic from the excluded dependency was included.

## Cargo.toml lints

The fixture verified `[lints.rust]` table settings with `level`/`priority`, `[lints.clippy]` string settings, and inherited `[workspace.lints.rust]` through `lints.workspace=true`. Inventory records explicit allows rather than pretending to reimplement all priority and attribute precedence. Cargo itself performs that resolution during compilation. [Manifest lint documentation](https://doc.rust-lang.org/cargo/reference/manifest.html#the-lints-section) specifies these settings.

Manifest scope association is package-wide. It can overlap local attributes that actually determine the effective level. Its line locator supports ordinary table/key spellings and discloses unresolved locations for more elaborate valid TOML forms. The original parsed TOML value governs discovery, so the line locator is not used to recognize lint settings.

## workspaces / dependencies

Default selection uses metadata's default members; `--workspace` and repeated exact `--package` selections are implemented. The fixture's unselected `beta` member was compiled as a dependency of `alpha` but was not forced in the default audit. Selecting the workspace exposed `beta`'s manifest-allowed dead code. The excluded path dependency was compiled but received no injected flags and was omitted from the diagnostic comparison. Its cached fingerprint directories had no cached lint-output file in the measured runs.

The wrapper allowlist controls injection, and package-ID filtering independently controls reporting. Using global rustflags and then merely filtering output would not provide the same isolation, especially because `--force-warn` bypasses dependency lint caps.

Cargo artifact records establish successful compiled roots even when the compiler does not execute again. Missing target roots remain `UNKNOWN`. Cargo compiler messages do not attach unit cfg/test/features, so the multiset comparison cannot fully distinguish duplicate normal/test compilations sharing a target root. That limits attribution precision; it does not justify combining dependency messages with member messages.

## cfg and macros

The parser inventories `cfg_attr` payloads and marks candidates under cfg syntax conditional. It follows ordinary `mod foo;`, `foo.rs`/`foo/mod.rs`, inline modules, and literal `#[path]` where resolvable. An outer module scope is extended to the statically discovered child files. These are source relationships, not compiler-confirmed active scopes.

Without the fixture's feature, the cfg-attribute candidate had no new diagnostic and remained `UNKNOWN`. With the feature, a new diagnostic overlapped its source scope, reported as an observation with unproven activity/responsibility. A separate cfg-gated item was not mislabeled stale.

Macro token bodies are not parsed as active Rust items. Known lint names can still reveal macro-generated diagnostics, which remain in the full evidence list. Expansion spans are excluded from ordinary scope associations; an otherwise matching macro-scope candidate becomes `UNKNOWN`. An include/procedural expansion can generate an acknowledgement name never discovered by the parser, so this prototype cannot force it or claim inventory completeness.

## performance

One rough Windows measurement per state, using the dependency-free fixture with two workspace members, one excluded path dependency, and all targets. Values are milliseconds; no statistical confidence or large-project extrapolation is implied. [Raw measurements](investigation/measurements.json) contain timings, artifact counts, and flags; `investigation/experiments/measure-audit.ps1` reproduces them.

| Mode/state | Baseline | Forced | Whole audit | Compiled units per pass, selected / all |
| --- | ---: | ---: | ---: | --- |
| rustc cold | 704 | 691 | 1644 | 8 / 9 |
| rustc cached | 95 | 91 | 409 | 0 / 0 |
| rustc after source edit | 307 | 280 | 836 | 5 / 5 |
| Clippy cold | 840 | 828 | 2004 | 8 / 9 |
| Clippy cached | 199 | 198 | 621 | 0 / 0 |
| Clippy after source edit | 366 | 380 | 976 | 5 / 5 |

Alpha's four rustc flags, or five flags in Clippy mode, were injected together; beta had one. This was two Cargo passes per mode, not one full build per lint. Cold passes each compiled all nine units because their caches were separate. After a source edit, unchanged dependencies and the build script stayed fresh. Cached Cargo output replay preserved the 38 rustc or 42 Clippy-mode new diagnostic occurrences without any new compilation.

The costs for larger workspaces include two initial dependency builds, two incremental selected-unit checks after changes, extra lint work for broad groups, parsing, Cargo/tool startup, and potentially large JSON output. Changing the forced lint set creates another forced cache. Sharing unchanged dependency artifacts without risking fresh workspace artifacts skipping wrapper-injected flags deserves separate work; it was not attempted here.

## false-positive / false-negative risks

- A scope-overlapping forced diagnostic can be enabled solely by changing an allow-by-default lint, or by bypassing another acknowledgement, manifest entry, or command-line cap. Calling it `ACTIVE` would overclaim.
- A new diagnostic inside an expectation's scope can belong to an inner allow that leaves the expectation unfulfilled.
- No observation in one build does not establish removal safety across features, platforms, test cfgs, macro expansion, profiles, or tool versions.
- Static scopes approximate compiler lint scope; less-common syntax and module-path rules can differ. Unresolved files and macro coverage limitations are disclosed, not reconstructed through heuristics.
- Removed, unknown, and renamed names remain `UNKNOWN`. Direct probes showed unknown-name diagnostics and rename/remove notices; a renamed dead-code alias still forced a real warning, but the prototype avoids treating every unresolved name as stale.
- Matching is local to a compiler run pair. Diagnostic wording, grouping, duplicate emission, evaluation order, future-incompatibility behavior, or build-script output can change. Source/cfg changes during the two passes and nondeterministic macros/build scripts can invalidate the comparison.
- Read-only source intent does not make compilation inert: Cargo executes build scripts/procedural macros and writes its usual build artifacts.

## upstream implications

The precise causal loss occurs when the lint-level builder sees an attribute after command-line `ForceWarn`: it retains `old_src`, which is already the force-warning command-line source, instead of retaining the otherwise-effective attribute level and source. Expectation IDs survive only internally where applicable. At emission, JSON exposes the warning, lint code, spans, and textual children, but no original level/source/expectation identity. A richer JSON serializer alone cannot recover data discarded earlier.

Even retaining the original effective source would answer which setting won, not whether removing it exposes a diagnostic: a shadowed ancestor allow or an allow-by-default fallback can still suppress it. Those are separate questions requiring either the surrounding policy chain or a defined removal counterfactual.

A useful smaller rustc primitive to discuss would preserve and emit the otherwise-effective lint level and its origin when a warning is forced: default vs command-line vs attribute, original level, source span/name/reason, and expectation association. A broader audit mode could emit normally suppressed acknowledged diagnostics while retaining ordinary expectation accounting. Such a mode must define which allow-by-default lints are evaluated and how allow/expect/deny/forbid/caps interact; blindly enabling every lint is not equivalent. This is a proposal for a separate compiler design discussion, not implemented or assumed to have maintainer approval.

Cargo could eventually attach reliable compilation-unit cfg/test/features to compiler messages and expose selected-unit extra flags with fingerprinting. Those would improve tooling precision and reduce wrapper/cache machinery. Neither would by itself repair the lost lint-level provenance inside rustc.

## recommendation for rust-lang/cargo#17526

Continue third-party experimentation for **inventory, reasons, and diagnostic evidence**. The working prototype validates that this is useful and technically possible with stable Cargo interfaces; it does not validate a reliable allow-removal or per-acknowledgement stale detector.

Do not recommend `cargo unfix`, automatic acknowledgement removal, or a first-class Cargo causal audit on this evidence. In particular, the allow-by-default counterexample and nested allows defeat the proposed inference from forced-only diagnostics to exact responsible attributes. Calling these findings `ACTIVE` would create unjustified confidence.

If the established use case is reviewing acknowledged lint occurrences, the external tool is a sufficient starting point. If the requirement is trustworthy exact acknowledgement responsibility, start a narrowly scoped rustc discussion about preserving otherwise-effective lint-level provenance through forced emission and exposing it structurally. Require an explicit distinction between the effective source and removal necessity. This offers more value than adding Cargo orchestration around uncertain source-span guesses.

`--force-warn warnings` being unsupported is a real coverage hole for blanket acknowledgements, but **it is not the main blocker**. Ordinary leaf lints already exhibit the attribution failure. Enabling that group alone would not distinguish default policy, nested acknowledgements, reasons, expectation identity, or cfg scope.

Consider Cargo improvements only after the use case and compiler evidence are clearer. A selected-unit flag interface with correct fingerprints and richer unit identity could make an external auditor faster and simpler. The current experiment does not establish that Cargo needs to own the auditor, nor that lint profiles or skipped autofixes solve its causal question.
