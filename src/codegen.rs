//! AST → JS arrow-function source string.
//!
//! The runtime contract: every emitted function takes a single `$s` argument
//! (the component state proxy) and returns the expression's value. Bare
//! identifiers resolve through `$s` so unknown names produce `undefined`
//! instead of `ReferenceError`, matching `with(state)` semantics.
//!
//! For assignment / increment expressions, the function reads + writes
//! through the proxy to fire the same notify traps as direct mutation.

use crate::parser::{AssignOp, BinOp, Expr, ObjectProp};

/// Compile a single expression to a JS arrow function source.
///
/// `mode` controls the function shape:
/// - `Read`  → `(($s) => (<expr>))` — for `{{ ... }}`, `:attr`, conditions, etc.
/// - `Write` → `(($s, __v) => { <expr> })` — currently unused (`setStateValue`
///   paths through `evaluate` already, just split via `splitLvalue`); kept for
///   symmetry / future writer-only paths.
pub fn compile(expr: &Expr, mode: Mode) -> String {
    let body = emit(expr);
    match mode {
        Mode::Read  => format!("(($s) => ({}))", body),
        Mode::Write => format!("(($s, __v) => {{ {} }})", body),
    }
}

#[derive(Copy, Clone)]
pub enum Mode { Read, Write }

fn emit(e: &Expr) -> String {
    match e {
        Expr::Number(n)    => format_number(*n),
        Expr::Str(s)       => js_string(s),
        Expr::Template(s)  => format!("`{}`", s),
        Expr::Bool(b)      => if *b { "true".into() } else { "false".into() },
        Expr::Null         => "null".into(),
        Expr::Undefined    => "undefined".into(),

        Expr::Ident(name) => emit_ident_read(name),

        Expr::Member { object, property, optional } => {
            let obj = emit(object);
            if *optional {
                format!("({}?.{})", obj, property)
            } else {
                // Wrap object in parens defensively when it's not already a simple
                // member chain — most cases here already produce safe output but
                // this avoids precedence surprises with unary minus, etc.
                format!("{}.{}", obj, property)
            }
        }

        Expr::Index { object, index } => {
            format!("{}[{}]", emit(object), emit(index))
        }

        Expr::Call { callee, args } => {
            // Detect synthetic ?.call() shape — see parser
            if let Expr::Member { property, optional, object } = callee.as_ref() {
                if *optional && property == "__optional_call__" {
                    let obj = emit(object);
                    let arg_src = args.iter().map(emit).collect::<Vec<_>>().join(", ");
                    return format!("(({} == null) ? undefined : {}({}))", obj, obj, arg_src);
                }
            }
            let arg_src = args.iter().map(emit).collect::<Vec<_>>().join(", ");
            format!("{}({})", emit(callee), arg_src)
        }

        Expr::Neg(e) => format!("(-{})", emit(e)),
        Expr::Pos(e) => format!("(+{})", emit(e)),
        Expr::Not(e) => format!("(!{})", emit(e)),

        Expr::Binary { op, left, right } => {
            let op_str = match op {
                BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*",
                BinOp::Div => "/", BinOp::Mod => "%",
                BinOp::Lt => "<", BinOp::Le => "<=", BinOp::Gt => ">", BinOp::Ge => ">=",
                BinOp::Eq | BinOp::StrictEq => "===",
                BinOp::NotEq | BinOp::StrictNotEq => "!==",
            };
            format!("({} {} {})", emit(left), op_str, emit(right))
        }

        Expr::And(l, r)      => format!("({} && {})", emit(l), emit(r)),
        Expr::Or(l, r)       => format!("({} || {})", emit(l), emit(r)),
        Expr::Coalesce(l, r) => format!("({} ?? {})", emit(l), emit(r)),

        Expr::Ternary { cond, then_branch, else_branch } => {
            format!("({} ? {} : {})", emit(cond), emit(then_branch), emit(else_branch))
        }

        Expr::Assign { target, op, value } => emit_assign(target, *op, value),

        Expr::UpdatePrefix  { target, increment } => emit_update(target, *increment, true),
        Expr::UpdatePostfix { target, increment } => emit_update(target, *increment, false),

        Expr::Object(props) => {
            let parts: Vec<String> = props.iter().map(|p| match p {
                ObjectProp::KeyValue(k, v) => format!("{}: {}", quote_key(k), emit(v)),
                ObjectProp::Computed(k, v) => format!("[{}]: {}", emit(k), emit(v)),
                ObjectProp::Shorthand(name) => format!("{}: {}", name, emit_ident_read(name)),
                ObjectProp::Spread(e)      => format!("...{}", emit(e)),
            }).collect();
            format!("({{ {} }})", parts.join(", "))
        }

        Expr::Array(items) => {
            let parts: Vec<String> = items.iter().map(emit).collect();
            format!("[{}]", parts.join(", "))
        }

        Expr::Sequence(items) => {
            let parts: Vec<String> = items.iter().map(emit).collect();
            format!("({})", parts.join(", "))
        }
    }
}

fn emit_ident_read(name: &str) -> String {
    // Read identifiers through the state proxy. The runtime's proxy `has` is
    // catch-all so `$s.foo` returns undefined for unknown keys, matching the
    // pre-precompiler `with(state)` behavior.
    format!("$s.{}", name)
}

fn emit_assign(target: &Expr, op: AssignOp, value: &Expr) -> String {
    let value_src = emit(value);
    let lhs = emit(target);
    let op_str = match op {
        AssignOp::Set => "=",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
        AssignOp::Mul => "*=",
        AssignOp::Div => "/=",
        AssignOp::Mod => "%=",
    };
    format!("({} {} {})", lhs, op_str, value_src)
}

fn emit_update(target: &Expr, increment: bool, prefix: bool) -> String {
    let lhs = emit(target);
    let op = if increment { "++" } else { "--" };
    if prefix {
        format!("({}{})", op, lhs)
    } else {
        format!("({}{})", lhs, op)
    }
}

fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

fn js_string(s: &str) -> String {
    // Single-quoted JS string with the minimum escaping required.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn quote_key(k: &str) -> String {
    // Bare identifier keys can stay unquoted; everything else gets quoted.
    let is_ident = !k.is_empty()
        && k.chars().next().map(|c| c == '_' || c == '$' || c.is_ascii_alphabetic()).unwrap_or(false)
        && k.chars().all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric());
    if is_ident { k.to_string() } else { js_string(k) }
}
