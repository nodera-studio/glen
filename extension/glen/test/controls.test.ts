import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import { thinkingLevelsOf, modelRow, OPS } from "../index.ts";
import { fixture, request, client, frame } from "./fake-pi.ts";
const args: Record<string, any> = { abort: {}, list_models: {}, set_model: { provider: "fake", model_id: "fake-2" }, set_thinking_level: { level: "high" }, compact: { instructions: "keep facts" } };
async function run(f: any, socket: string, op: string, a: any = args[op]) {
    const frames = await request(socket, frame(op, a));
    assert.equal(frames.length, 1); assert.equal(frames[0].id, "r");
    if (frames[0].ok) assert.equal(frames[0].data.session_id, f.fields.sessionId);
    return frames[0];
}
test("AC366 normalized rows and supported thinking levels", () => {
    assert.deepEqual(thinkingLevelsOf(null), ["off"]);
    assert.deepEqual(thinkingLevelsOf({ reasoning: false, thinking: { efforts: ["high"] } }), ["off"]);
    assert.deepEqual(modelRow({ provider: "p", id: "m", name: "M", reasoning: true, thinking: { efforts: ["high"] } }), { provider: "p", id: "m", name: "M", reasoning: true, thinking_levels: ["off", "high"], context_window: null, max_tokens: null });
    assert.deepEqual(OPS, ["hello", "state", "send", ...Object.keys(args)]);
});
test("AC366 every control rejects session mismatch and bad args without side effects", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        for (const op of Object.keys(args)) {
            const r = await run(f, e.socket, op, { ...args[op], expect_session_id: "other" });
            assert.equal(r.error.code, "session_mismatch"); assert.equal(r.error.details.session_id, f.fields.sessionId);
            assert.equal((await run(f, e.socket, op, { ...args[op], expect_session_id: 1 })).error.code, "bad_request");
            assert.equal((await run(f, e.socket, op, [])).error.code, "bad_request");
        }
        assert.deepEqual(f.calls, { abort: 0, setModel: [], thinking: [], compact: [] });
        assert.equal((await run(f, e.socket, "set_model", { provider: " ", model_id: "a" })).error.code, "bad_request");
        assert.equal((await run(f, e.socket, "set_thinking_level", { level: "auto" })).error.code, "bad_request");
        assert.equal((await run(f, e.socket, "compact", { instructions: "é".repeat(262145) })).error.code, "bad_request");
    } finally { await f.close(); }
});
test("AC356 abort idle, settled, and still pending", async () => {
    for (const opts of [{}, { idle: false, abortDelay: 70 }, { idle: false, abortStuck: true }]) {
        const f = await fixture(opts); f.deps.abortWaitMs = 100;
        try { const e = await f.start(); assert.ok(e, f.notes.join("\n")); const r = await run(f, e.socket, "abort");
            assert.equal(r.ok, true); assert.equal(r.data.aborted, opts.idle === false); assert.equal(r.data.idle_after, !opts.abortStuck);
            assert.equal(f.calls.abort, opts.idle === false ? 1 : 0); assert.equal(f.notes.length, opts.idle === false ? 1 : 0);
        } finally { await f.close(); }
    }
});
test("AC366 list, switch, thinking, compaction happy paths and UI", async () => {
    for (const hasUI of [true, false]) {
        const f = await fixture({ hasUI }); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
            const listed = await run(f, e.socket, "list_models"); assert.equal(listed.data.models.length, 6); assert.equal(listed.data.thinking_level, "medium"); assert.equal(f.notes.length, 0);
            const thinking = await run(f, e.socket, "set_thinking_level"); assert.equal(thinking.data.previous, "medium"); assert.equal(thinking.data.thinking_level, "high");
            const switched = await run(f, e.socket, "set_model"); assert.equal(switched.data.previous.id, "fake-1"); assert.equal(switched.data.model.id, "fake-2"); assert.deepEqual(switched.data.thinking_levels, ["off"]);
            const invalid = await run(f, e.socket, "set_thinking_level"); assert.equal(invalid.error.code, "invalid_level"); assert.deepEqual(invalid.error.details.available, ["off"]); assert.equal(f.calls.thinking.length, 1);
            const compacted = await run(f, e.socket, "compact"); assert.equal(compacted.data.tokens_before, 1234); assert.equal(compacted.data.tokens_after, 123); assert.deepEqual(f.calls.compact, ["keep facts"]);
            assert.equal(f.notes.length, hasUI ? 3 : 0);
            // Every persistent file the extension produces belongs to this temp GLEN_HOME.
            assert.deepEqual(fs.readdirSync(f.home), ["run"]);
        } finally { await f.close(); }
    }
});
test("AC366 model errors, compaction errors and busy guards", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        assert.equal((await run(f, e.socket, "set_model", { provider: "missing", model_id: "x" })).error.code, "model_not_found");
        f.fields.noAuth = true; assert.equal((await run(f, e.socket, "set_model")).error.code, "no_auth");
        f.fields.noAuth = false; f.fields.modelError = "provider failed"; assert.equal((await run(f, e.socket, "set_model")).error.code, "omp_error");
        for (const [idle, pending] of [[false, false], [true, true]]) { f.fields.idle = idle; f.fields.pending = pending; assert.equal((await run(f, e.socket, "compact")).error.code, "busy"); }
        assert.equal(f.calls.compact.length, 0); f.fields.pending = false; f.fields.compactError = "Nothing to compact";
        const failed = await run(f, e.socket, "compact"); assert.equal(failed.error.code, "omp_error"); assert.equal(failed.error.message, "Nothing to compact");
    } finally { await f.close(); }
});
test("AC366 compaction without a new entry is an omp error even when it returns normally", async () => {
    for (const compactResult of [undefined, "ok", "failed"]) {
        const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
            // An older compaction and unchanged tokens must not count as success.
            assert.equal((await run(f, e.socket, "compact")).ok, true);
            const before = [...f.ctx.sessionManager.getEntries()];
            f.fields.compactNoEntry = true; f.fields.compactResult = compactResult;
            const failed = await run(f, e.socket, "compact");
            assert.equal(failed.ok, false); assert.equal(failed.error.code, "omp_error");
            assert.equal(failed.error.message, "compaction did not run");
            assert.deepEqual(f.ctx.sessionManager.getEntries(), before); assert.equal(f.fields.tokens, 123);
            f.fields.compactNoEntry = false; f.fields.compactResult = undefined;
            assert.equal((await run(f, e.socket, "compact")).ok, true);
        } finally { await f.close(); }
    }
});
test("AC366 cancelled compaction returning normally is an omp error", async () => {
    const f = await fixture({ compactDelay: 1000, compactAbortReturns: true });
    try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const c = await client(e.socket, frame("compact")); await c.until(() => f.calls.compact.length === 1);
        assert.equal((await run(f, e.socket, "abort")).ok, true);
        await c.until(() => c.ended); assert.equal(c.frames.length, 1);
        assert.equal(c.frames[0].ok, false); assert.equal(c.frames[0].error.code, "omp_error");
        assert.equal(c.frames[0].error.message, "compaction cancelled");
        assert.equal(f.fields.tokens, 1234); assert.deepEqual(f.ctx.sessionManager.getEntries(), []);
    } finally { await f.close(); }
});
test("AC366 session switch while setModel awaits returns post-op id", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        f.fields.onSetModel = async () => { f.fields.sessionId = "new-session"; };
        const r = await run(f, e.socket, "set_model", { ...args.set_model, expect_session_id: f.fields.sessionId }); assert.equal(r.data.session_id, "new-session");
    } finally { await f.close(); }
});
test("AC366 abort cancels glen compaction; second compact is busy", async () => {
    const f = await fixture({ compactDelay: 1000 }); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        const c = await client(e.socket, frame("compact")); await c.until(() => f.calls.compact.length === 1);
        assert.equal((await run(f, e.socket, "compact")).error.code, "busy");
        const aborted = await run(f, e.socket, "abort"); assert.equal(aborted.data.was, "busy"); assert.equal(f.calls.abort, 1);
        await c.until(() => c.ended); assert.equal(c.frames.length, 1); assert.equal(c.frames[0].error.code, "omp_error");
    } finally { await f.close(); }
});
test("AC366 subagent context cannot execute controls", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n")); f.ctx.agent.kind = "sub";
        for (const op of Object.keys(args)) assert.deepEqual(await request(e.socket, frame(op, args[op])), []);
        assert.deepEqual(f.calls, { abort: 0, setModel: [], thinking: [], compact: [] });
        f.ctx.agent.kind = "main";
    } finally { f.ctx.agent.kind = "main"; await f.close(); }
});
