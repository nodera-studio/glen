import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { procPlace, removeRegistryIfUnchanged, scopeOf } from "../index.ts";
import { fixture } from "./fake-pi.ts";
test("stale cleanup verifies the renamed registry and restores a fresh publisher", () => {
    const home = fs.mkdtempSync("/tmp/glen-reg-");
    try {
        const file = `${home}/session.json`;
        const stale = JSON.stringify({ pid: 12, started_at: "old" });
        const fresh = JSON.stringify({ pid: 34, started_at: "new" });
        fs.writeFileSync(file, fresh);
        assert.equal(removeRegistryIfUnchanged(file, stale), false);
        assert.equal(fs.readFileSync(file, "utf8"), fresh);
        assert.deepEqual(fs.readdirSync(home), ["session.json"]);
        assert.equal(removeRegistryIfUnchanged(file, fresh), true);
        assert.equal(fs.existsSync(file), false);
    } finally { fs.rmSync(home, { recursive: true, force: true }); }
});
// S-04/S-10 (AC-H303): a fake procfs root stands in for /proc.
function fakeProc(pidns: number | null, bootId: string | null) {
    const root = fs.mkdtempSync("/tmp/glen-proc-");
    fs.mkdirSync(`${root}/self/ns`, { recursive: true }); fs.mkdirSync(`${root}/sys/kernel/random`, { recursive: true });
    if (pidns !== null) fs.symlinkSync(`pid:[${pidns}]`, `${root}/self/ns/pid`);
    if (bootId !== null) fs.writeFileSync(`${root}/sys/kernel/random/boot_id`, bootId + "\n");
    return root;
}
test("procPlace reads the pid namespace and boot id, and nothing without a procfs", () => {
    const root = fakeProc(4026531836, "boot-a");
    try {
        assert.deepEqual(procPlace(root), { pidns: 4026531836, bootId: "boot-a" });
        assert.deepEqual(procPlace(undefined), { pidns: null, bootId: null });
        assert.deepEqual(procPlace(`${root}/missing`), { pidns: null, bootId: null });
    } finally { fs.rmSync(root, { recursive: true, force: true }); }
});
test("scopeOf mirrors glen: another boot is dead, another or uncomparable namespace is elsewhere", () => {
    const me = { pidns: 100, bootId: "boot-a" }, blind = { pidns: null, bootId: null };
    const cases: [any, any, string][] = [
        [{ pid: 1 }, me, "here"],
        [{ pid: 1, pidns: 100, boot_id: "boot-a" }, me, "here"],
        [{ pid: 1, pidns: 200, boot_id: "boot-a" }, me, "elsewhere"],
        [{ pid: 1, pidns: 200 }, me, "elsewhere"],
        [{ pid: 1, pidns: 100, boot_id: "boot-b" }, me, "other_boot"],
        [{ pid: 1, pidns: 200, boot_id: "boot-b" }, me, "other_boot"],
        [{ pid: 1, pidns: "100" }, me, "elsewhere"],
        [{ pid: 1, boot_id: 7 }, me, "elsewhere"],
        [{ pid: 1, pidns: 100 }, blind, "elsewhere"],
        [{ pid: 1, boot_id: "boot-a" }, blind, "elsewhere"],
        [{ pid: 1 }, blind, "here"],
    ];
    for (const [r, place, want] of cases) assert.equal(scopeOf(r, place), want, JSON.stringify([r, place]));
});
test("AC-H303 the sweep never removes a registry recording another pid namespace", async () => {
    const proc = fakeProc(100, "boot-a"), f = await fixture();
    try {
        const dir = path.join(f.home, "run/live"); fs.mkdirSync(dir, { recursive: true });
        const regs: [string, any][] = [
            ["foreign", { pid: 77, pidns: 200, boot_id: "boot-a" }],
            ["unsure", { pid: 78, pidns: "x" }],
            ["dead", { pid: 79, pidns: 100, boot_id: "boot-a" }],
            ["legacy", { pid: 80 }],
            ["oldboot", { pid: 88, pidns: 100, boot_id: "boot-b" }],
        ];
        for (const [id, r] of regs) { fs.writeFileSync(path.join(dir, `${id}.json`), JSON.stringify(r)); fs.writeFileSync(path.join(dir, `${r.pid}.sock`), ""); }
        fs.writeFileSync(path.join(dir, "81.sock"), "");
        f.deps.proc = proc; f.deps.isAlive = (pid: number) => pid === 88 || pid === f.deps.pid;
        const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const exists = (name: string) => fs.existsSync(path.join(dir, name));
        for (const kept of ["foreign.json", "77.sock", "unsure.json", "78.sock", "88.sock"]) assert.ok(exists(kept), `${kept} kept`);
        for (const gone of ["dead.json", "79.sock", "legacy.json", "80.sock", "oldboot.json", "81.sock"]) assert.ok(!exists(gone), `${gone} removed`);
        const own = JSON.parse(fs.readFileSync(e.registry, "utf8"));
        assert.equal(own.pidns, 100); assert.equal(own.boot_id, "boot-a"); assert.equal(own.pid, f.deps.pid);
    } finally { await f.close(); fs.rmSync(proc, { recursive: true, force: true }); }
});
test("a registry write without a procfs records no place, and own cleanup skips another namespace's registry", async () => {
    const f = await fixture();
    try {
        const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const own = JSON.parse(fs.readFileSync(e.registry, "utf8"));
        assert.ok(!("pidns" in own) && !("boot_id" in own));
        // Another namespace's endpoint with our pid number published this session's registry.
        const other = JSON.stringify({ ...own, pidns: 999 });
        fs.writeFileSync(e.registry, other);
        await f.emit("session_shutdown");
        assert.equal(fs.readFileSync(e.registry, "utf8"), other);
    } finally { await f.close(); }
});
test("an other-boot registry never unlinks a socket while our own pid namespace is unknown", async () => {
    // A forged record whose pidns is null must not match an unreadable own namespace (null === null).
    const proc = fakeProc(null, "boot-a"), f = await fixture();
    try {
        const dir = path.join(f.home, "run/live"); fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(path.join(dir, "forged.json"), JSON.stringify({ pid: 92, pidns: null, boot_id: "boot-b" }));
        fs.writeFileSync(path.join(dir, "92.sock"), "");
        f.deps.proc = proc; f.deps.isAlive = (pid: number) => pid === f.deps.pid;
        const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        assert.ok(fs.existsSync(path.join(dir, "92.sock")), "an unknown own namespace never proves the pid is numbered here");
    } finally { await f.close(); fs.rmSync(proc, { recursive: true, force: true }); }
});
test("MEDIUM-001 an other-boot registry of another pid namespace keeps the socket its pid names", async () => {
    const proc = fakeProc(100, "boot-a"), f = await fixture();
    try {
        const dir = path.join(f.home, "run/live"); fs.mkdirSync(dir, { recursive: true });
        const regs: [string, any][] = [
            ["foreignboot", { pid: 90, pidns: 200, boot_id: "boot-b" }],
            ["hereboot", { pid: 91, pidns: 100, boot_id: "boot-b" }],
        ];
        for (const [id, r] of regs) { fs.writeFileSync(path.join(dir, `${id}.json`), JSON.stringify(r)); fs.writeFileSync(path.join(dir, `${r.pid}.sock`), ""); }
        f.deps.proc = proc; f.deps.isAlive = (pid: number) => pid === f.deps.pid;
        const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const exists = (name: string) => fs.existsSync(path.join(dir, name));
        assert.ok(!exists("foreignboot.json") && !exists("hereboot.json"), "both stale registries go");
        assert.ok(exists("90.sock"), "a pid numbered in another namespace keeps its socket");
        assert.ok(!exists("91.sock"), "this namespace's dead pid loses its socket");
    } finally { await f.close(); fs.rmSync(proc, { recursive: true, force: true }); }
});
