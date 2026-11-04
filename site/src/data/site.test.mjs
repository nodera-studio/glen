import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { site } from './site.ts';

const root = new URL('../../../', import.meta.url);
const compat = JSON.parse(readFileSync(new URL('src/omp/compat.json', root), 'utf8'));

// Docs audit M2: the chips showed a version and an omp that no file kept in step.
test('the version chip is the crate version: a release PR bumps Cargo.toml and site.ts together', () => {
 const cargo = readFileSync(new URL('Cargo.toml', root), 'utf8').match(/^\[package\][^[]*?^version = "([^"]+)"/m)?.[1];
 assert.ok(cargo, 'Cargo.toml has a [package] version');
 assert.equal(site.version, cargo, `site/src/data/site.ts version ${site.version} must equal Cargo.toml's ${cargo}`);
});

test('the omp versions come from src/omp/compat.json', () => {
 assert.equal(site.testedOmp, compat.tested);
 const range = compat.min === compat.max_tested ? compat.min : `${compat.min} to ${compat.max_tested}`;
 assert.equal(site.supportedOmp, range);
});
