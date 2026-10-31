// W8 audit fixes in the extension: SEC-3 (an untrusted GLEN_HOME gets no endpoint) and ERR-4
// (a torn inbox tail never swallows the next glen_reply record).
import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { appendLine, unsafeDir } from "../index.ts";
import { fixture } from "./fake-pi.ts";
function lines(file: string) { return fs.readFileSync(file, "utf8").split("\n"); }
test("ERR-4 appendLine terminates a torn tail and never adds blank lines", () => {
    const dir = fs.mkdtempSync("/tmp/glen-append-");
    try {
        const file = path.join(dir, "inbox.ndjson");
        appendLine(file, "{\"a\":1}");
        assert.equal(fs.readFileSync(file, "utf8"), "{\"a\":1}\n");
        fs.appendFileSync(file, "{\"v\":1,\"text\":\"cut");
        appendLine(file, "{\"b\":2}");
        assert.deepEqual(lines(file), ["{\"a\":1}", "{\"v\":1,\"text\":\"cut", "{\"b\":2}", ""]);
        appendLine(file, "{\"c\":3}");
        assert.ok(!fs.readFileSync(file, "utf8").includes("\n\n"));
        assert.equal(fs.statSync(file).mode & 0o777, 0o600);
    } finally { fs.rmSync(dir, { recursive: true, force: true }); }
});
test("ERR-4 glen_reply after a torn inbox tail writes a parseable record", async () => {
    const f = await fixture();
    try {
        await f.start();
        const file = path.join(f.home, "inbox", `${f.fields.sessionId}.ndjson`);
        fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
        fs.writeFileSync(file, "{\"v\":1,\"id\":\"torn\",\"text\":\"cu", { mode: 0o600 });
        const tool = f.tools.find((t: any) => t.name === "glen_reply");
        assert.equal((await tool.execute("call", { text: "after the tear" }, null, null, f.ctx)).content[0].text, "delivered");
        const all = lines(file);
        assert.equal(all.length, 3);
        assert.throws(() => JSON.parse(all[0]));
        assert.equal(JSON.parse(all[1]).text, "after the tear");
    } finally { await f.close(); }
});
test("SEC-3 unsafeDir accepts a private directory and names each violation", () => {
    const dir = fs.mkdtempSync("/tmp/glen-home-");
    try {
        const uid = process.getuid!();
        assert.equal(unsafeDir(dir, uid), null);
        assert.equal(unsafeDir(path.join(dir, "missing"), uid), null);
        fs.chmodSync(dir, 0o755);
        assert.equal(unsafeDir(dir, uid), null, "0755 has no group/other write bits");
        fs.chmodSync(dir, 0o775);
        assert.match(unsafeDir(dir, uid)!, /writable by group or other/);
        fs.chmodSync(dir, 0o700);
        assert.match(unsafeDir(dir, uid + 1)!, /owned by uid/);
        fs.symlinkSync(dir, path.join(dir, "link"));
        // LOW-008 (W8 review): a symlink is named as one, matching glen's Rust wording.
        assert.match(unsafeDir(path.join(dir, "link"), uid)!, /is a symlink/);
        fs.writeFileSync(path.join(dir, "file"), "");
        assert.match(unsafeDir(path.join(dir, "file"), uid)!, /not a directory/);
    } finally { fs.rmSync(dir, { recursive: true, force: true }); }
});
async function refused(prepare: (home: string, f: any) => void) {
    const f = await fixture();
    try {
        prepare(f.home, f);
        assert.equal(await f.start(), null);
        assert.equal(f.handle.endpoint(), null);
        assert.equal(f.notes.length, 1, JSON.stringify(f.notes));
        assert.match(f.notes[0], /refusing GLEN_HOME/);
        assert.match(f.notes[0], /live delivery is off/);
        const live = path.join(f.home, "run/live");
        const entries = fs.existsSync(live) && fs.lstatSync(live).isDirectory() ? fs.readdirSync(live) : [];
        assert.deepEqual(entries.filter(n => n.endsWith(".sock") || n.endsWith(".json")), []);
    } finally { quietChmod(f.home); await f.close(); }
}
function quietChmod(home: string) { try { fs.chmodSync(home, 0o700); } catch { /* already gone */ } }
test("SEC-3 a group/other-writable GLEN_HOME root gets no endpoint", async () => {
    await refused(home => fs.chmodSync(home, 0o777));
});
test("SEC-3 a writable run/live gets no endpoint", async () => {
    await refused(home => { fs.mkdirSync(path.join(home, "run/live"), { recursive: true }); fs.chmodSync(path.join(home, "run/live"), 0o777); });
});
test("SEC-3 a symlinked run/live gets no endpoint and nothing lands in its target", async () => {
    const target = fs.mkdtempSync("/tmp/glen-target-");
    try {
        await refused(home => { fs.mkdirSync(path.join(home, "run"), { mode: 0o700 }); fs.symlinkSync(target, path.join(home, "run/live")); });
        assert.deepEqual(fs.readdirSync(target), []);
    } finally { fs.rmSync(target, { recursive: true, force: true }); }
});
test("SEC-3 a GLEN_HOME owned by another uid gets no endpoint", async () => {
    await refused((_home, f) => { f.deps.uid = process.getuid!() + 1; });
});
test("SEC-3 a private 0755 GLEN_HOME still registers", async () => {
    const f = await fixture();
    try {
        fs.chmodSync(f.home, 0o755);
        const e = await f.start();
        assert.ok(e && fs.existsSync(e.socket) && fs.existsSync(e.registry));
        assert.deepEqual(f.notes, []);
    } finally { await f.close(); }
});
async function replyRefused(prepare: (home: string, f: any) => void) {
    const f = await fixture();
    try {
        prepare(f.home, f);
        const tool = f.tools.find((t: any) => t.name === "glen_reply");
        const out = await tool.execute("call", { text: "secret answer" }, null, null, f.ctx);
        assert.equal(out.isError, true);
        assert.match(out.content[0].text, /refusing GLEN_HOME/);
    } finally { quietChmod(f.home); await f.close(); }
}
test("SEC-3 glen_reply refuses an untrusted GLEN_HOME and creates no inbox", async () => {
    let home = "";
    await replyRefused(h => { home = h; fs.chmodSync(h, 0o777); });
    assert.equal(fs.existsSync(path.join(home, "inbox")), false);
});
test("SEC-3 glen_reply refuses a writable inbox", async () => {
    await replyRefused(home => { fs.mkdirSync(path.join(home, "inbox"), { mode: 0o700 }); fs.chmodSync(path.join(home, "inbox"), 0o777); });
});
test("SEC-3 glen_reply never writes through a symlinked inbox or inbox file", async () => {
    const target = fs.mkdtempSync("/tmp/glen-target-");
    try {
        await replyRefused(home => fs.symlinkSync(target, path.join(home, "inbox")));
        assert.deepEqual(fs.readdirSync(target), []);
        const victim = path.join(target, "victim");
        fs.writeFileSync(victim, "untouched");
        const f = await fixture();
        try {
            fs.mkdirSync(path.join(f.home, "inbox"), { mode: 0o700 });
            fs.symlinkSync(victim, path.join(f.home, "inbox", `${f.fields.sessionId}.ndjson`));
            const tool = f.tools.find((t: any) => t.name === "glen_reply");
            const out = await tool.execute("call", { text: "secret answer" }, null, null, f.ctx);
            assert.equal(out.isError, true);
            assert.equal(fs.readFileSync(victim, "utf8"), "untouched");
        } finally { await f.close(); }
    } finally { fs.rmSync(target, { recursive: true, force: true }); }
});
