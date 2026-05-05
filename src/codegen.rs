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
    let body = emit(expr, &[]);
    match mode {
        Mode::Read  => format!("(($s) => ({}))", body),
        Mode::Write => format!("(($s, __v) => {{ {} }})", body),
    }
}

#[derive(Copy, Clone)]
pub enum Mode { Read, Write }

/// Identifier scope at this emit site. Identifiers found in `scope` resolve
/// as locals (e.g. arrow function parameters); everything else resolves
/// through the state proxy as `$s.<name>`. Mirrors the runtime `with(state)`
/// semantics where local arrow-param bindings shadow state properties.
fn emit(e: &Expr, scope: &[String]) -> String {
    match e {
        Expr::Number(n)    => format_number(*n),
        Expr::Str(s)       => js_string(s),
        Expr::Template(s)  => format!("`{}`", s),
        Expr::Bool(b)      => if *b { "true".into() } else { "false".into() },
        Expr::Null         => "null".into(),
        Expr::Undefined    => "undefined".into(),

        Expr::Ident(name) => emit_ident_read(name, scope),

        Expr::Member { object, property, optional } => {
            let obj = emit(object, scope);
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
            format!("{}[{}]", emit(object, scope), emit(index, scope))
        }

        Expr::Call { callee, args } => {
            // Detect synthetic ?.call() shape — see parser
            if let Expr::Member { property, optional, object } = callee.as_ref() {
                if *optional && property == "__optional_call__" {
                    let obj = emit(object, scope);
                    let arg_src = args.iter().map(|a| emit(a, scope)).collect::<Vec<_>>().join(", ");
                    return format!("(({} == null) ? undefined : {}({}))", obj, obj, arg_src);
                }
            }
            let arg_src = args.iter().map(|a| emit(a, scope)).collect::<Vec<_>>().join(", ");
            format!("{}({})", emit(callee, scope), arg_src)
        }

        Expr::Neg(e) => format!("(-{})", emit(e, scope)),
        Expr::Pos(e) => format!("(+{})", emit(e, scope)),
        Expr::Not(e) => format!("(!{})", emit(e, scope)),

        Expr::Binary { op, left, right } => {
            let op_str = match op {
                BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*",
                BinOp::Div => "/", BinOp::Mod => "%",
                BinOp::Lt => "<", BinOp::Le => "<=", BinOp::Gt => ">", BinOp::Ge => ">=",
                BinOp::Eq | BinOp::StrictEq => "===",
                BinOp::NotEq | BinOp::StrictNotEq => "!==",
            };
            format!("({} {} {})", emit(left, scope), op_str, emit(right, scope))
        }

        Expr::And(l, r)      => format!("({} && {})", emit(l, scope), emit(r, scope)),
        Expr::Or(l, r)       => format!("({} || {})", emit(l, scope), emit(r, scope)),
        Expr::Coalesce(l, r) => format!("({} ?? {})", emit(l, scope), emit(r, scope)),

        Expr::Ternary { cond, then_branch, else_branch } => {
            format!("({} ? {} : {})", emit(cond, scope), emit(then_branch, scope), emit(else_branch, scope))
        }

        Expr::Assign { target, op, value } => emit_assign(target, *op, value, scope),

        Expr::UpdatePrefix  { target, increment } => emit_update(target, *increment, true, scope),
        Expr::UpdatePostfix { target, increment } => emit_update(target, *increment, false, scope),

        Expr::Object(props) => {
            let parts: Vec<String> = props.iter().map(|p| match p {
                ObjectProp::KeyValue(k, v) => format!("{}: {}", quote_key(k), emit(v, scope)),
                ObjectProp::Computed(k, v) => format!("[{}]: {}", emit(k, scope), emit(v, scope)),
                ObjectProp::Shorthand(name) => format!("{}: {}", name, emit_ident_read(name, scope)),
                ObjectProp::Spread(e)      => format!("...{}", emit(e, scope)),
            }).collect();
            format!("({{ {} }})", parts.join(", "))
        }

        Expr::Array(items) => {
            let parts: Vec<String> = items.iter().map(|i| emit(i, scope)).collect();
            format!("[{}]", parts.join(", "))
        }

        Expr::Sequence(items) => {
            let parts: Vec<String> = items.iter().map(|i| emit(i, scope)).collect();
            format!("({})", parts.join(", "))
        }

        Expr::Arrow { params, body } => {
            // Extend scope with the arrow params so identifier reads inside
            // the body resolve to the local binding instead of `$s.<name>`.
            // This matches `with(state)` runtime semantics where the inner
            // function's parameter shadows any same-named state key.
            let mut child_scope: Vec<String> = scope.to_vec();
            for p in params { child_scope.push(p.clone()); }
            let params_src = params.join(", ");
            let body_src = emit(body, &child_scope);
            if params.len() == 1 {
                format!("({} => {})", params_src, body_src)
            } else {
                format!("(({}) => {})", params_src, body_src)
            }
        }
    }
}

fn emit_ident_read(name: &str, scope: &[String]) -> String {
    // Local binding (arrow param) → emit as-is. Otherwise route through the
    // state proxy so unknown names produce undefined instead of ReferenceError,
    // matching the pre-precompiler `with(state)` behavior.
    if scope.iter().any(|s| s == name) {
        name.to_string()
    } else {
        format!("$s.{}", name)
    }
}

fn emit_assign(target: &Expr, op: AssignOp, value: &Expr, scope: &[String]) -> String {
    let value_src = emit(value, scope);
    let lhs = emit(target, scope);
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

fn emit_update(target: &Expr, increment: bool, prefix: bool, scope: &[String]) -> String {
    let lhs = emit(target, scope);
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
