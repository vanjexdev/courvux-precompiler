// Round-trip smoke test: load the wasm-pack Node build, compile a handful of
// expressions, evaluate the resulting arrow functions against a mock state
// proxy, and assert the value matches what the runtime would have produced
// pre-precompiler.

import { compile, version } from '../pkg/courvux_precompiler.js';

const failures = [];
const ok = (label, got, expected) => {
    if (Object.is(got, expected) || JSON.stringify(got) === JSON.stringify(expected)) {
        console.log(`  ✅ ${label}`);
    } else {
        console.log(`  ❌ ${label}`);
        console.log(`     expected: ${JSON.stringify(expected)}`);
        console.log(`     got:      ${JSON.stringify(got)}`);
        failures.push(label);
    }
};

const run = (src, state) => {
    const out = compile(src);
    if (typeof out === 'string' && out.startsWith('{"__compileError":true,')) {
        const err = JSON.parse(out);
        throw new Error(`compile error in \`${src}\`: ${err.error} (pos ${err.pos})`);
    }
    // The compiled output is a JS arrow function source; evaluate via Function
    // so we exercise the actual emitted code, not a re-parse.
    // eslint-disable-next-line no-new-func
    const fn = new Function('return ' + out)();
    return fn(state);
};

console.log(`courvux-precompiler v${version()}`);
console.log('Round-trip smoke (compiler output → evaluated against mock state):');

const state = {
    count: 5,
    name: 'Ada',
    user: { profile: { name: 'Grace' } },
    cards: [{ id: 1 }, { id: 2 }, { id: 3 }],
    draft: { backlog: '', progress: 'wip' },
    col: { key: 'progress' },
    flag: false,
    save() { return 'saved-' + this.count; },
};

ok('literal number',         run('42', state),                       42);
ok('literal string',         run("'hello'", state),                  'hello');
ok('identifier read',        run('count', state),                    5);
ok('member chain',           run('user.profile.name', state),        'Grace');
ok('bracket dynamic key',    run('draft[col.key]', state),           'wip');
ok('arithmetic',             run('count + 1', state),                6);
ok('comparison',             run('count > 0', state),                true);
ok('ternary',                run("count > 0 ? 'on' : 'off'", state), 'on');
ok('logical and',            run('flag && count', state),            false);
ok('logical or',             run('flag || count', state),            5);
ok('coalesce',               run('flag ?? count', state),            false);  // false is not nullish
ok('coalesce nullish',       run('missing ?? count', state),         5);
ok('method call',            run('save()', state),                   'saved-5');
ok('object literal',         run("{ active: count > 0, big: count }", state), { active: true, big: 5 });
ok('array literal',          run('[1, 2, count]', state),            [1, 2, 5]);
ok('postfix increment',      (() => { const s = { ...state }; const before = run('count++', s); return [before, s.count]; })(), [5, 6]);
ok('assign',                 (() => { const s = { ...state }; run('count = 99', s); return s.count; })(), 99);
ok('strict equality',        run("name === 'Ada'", state),           true);
ok('optional chaining',      run('user?.profile?.name', state),      'Grace');
ok('optional chaining null', run('missing?.foo?.bar', state),        undefined);

// Error path: a syntax error returns a tagged JSON envelope, not a throw.
const errOut = compile('count >');
if (typeof errOut !== 'string' || !errOut.startsWith('{"__compileError":true,')) {
    failures.push('expected compile error envelope for `count >`');
} else {
    console.log('  ✅ syntax error envelope');
}

console.log('');
if (failures.length) {
    console.log(`── ${failures.length} failure(s) ──`);
    process.exit(1);
} else {
    console.log('── all smoke tests pass ──');
}
