# courvux-precompiler

Build-time expression compiler for the [Courvux](https://github.com/vanjexdev/courvux) reactive UI framework.

Compiles template expressions (`{{ count + 1 }}`, `:class="{ active: isOn }"`, `@click="save(id)"`, `cv-model="form.name"`, …) to JavaScript arrow functions at build time, so the runtime never has to call `new Function()` and apps can ship with `Content-Security-Policy: script-src 'self'` (no `unsafe-eval`).

The compiler is written in Rust and ships as both a Rust crate (`courvux-precompiler`) and a WebAssembly module loadable from Node and the browser. App developers do **not** need a Rust toolchain — the `.wasm` binary is published with the npm package.

## Status

`0.1.0` — initial scaffold. Used as a build dependency by the `courvux/plugin/vite-plugin-courvux-precompile.js` Vite plugin in the main framework repo.

## Why a precompiler?

Pre-precompiler, Courvux compiled every template expression at runtime via `new Function('with(state){ return (expr) }')`. That works in every browser and stays under ~22 KB gzipped, but it requires the page's CSP to allow `script-src 'unsafe-eval'`. Apps that want a strict CSP — desktop shells via Tauri/Electron, anything that runs alongside untrusted third-party code, anything graded by automated security tooling — were stuck.

This crate moves the expression compilation to build time. The Vite plugin walks every template, extracts every expression, asks the Wasm compiler to turn each one into a JS arrow function, and emits a per-template registry the runtime consults before falling back to `new Function`. Apps that go through the build step ship without `unsafe-eval`. Apps that drop Courvux into a `<script>` tag with an importmap keep the runtime fallback unchanged.

## Supported expression subset

Anything you can sensibly write inside a Courvux template attribute:

- Literals: numbers, strings (`'`, `"`), template literals, `true`, `false`, `null`, `undefined`
- Identifiers and dot / bracket access, including optional chaining `?.`
- Function calls
- Arithmetic: `+ - * / %`
- Comparison: `< <= > >= == != === !==` (template `==` and `!=` are treated as `===` / `!==` to match Courvux runtime semantics)
- Logical: `&& || ??`
- Ternary
- Assignment: `= += -= *= /= %=`
- Pre/post increment / decrement: `++` / `--`
- Object literals (with shorthand, computed keys, spread)
- Array literals
- Comma sequences (for multi-statement event handlers)

Out of scope (rejected at parse time so the build fails loud):

- `function` / `class` declarations
- `await`, `yield`, generator / async syntax
- Regex literals
- Destructuring assignment

## Build

```bash
cargo test                              # Rust unit tests
wasm-pack build --target nodejs --release   # for Node-side use (Vite plugin)
wasm-pack build --target web    --release   # for browser-side use (rare)
```

## Use from Rust

```rust
use courvux_precompiler::compile_expression;

let js = compile_expression("count > 0 ? 'on' : 'off'").unwrap();
assert_eq!(js, "(($s) => ((($s.count > 0) ? 'on' : 'off')))");
```

## Use from Node (after `wasm-pack build --target nodejs`)

```js
import { compile, version } from 'courvux-precompiler';

console.log(version());                       // '0.1.0'
const js = compile('count > 0 ? "on" : "off"');
console.log(js);                              // (($s) => ((($s.count > 0) ? 'on' : 'off')))

// Errors come back as a JSON-tagged string with __compileError flag:
const result = compile('count >');
const parsed = result.startsWith('{"__compileError":true,') ? JSON.parse(result) : null;
if (parsed) {
    console.error(`Compile failed at offset ${parsed.pos}: ${parsed.error}`);
}
```

## License

MIT — see [LICENSE](./LICENSE).
