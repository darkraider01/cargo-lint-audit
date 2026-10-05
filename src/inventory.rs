use crate::{
    cargo::{self, Package},
    diagnostics::normalize,
    Result,
};
use proc_macro2::Span;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
use syn::{
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
    Attribute, Expr, Lit, Meta, Token,
};

#[derive(Clone, Debug, Serialize)]
pub struct Scope {
    pub file: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub package_id: String,
    pub package: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub kind: String,
    pub lint: String,
    pub reason: Option<String>,
    pub source: String,
    pub conditional: bool,
    pub scopes: Vec<Scope>,
    pub target_roots: BTreeSet<String>,
}

#[derive(Default)]
pub struct Inventory {
    pub candidates: Vec<Candidate>,
    pub limitations: BTreeSet<String>,
}

pub fn discover(packages: &[&Package], workspace: &Path) -> Result<Inventory> {
    let mut inventory = Inventory::default();
    for package in packages {
        for target in &package.targets {
            let mut stack = BTreeSet::new();
            let root = target.src_path.to_string_lossy().into_owned();
            let module_dir = target
                .src_path
                .parent()
                .ok_or("target has no source directory")?;
            inventory.file(
                package,
                &target.src_path,
                module_dir,
                &root,
                false,
                &mut stack,
            )?;
        }
        inventory.manifest(package, workspace)?;
    }
    inventory.candidates.sort_by(|a, b| {
        (&a.package_id, &a.file, a.line, a.column, &a.lint).cmp(&(
            &b.package_id,
            &b.file,
            b.line,
            b.column,
            &b.lint,
        ))
    });
    let mut merged: Vec<Candidate> = Vec::new();
    for candidate in inventory.candidates {
        if let Some(previous) = merged.last_mut().filter(|p| {
            p.package_id == candidate.package_id
                && p.file == candidate.file
                && p.line == candidate.line
                && p.column == candidate.column
                && p.lint == candidate.lint
                && p.kind == candidate.kind
        }) {
            previous.target_roots.extend(candidate.target_roots);
            previous.scopes.extend(candidate.scopes);
            previous.conditional |= candidate.conditional;
        } else {
            merged.push(candidate);
        }
    }
    inventory.candidates = merged;
    Ok(inventory)
}

impl Inventory {
    fn file(
        &mut self,
        package: &Package,
        path: &Path,
        module_dir: &Path,
        root: &str,
        conditional: bool,
        stack: &mut BTreeSet<PathBuf>,
    ) -> Result<Vec<Scope>> {
        if !path.exists() {
            self.limitations
                .insert(format!("module source is unavailable: {}", path.display()));
            return Ok(Vec::new());
        }
        let path = cargo::canonical(path)?;
        if !stack.insert(path.clone()) {
            self.limitations
                .insert(format!("module cycle: {}", path.display()));
            return Ok(Vec::new());
        }
        let source = fs::read_to_string(&path)?;
        let file = path.to_string_lossy().into_owned();
        let full_scope = Scope {
            file: file.clone(),
            start: 0,
            end: source.len(),
        };
        let mut subtree = vec![full_scope.clone()];
        let syntax = match syn::parse_file(&source) {
            Ok(syntax) => syntax,
            Err(error) => {
                self.limitations
                    .insert(format!("could not parse {file}: {error}"));
                stack.remove(&path);
                return Ok(Vec::new());
            }
        };
        let first = self.candidates.len();
        let mut visitor = SourceVisitor {
            inventory: self,
            package,
            file: &file,
            source: &source,
            root,
            scope: full_scope,
            conditional,
            module_dir: module_dir.into(),
            modules: Vec::new(),
        };
        visitor.visit_file(&syntax);
        let modules = visitor.modules;
        let last = self.candidates.len();
        for module in modules {
            let Some(module_path) = module.path else {
                self.limitations
                    .insert(format!("unresolved module {} in {file}", module.name));
                continue;
            };
            let child_dir = if module_path.file_name().is_some_and(|name| name == "mod.rs") {
                module_path
                    .parent()
                    .ok_or("module path has no parent")?
                    .to_path_buf()
            } else {
                module_path.with_extension("")
            };
            let children = self.file(
                package,
                &module_path,
                &child_dir,
                root,
                module.conditional,
                stack,
            )?;
            for candidate in &mut self.candidates[first..last] {
                if candidate
                    .scopes
                    .iter()
                    .any(|scope| scope.start <= module.start && scope.end >= module.end)
                {
                    candidate.scopes.extend(children.clone());
                }
            }
            subtree.extend(children);
        }
        stack.remove(&path);
        Ok(subtree)
    }

    fn manifest(&mut self, package: &Package, workspace: &Path) -> Result<()> {
        let source = fs::read_to_string(&package.manifest_path)?;
        let manifest: toml::Value = toml::from_str(&source)?;
        let Some(mut lints) = manifest.get("lints").cloned() else {
            return Ok(());
        };
        let mut path = package.manifest_path.clone();
        let mut section_prefix = "lints";
        let mut source = source;
        if lints.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
            path = workspace.join("Cargo.toml");
            source = fs::read_to_string(&path)?;
            let workspace_manifest: toml::Value = toml::from_str(&source)?;
            lints = workspace_manifest
                .get("workspace")
                .and_then(|w| w.get("lints"))
                .cloned()
                .ok_or("[lints] workspace=true without [workspace.lints]")?;
            section_prefix = "workspace.lints";
        }
        for tool in ["rust", "clippy"] {
            for (name, setting) in lints
                .get(tool)
                .and_then(toml::Value::as_table)
                .into_iter()
                .flatten()
            {
                let level = setting
                    .as_str()
                    .or_else(|| setting.get("level").and_then(toml::Value::as_str));
                if level != Some("allow") {
                    continue;
                }
                let section = format!("{section_prefix}.{tool}");
                let location = manifest_line(&source, &section, name);
                if location.is_none() {
                    self.limitations.insert(format!(
                        "manifest location not resolved: {} {section}.{name}",
                        path.display()
                    ));
                }
                let (line, text) = location.unwrap_or((1, format!("{section}.{name} = {setting}")));
                self.candidates.push(Candidate {
                    package_id: package.id.clone(),
                    package: package.name.clone(),
                    file: cargo::canonical(&path)?.to_string_lossy().into_owned(),
                    line,
                    column: 1,
                    kind: "manifest_allow".into(),
                    lint: normalize(&if tool == "rust" {
                        name.clone()
                    } else {
                        format!("{tool}::{name}")
                    }),
                    reason: None,
                    source: text,
                    conditional: false,
                    scopes: Vec::new(),
                    target_roots: package
                        .targets
                        .iter()
                        .map(|t| t.src_path.to_string_lossy().into_owned())
                        .collect(),
                });
            }
        }
        Ok(())
    }
}

fn manifest_line(source: &str, section: &str, name: &str) -> Option<(usize, String)> {
    let mut current = "";
    for (line, text) in source.lines().enumerate() {
        let text = text.trim();
        if text.starts_with('[') {
            current = text.trim_start_matches('[').trim_end_matches(']');
        }
        if current == section
            && text
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().trim_matches('"').trim_matches('\'') == name)
        {
            return Some((line + 1, text.into()));
        }
        if current == format!("{section}.{name}") && text.starts_with("level") {
            return Some((line + 1, text.into()));
        }
    }
    None
}

struct Module {
    name: String,
    path: Option<PathBuf>,
    start: usize,
    end: usize,
    conditional: bool,
}

struct SourceVisitor<'a> {
    inventory: &'a mut Inventory,
    package: &'a Package,
    file: &'a str,
    source: &'a str,
    root: &'a str,
    scope: Scope,
    conditional: bool,
    module_dir: PathBuf,
    modules: Vec<Module>,
}

impl SourceVisitor<'_> {
    fn attribute(&mut self, meta: &Meta, attr: &Attribute, conditional: bool) -> Result<()> {
        if meta.path().is_ident("cfg_attr") {
            let Meta::List(list) = meta else {
                return Ok(());
            };
            let nested = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            for meta in nested.iter().skip(1) {
                self.attribute(meta, attr, true)?;
            }
        } else if meta.path().is_ident("allow") || meta.path().is_ident("expect") {
            let Meta::List(list) = meta else {
                return Ok(());
            };
            let nested = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            let reason = nested.iter().find_map(|meta| match meta {
                Meta::NameValue(value) if value.path.is_ident("reason") => match &value.value {
                    Expr::Lit(expr) => match &expr.lit {
                        Lit::Str(reason) => Some(reason.value()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            });
            for meta in nested {
                if let Meta::Path(path) = meta {
                    let lint = path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    let span = attr.span();
                    self.inventory.candidates.push(Candidate {
                        package_id: self.package.id.clone(),
                        package: self.package.name.clone(),
                        file: self.file.into(),
                        line: span.start().line,
                        column: span.start().column + 1,
                        kind: if list.path.is_ident("expect") {
                            "expect"
                        } else {
                            "allow"
                        }
                        .into(),
                        lint: normalize(&lint),
                        reason: reason.clone(),
                        source: self
                            .source
                            .get(span.byte_range())
                            .unwrap_or("<attribute>")
                            .into(),
                        conditional: conditional || self.conditional,
                        scopes: vec![self.scope.clone()],
                        target_roots: BTreeSet::from([self.root.into()]),
                    });
                }
            }
        }
        Ok(())
    }

    fn in_scope(&mut self, span: Span, attrs: &[Attribute], visit: impl FnOnce(&mut Self)) {
        let range = span.byte_range();
        let previous = std::mem::replace(
            &mut self.scope,
            Scope {
                file: self.file.into(),
                start: range.start,
                end: range.end,
            },
        );
        let conditional = self.conditional;
        self.conditional |= attrs
            .iter()
            .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"));
        let first = self.inventory.candidates.len();
        visit(self);
        if self.conditional {
            for candidate in &mut self.inventory.candidates[first..] {
                candidate.conditional = true;
            }
        }
        self.scope = previous;
        self.conditional = conditional;
    }
}

impl<'ast> Visit<'ast> for SourceVisitor<'_> {
    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        self.conditional |= attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr");
        if let Err(error) = self.attribute(&attr.meta, attr, false) {
            self.inventory.limitations.insert(format!(
                "unsupported attribute at {}:{}: {error}",
                self.file,
                attr.span().start().line
            ));
        }
    }

    fn visit_file(&mut self, file: &'ast syn::File) {
        self.conditional |= file
            .attrs
            .iter()
            .any(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"));
        visit::visit_file(self, file);
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs = match item {
            syn::Item::Const(x) => &x.attrs,
            syn::Item::Enum(x) => &x.attrs,
            syn::Item::ExternCrate(x) => &x.attrs,
            syn::Item::Fn(x) => &x.attrs,
            syn::Item::ForeignMod(x) => &x.attrs,
            syn::Item::Impl(x) => &x.attrs,
            syn::Item::Macro(x) => &x.attrs,
            syn::Item::Mod(x) => &x.attrs,
            syn::Item::Static(x) => &x.attrs,
            syn::Item::Struct(x) => &x.attrs,
            syn::Item::Trait(x) => &x.attrs,
            syn::Item::TraitAlias(x) => &x.attrs,
            syn::Item::Type(x) => &x.attrs,
            syn::Item::Union(x) => &x.attrs,
            syn::Item::Use(x) => &x.attrs,
            _ => return,
        };
        self.in_scope(item.span(), attrs, |visitor| {
            visit::visit_item(visitor, item)
        });
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        let conditional = self.conditional;
        self.conditional |= module
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"));
        if module.content.is_none() {
            let name = module.ident.to_string();
            let explicit = module.attrs.iter().find_map(|attr| match &attr.meta {
                Meta::NameValue(value) if value.path.is_ident("path") => match &value.value {
                    Expr::Lit(expr) => match &expr.lit {
                        Lit::Str(path) => Some(path.value()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            });
            let path = if let Some(path) = explicit {
                Some(self.module_dir.join(path))
            } else {
                let flat = self.module_dir.join(format!("{name}.rs"));
                let nested = self.module_dir.join(&name).join("mod.rs");
                match (flat.exists(), nested.exists()) {
                    (true, false) => Some(flat),
                    (false, true) => Some(nested),
                    _ => None,
                }
            };
            let range = module.span().byte_range();
            self.modules.push(Module {
                name,
                path,
                start: range.start,
                end: range.end,
                conditional: self.conditional,
            });
            visit::visit_item_mod(self, module);
        } else {
            let previous = self.module_dir.clone();
            self.module_dir.push(module.ident.to_string());
            visit::visit_item_mod(self, module);
            self.module_dir = previous;
        }
        self.conditional = conditional;
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        self.in_scope(local.span(), &local.attrs, |visitor| {
            visit::visit_local(visitor, local)
        });
    }

    fn visit_field(&mut self, field: &'ast syn::Field) {
        self.in_scope(field.span(), &field.attrs, |visitor| {
            visit::visit_field(visitor, field)
        });
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        self.in_scope(arm.span(), &arm.attrs, |visitor| {
            visit::visit_arm(visitor, arm)
        });
    }

    fn visit_variant(&mut self, variant: &'ast syn::Variant) {
        self.in_scope(variant.span(), &variant.attrs, |visitor| {
            visit::visit_variant(visitor, variant)
        });
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attrs = match item {
            syn::ImplItem::Const(x) => &x.attrs,
            syn::ImplItem::Fn(x) => &x.attrs,
            syn::ImplItem::Type(x) => &x.attrs,
            syn::ImplItem::Macro(x) => &x.attrs,
            _ => return,
        };
        self.in_scope(item.span(), attrs, |visitor| {
            visit::visit_impl_item(visitor, item)
        });
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        let attrs = match item {
            syn::TraitItem::Const(x) => &x.attrs,
            syn::TraitItem::Fn(x) => &x.attrs,
            syn::TraitItem::Type(x) => &x.attrs,
            syn::TraitItem::Macro(x) => &x.attrs,
            _ => return,
        };
        self.in_scope(item.span(), attrs, |visitor| {
            visit::visit_trait_item(visitor, item)
        });
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        self.in_scope(expr.span(), &[], |visitor| visit::visit_expr(visitor, expr));
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.inventory.limitations.insert(format!(
            "macro tokens are not expanded: {}:{} ({})",
            self.file,
            node.span().start().line,
            node.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventories_real_attributes_without_reading_comments_or_macro_tokens() {
        let source = "#![allow(dead_code)]\n// #[allow(unused)]\n#[allow(unused_variables, reason = \"compat\")] fn f() {}\n#[cfg_attr(feature = \"extra\", expect(clippy::needless_return))] fn g() {}\nmacro_rules! generated { () => { #[allow(unused_mut)] fn h() {} } }";
        let package = Package {
            id: "p".into(),
            name: "p".into(),
            manifest_path: "Cargo.toml".into(),
            targets: vec![],
        };
        let mut inventory = Inventory::default();
        let mut visitor = SourceVisitor {
            inventory: &mut inventory,
            package: &package,
            file: "lib.rs",
            source,
            root: "lib.rs",
            scope: Scope {
                file: "lib.rs".into(),
                start: 0,
                end: source.len(),
            },
            conditional: false,
            module_dir: PathBuf::new(),
            modules: vec![],
        };
        visitor.visit_file(&syn::parse_file(source).unwrap());
        assert_eq!(inventory.candidates.len(), 3);
        assert_eq!(inventory.candidates[1].reason.as_deref(), Some("compat"));
        assert_eq!(
            inventory.candidates[1].source,
            "#[allow(unused_variables, reason = \"compat\")]"
        );
        assert!(inventory.candidates[2].conditional);
        assert_eq!(inventory.candidates[2].kind, "expect");
        assert_eq!(inventory.candidates[0].scopes[0].end, source.len());
    }
}
