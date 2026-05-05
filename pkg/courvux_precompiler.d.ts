/* tslint:disable */
/* eslint-disable */

/**
 * WASM entry: returns compiled JS source on success, or a JS object with
 * `{ error: string, pos: number }` on failure. We do not throw across the
 * WASM boundary because hosts (Vite plugin, Node test harnesses) get cleaner
 * error reporting from a tagged result.
 */
export function compile(src: string): any;

/**
 * Returns the precompiler version (matches `Cargo.toml`). Useful for the
 * Vite plugin to log on startup and for test harnesses to gate behavior.
 */
export function version(): string;
