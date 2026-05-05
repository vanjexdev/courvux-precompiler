//! courvux-precompiler — build-time expression compiler for the Courvux
//! reactive UI framework.
//!
//! Public API (Rust): `compile_expression(src) -> Result<String, CompileError>`.
//! Public API (WASM): `compile(src: &str) -> JsValue` — returns either the
//! compiled JS arrow-function source string or an error object
//! `{ error: string, pos: number }`.

mod lexer;
mod parser;
mod codegen;

use codegen::{compile, Mode};
use parser::{parse, ParseError};
use wasm_bindgen::prelude::*;

#[derive(Debug)]
pub struct CompileError {
    pub message: String,
    pub pos: usize,
}

impl From<ParseError> for CompileError {
    fn from(e: ParseError) -> Self {
        Self { message: e.message, pos: e.pos }
    }
}

/// Compile a single template expression to a JS arrow function source.
///
/// Output shape: `(($s) => (<expr>))` where `$s` is the runtime state proxy.
pub fn compile_expression(src: &str) -> Result<String, CompileError> {
    let ast = parse(src)?;
    Ok(compile(&ast, Mode::Read))
}

// ── WASM bindings ────────────────────────────────────────────────────────────

/// WASM entry: returns compiled JS source on success, or a JS object with
/// `{ error: string, pos: number }` on failure. We do not throw across the
/// WASM boundary because hosts (Vite plugin, Node test harnesses) get cleaner
/// error reporting from a tagged result.
#[wasm_bindgen(js_name = compile)]
pub fn wasm_compile(src: &str) -> JsValue {
    match compile_expression(src) {
        Ok(js) => JsValue::from_str(&js),
        Err(err) => {
            // Build { error, pos } via JSON for zero-dep encoding.
            let escaped = err.message.replace('\\', "\\\\").replace('"', "\\\"");
            let json = format!(r#"{{"__compileError":true,"error":"{}","pos":{}}}"#, escaped, err.pos);
            JsValue::from_str(&json)
        }
    }
}

/// Returns the precompiler version (matches `Cargo.toml`). Useful for the
/// Vite plugin to log on startup and for test harnesses to gate behavior.
#[wasm_bindgen(js_name = version)]
pub fn wasm_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ── Re-exports for Rust consumers ────────────────────────────────────────────
pub use codegen::Mode as CodegenMode;
pub use parser::Expr;

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> String {
        compile_expression(src).expect(&format!("expected `{}` to compile", src))
    }

    #[test]
    fn literals() {
        assert_eq!(ok("1"),         "(($s) => (1))");
        assert_eq!(ok("1.5"),       "(($s) => (1.5))");
        assert_eq!(ok("'hi'"),      "(($s) => ('hi'))");
        assert_eq!(ok("true"),      "(($s) => (true))");
        assert_eq!(ok("null"),      "(($s) => (null))");
        assert_eq!(ok("undefined"), "(($s) => (undefined))");
    }

    #[test]
    fn identifier_reads_through_state() {
        assert_eq!(ok("count"),  "(($s) => ($s.count))");
        assert_eq!(ok("$store"), "(($s) => ($s.$store))");
    }

    #[test]
    fn member_and_index_chains() {
        assert_eq!(ok("user.name"),       "(($s) => ($s.user.name))");
        assert_eq!(ok("user.profile.bio"),"(($s) => ($s.user.profile.bio))");
        assert_eq!(ok("draft[col.key]"),  "(($s) => ($s.draft[$s.col.key]))");
        assert_eq!(ok("arr[0]"),          "(($s) => ($s.arr[0]))");
        assert_eq!(ok("$store.user?.id"), "(($s) => (($s.$store.user?.id)))");
    }

    #[test]
    fn arithmetic_and_comparison() {
        assert_eq!(ok("a + b"),      "(($s) => (($s.a + $s.b)))");
        assert_eq!(ok("count > 0"),  "(($s) => (($s.count > 0)))");
        assert_eq!(ok("a == b"),     "(($s) => (($s.a === $s.b)))");
        assert_eq!(ok("a != b"),     "(($s) => (($s.a !== $s.b)))");
    }

    #[test]
    fn ternary_and_logical() {
        assert_eq!(
            ok("count > 0 ? 'on' : 'off'"),
            "(($s) => ((($s.count > 0) ? 'on' : 'off')))"
        );
        assert_eq!(ok("a && b"),  "(($s) => (($s.a && $s.b)))");
        assert_eq!(ok("a || b"),  "(($s) => (($s.a || $s.b)))");
        assert_eq!(ok("a ?? b"),  "(($s) => (($s.a ?? $s.b)))");
    }

    #[test]
    fn calls_with_args() {
        assert_eq!(ok("save()"),               "(($s) => ($s.save()))");
        assert_eq!(ok("save(1, 2)"),           "(($s) => ($s.save(1, 2)))");
        assert_eq!(ok("toggle(item.id)"),      "(($s) => ($s.toggle($s.item.id)))");
        assert_eq!(ok("user.fullName()"),      "(($s) => ($s.user.fullName()))");
    }

    #[test]
    fn assignments() {
        assert_eq!(ok("count = 0"),     "(($s) => (($s.count = 0)))");
        assert_eq!(ok("flag = !flag"),  "(($s) => (($s.flag = (!$s.flag))))");
        assert_eq!(ok("count += 1"),    "(($s) => (($s.count += 1)))");
    }

    #[test]
    fn updates() {
        assert_eq!(ok("count++"), "(($s) => (($s.count++)))");
        assert_eq!(ok("--n"),     "(($s) => ((--$s.n)))");
    }

    #[test]
    fn object_and_array_literals() {
        assert_eq!(
            ok("{ active: count > 0, big }"),
            "(($s) => (({ active: ($s.count > 0), big: $s.big })))"
        );
        assert_eq!(ok("[1, 2, x]"), "(($s) => ([1, 2, $s.x]))");
    }

    #[test]
    fn sequence_for_handlers() {
        assert_eq!(ok("a = 1, b = 2"), "(($s) => ((($s.a = 1), ($s.b = 2))))");
    }

    #[test]
    fn parse_errors_have_messages() {
        let err = compile_expression("count >").unwrap_err();
        assert!(err.message.contains("unexpected"), "got: {}", err.message);

        let err = compile_expression("user..name").unwrap_err();
        assert!(err.message.contains("expected property name"), "got: {}", err.message);
    }

    #[test]
    fn unsupported_syntax_rejected_loudly() {
        assert!(compile_expression("function () {}").is_err());
        assert!(compile_expression("class X {}").is_err());
        assert!(compile_expression("/regex/").is_err());
    }
}
