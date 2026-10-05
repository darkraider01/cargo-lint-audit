#[allow(dead_code)]
fn active_allow() {}

#[allow(dead_code)]
fn used_allow() {}

#[allow(dead_code, reason = "compat")]
fn reason_allow() {}

#[expect(dead_code, reason = "expected compatibility stub")]
fn expected_unused() {}

#[expect(dead_code)]
fn expected_but_used() {}

#[allow(dead_code, unused_variables)]
fn multiple_lints() { let multiple = 1; }

#[allow(unused)]
fn grouped() { let mut grouped_binding = 1; }

mod inner {
    #![allow(dead_code)]
    fn inner_unused() {}
}

#[allow(dead_code)]
mod external;

#[cfg_attr(feature = "extra", allow(dead_code, reason = "feature-dependent"))]
fn conditional_allow() {}

#[allow(dead_code)]
#[cfg(feature = "extra")]
fn feature_only() {}

#[allow(clippy::needless_return)]
fn clippy_return() -> u8 { return 1; }

#[expect(clippy::needless_return)]
fn clippy_expected() -> u8 { return 2; }

#[allow(trivial_casts)]
fn default_allow() { let value: &u8 = &1; let _ = value as *const u8; }

fn unacknowledged_default() { let value: &u8 = &1; let _ = value as *const u8; }

#[allow(dead_code)]
mod nested {
    #[allow(dead_code)]
    fn doubly_allowed() {}
}

#[allow(lint_audit_nonexistent)]
fn unknown_allow() {}

#[allow(private_in_public)]
fn removed_allow() {}

#[allow(unused_tuple_struct_fields)]
struct Renamed(u8);

macro_rules! generated {
    () => { #[allow(dead_code)] fn macro_generated() {} }
}
generated!();

#[allow(dead_code, reason = "macro scope")]
mod macro_scope {
    macro_rules! make_function { () => { fn expanded_unused() {} } }
    make_function!();
}

fn main() {
    used_allow(); expected_but_used(); default_allow(); unacknowledged_default();
    let _ = (clippy_return(), clippy_expected(), dependency::answer(), beta::answer());
}
