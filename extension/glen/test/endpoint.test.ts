import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { createGlen, resolveGlenHome, isPlainToken, parseReplyArgs, RESERVED_OPS, sunPathMax, LIMITS } from "../index.ts";
import { fixture, request, client, frame } from "./fake-pi.ts";
test("AC306 managed registers nothing", async () => { const f = await fixture({}, { GLEN_MANAGED: "1" }); try { await f.start(); assert.equal(f.tools.length, 0); assert.equal(f.handlers.size, 0); assert.deepEqual(fs.readdirSync(f.home), []); } finally { await f.close(); } });
test("AC305/308 endpoint lifecycle, private registry and state", async () => {
    const f = await fixture(); try {
        const e = await f.start(); assert.ok(e, f.notes.join("\n")); const r = JSON.parse(fs.readFileSync(e.registry, "utf8"));
        for (const [file, mode] of [[e.registry, 0o600], [e.socket, 0o600], [path.dirname(e.socket), 0o700]] as const) assert.equal(fs.statSync(file).mode & 0o777, mode);
        assert.equal(r.session_id, f.fields.sessionId); assert.equal(r.mode, "tui"); assert.equal(r.protocol, 1); assert.equal(r.omp_version, "18.14.0"); assert.equal(r.cwd, f.ctx.cwd);
        const [hello] = await request(e.socket, frame("hello")); assert.deepEqual(hello.data.ops, ["hello", "state", "send"]); assert.equal(hello.data.state, "idle"); assert.equal(hello.data.active_requests, 0);
        f.fields.idle = false; f.fields.pending = true; const [state] = await request(e.socket, frame("state")); assert.equal(state.data.busy, true); assert.equal(state.data.queued, true); assert.equal(state.data.protocol, undefined);
        const old = fs.readFileSync(e.registry, "utf8"); f.ctx.agent.kind = "sub"; f.fields.sessionId = "sub"; await f.emit("session_start"); assert.equal(fs.readFileSync(e.registry, "utf8"), old); f.ctx.agent.kind = "main"; f.fields.sessionId = r.session_id;
        await f.emit("session_shutdown"); assert.ok(!fs.existsSync(e.socket)); assert.ok(!fs.existsSync(e.registry));
    } finally { await f.close(); }
});
test("AC305 exclusions and utf8 sun_path boundary", async () => {
    for (const opts of [{ mode: "rpc" }, { mode: "print" }, { agentKind: "sub" }, { sessionId: "../bad" }]) { const f = await fixture(opts); try { assert.equal(await f.start(), null); assert.equal(f.tools.length, 1); assert.deepEqual(fs.readdirSync(f.home), []); } finally { await f.close(); } }
    for (const home of ["relative", "/" + "x".repeat(200), "/" + "é".repeat(60)]) { const f = await fixture({}, { GLEN_HOME: home }); try { await f.start(); await f.start(); assert.equal(f.handle.endpoint(), null); assert.equal(f.notes.length, 2); } finally { await f.close(); } }
    assert.equal(resolveGlenHome({ GLEN_HOME: "~/g" }, "__dev", "/h"), "/h/g"); assert.throws(() => resolveGlenHome({ GLEN_HOME: "a" }, "", "/h")); assert.equal(sunPathMax("darwin"), 104); assert.ok(isPlainToken("abc-_1")); assert.ok(!isPlainToken("a/b"));
});
test("AC307 protocol table and idle bound", async () => {
    const f = await fixture(); try { const e = await f.start();
        for (const [input, code] of [[{ ...frame("hello"), v: 2 }, "unsupported_version"], [frame("wat"), "unsupported_op"], ["bad\n", "bad_request"], [JSON.stringify({ v: 1, op: "state" }) + "\n", "bad_request"], ["x".repeat(LIMITS.maxLine + 1) + "\n", "bad_request"]] as const) { const r = await request(e.socket, input); assert.equal(r.length, 1); assert.equal(r[0].error.code, code); }
        for (const op of RESERVED_OPS) { const [r] = await request(e.socket, frame(op)); assert.equal(r.error.code, "unsupported_op"); assert.deepEqual(r.error.details, { op, ops: ["hello", "state", "send"], reserved: true }); }
        const c = await client(e.socket); await c.until(() => c.ended, 500);
        const [r] = await request(e.socket, { ...frame("hello"), unknown: true }); assert.equal(r.ok, true);
    } finally { await f.close(); }
});
test("AC311 reply validation, nulls, permissions and write failures", async () => {
    for (const input of [{}, { text: 1 }, { text: " " }, { text: "é".repeat(32769) }]) assert.throws(() => parseReplyArgs(input), /text must be a non-empty string of at most 65536 bytes/);
    for (const final of [null, 1, "true"]) assert.throws(() => parseReplyArgs({ text: "ok", final }), /final must be a boolean/);
    const f = await fixture({ mode: "print" }); try {
        const tool = f.tools[0]; assert.equal(tool.loadMode, "essential"); assert.equal(tool.approval, "read"); assert.equal(tool.parameters.isStrict, true);
        const run = (args: any) => tool.execute("tool", args, null, null, f.ctx);
        assert.equal((await run({ text: "no", final: null })).isError, true); assert.deepEqual(fs.readdirSync(f.home), []);
        assert.equal((await run({ text: "hello" })).content[0].text, "delivered"); const file = path.join(f.home, "inbox", `${f.fields.sessionId}.ndjson`); const r = JSON.parse(fs.readFileSync(file, "utf8"));
        assert.deepEqual(Object.keys(r).sort(), ["v", "id", "session_id", "job_id", "request_id", "tool_call_id", "text", "final", "at"].sort()); assert.equal(r.job_id, null); assert.equal(r.request_id, null); assert.equal(r.tool_call_id, "tool"); assert.equal(fs.statSync(file).mode & 0o777, 0o600); assert.equal(fs.statSync(path.dirname(file)).mode & 0o777, 0o700);
        fs.rmSync(path.dirname(file), { recursive: true }); fs.writeFileSync(path.dirname(file), "blocked"); assert.equal((await run({ text: "oops" })).isError, true);
    } finally { await f.close(); }
});
test("AC313 stale sweep preserves live pids and unlinks own leftovers", async () => {
    const f = await fixture(); try { const dir = path.join(f.home, "run/live"); fs.mkdirSync(dir, { recursive: true });
        for (const [id, pid] of [["dead", 77], ["alive", 88]] as const) { fs.writeFileSync(path.join(dir, `${id}.json`), JSON.stringify({ pid })); fs.writeFileSync(path.join(dir, `${pid}.sock`), ""); }
        fs.writeFileSync(path.join(dir, `${f.deps.pid}.sock`), "stale"); f.deps.isAlive = (pid: number) => pid === 88 || pid === f.deps.pid;
        await f.start(); assert.ok(!fs.existsSync(path.join(dir, "dead.json"))); assert.ok(!fs.existsSync(path.join(dir, "77.sock"))); assert.ok(fs.existsSync(path.join(dir, "alive.json"))); assert.ok(fs.existsSync(path.join(dir, "88.sock")));
    } finally { await f.close(); }
});
test("duplicate module copies register nothing twice", async () => {
    const { createFakeHost } = await import("./fake-pi.ts");
    const first = createFakeHost({ mode: "print" }), second = createFakeHost({ mode: "print" });
    const original = await import("../index.ts"); original.default(first.pi);
    const copy = await import("../index.ts?duplicate-copy"); copy.default(second.pi);
    assert.equal(first.tools.length, 1); assert.equal(second.tools.length, 0); assert.equal(second.handlers.size, 0);
});
test("AC307 partial bytes do not extend the complete-line deadline", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n")); const c = await client(e.socket);
        c.socket.write('{"v":'); await new Promise(r => setTimeout(r, 50)); c.socket.write("1");
        await c.until(() => c.ended, 150); assert.deepEqual(c.frames, []);
    } finally { await f.close(); }
});
test("AC313 shutdown closes idle connections without waiting for their timeout", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const c = await client(e.socket); await f.emit("session_shutdown"); await c.until(() => c.ended, 500);
        assert.deepEqual(c.frames, []); assert.ok(!fs.existsSync(e.socket)); assert.ok(!fs.existsSync(e.registry));
    } finally { await f.close(); }
});
test("AC321 multibyte home at each platform byte limit is rejected before mkdir", async () => {
    for (const platform of ["darwin", "linux"]) {
        const f = await fixture(); try {
            f.deps.platform = platform;
            const limit = sunPathMax(platform), prefix = f.home + "/", tail = `/run/live/${f.deps.pid}.sock`;
            const remaining = limit - Buffer.byteLength(prefix + tail);
            const home = prefix + "é".repeat(Math.floor(remaining / 2)) + (remaining % 2 ? "a" : "");
            assert.equal(Buffer.byteLength(home + tail), limit);
            assert.ok((home + tail).length < limit);
            f.deps.env.GLEN_HOME = home;
            await f.start(); await f.start();
            assert.equal(f.handle.endpoint(), null); assert.equal(f.notes.length, 2);
            assert.match(f.notes[0], /GLEN_HOME is too long/); assert.ok(!fs.existsSync(home));
        } finally { await f.close(); }
    }
});
test("AC307 exact line limit is accepted and bytes after the first newline are ignored", async () => {
    const f = await fixture(); try {
        const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const line = JSON.stringify(frame("state"));
        const atLimit = line + " ".repeat(LIMITS.maxLine - Buffer.byteLength(line)) + "\n";
        const [response, ...extra] = await request(e.socket, atLimit);
        assert.equal(response.ok, true); assert.deepEqual(extra, []);
        const frames = await request(e.socket, line + "\n" + "x".repeat(LIMITS.maxLine + 1));
        assert.equal(frames.length, 1); assert.equal(frames[0].ok, true);
    } finally { await f.close(); }
});
test("omitted final is persisted as an explicit false", async () => {
    const f = await fixture({ mode: "print" });
    try {
        assert.equal((await f.tools[0].execute("default-final", { text: "progress" }, null, null, f.ctx)).isError, undefined);
        const record = JSON.parse(fs.readFileSync(path.join(f.home, "inbox", `${f.fields.sessionId}.ndjson`), "utf8"));
        assert.ok(Object.hasOwn(record, "final"));
        assert.equal(record.final, false);
    } finally { await f.close(); }
});
test("AC321 warns once on every failing session_start", async () => {
    const f = await fixture({}, { GLEN_HOME: "relative" });
    try {
        for (let i = 1; i <= 3; i++) {
            await f.emit("session_start"); assert.equal(f.notes.length, i);
            await f.emit("agent_start"); assert.equal(f.notes.length, i);
        }
    } finally { await f.close(); }
});
