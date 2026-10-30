import { test } from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { finish } from "../index.ts";
import { fixture, client, frame, request } from "./fake-pi.ts";

function socket() {
    const conn: any = new EventEmitter(), calls: string[] = [];
    const ctx: any = { setTimeout: (fn: () => void, ms: number) => setTimeout(fn, ms), clearTimer: (timer: any) => clearTimeout(timer) };
    conn.destroyed = false; conn.writableEnded = false; conn.readableEnded = true; conn.writableLength = 0;
    conn.write = (data: string, callback: Function) => { calls.push(`write:${data}`); conn.flushed = callback; return false; };
    conn.end = (callback: Function) => { calls.push("end"); conn.writableEnded = true; conn.finished = callback; };
    conn.destroy = () => { calls.push("destroy"); conn.destroyed = true; conn.emit("close"); };
    return { conn, calls, ctx };
}
test("D1 final response waits for the write callback under backpressure before ending", () => {
    for (const response of [undefined, { ok: true }]) {
        const { conn, calls, ctx } = socket();
        // With no final frame, the barrier protects a previously queued event.
        if (response === undefined) conn.writableLength = 1;
        finish(ctx, conn, response);
        assert.deepEqual(calls, [response ? 'write:{"ok":true}\n' : "write:"]);
        conn.flushed(); assert.equal(calls.at(-1), "end");
        conn.finished(); assert.equal(calls.at(-1), "destroy");
    }
});
test("C2-8 idle finish ends exactly once without an empty flush write", () => {
    const { conn, calls, ctx } = socket();
    finish(ctx, conn);
    assert.deepEqual(calls, ["end"]);
    finish(ctx, conn);
    assert.deepEqual(calls, ["end"]);
    conn.finished();
    assert.deepEqual(calls, ["end", "destroy"]);
});
test("C2-8 idle finish preserves the close deadline while waiting for peer EOF", t => {
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const { conn, calls, ctx } = socket(); conn.readableEnded = false;
    finish(ctx, conn); conn.finished();
    assert.deepEqual(calls, ["end"]);
    t.mock.timers.tick(2999); assert.equal(conn.destroyed, false);
    t.mock.timers.tick(1); assert.deepEqual(calls, ["end", "destroy"]);
});
test("D1 stalled writes and stalled end callbacks are destroyed within three seconds", t => {
    t.mock.timers.enable({ apis: ["setTimeout"] });
    for (const flushed of [false, true]) {
        const { conn, calls, ctx } = socket(); finish(ctx, conn, { ok: true });
        if (flushed) conn.flushed();
        t.mock.timers.tick(2999); assert.equal(conn.destroyed, false);
        t.mock.timers.tick(1); assert.equal(calls.at(-1), "destroy");
    }
});
test("AC307 finishing a response keeps reading trailing input until peer EOF or the close deadline", t => {
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const { conn, calls, ctx } = socket(); conn.readableEnded = false;
    finish(ctx, conn, { ok: true });
    conn.flushed(); conn.finished();
    assert.deepEqual(calls, ['write:{"ok":true}\n', "end"]);
    assert.equal(conn.destroyed, false);
    conn.emit("data", Buffer.alloc(65536, "x"));
    t.mock.timers.tick(2999); assert.equal(conn.destroyed, false);
    t.mock.timers.tick(1); assert.equal(calls.at(-1), "destroy");
});
test("D1 failed response writes destroy the socket without ending", () => {
    for (const throws of [false, true]) {
        const { conn, calls, ctx } = socket();
        if (throws) conn.write = () => { throw new Error("closed"); };
        finish(ctx, conn, { ok: false });
        if (!throws) conn.flushed(new Error("closed"));
        assert.equal(conn.destroyed, true); assert.equal(calls.includes("end"), false);
    }
});
test("D1 fallback timer uses the owning context and clears its opaque handle on close", () => {
    const { conn } = socket(), handle = Symbol("context timer");
    let scheduled: (() => void) | undefined, cleared = false;
    const ctx: any = {
        setTimeout(fn: () => void, ms: number) { assert.equal(ms, 3000); scheduled = fn; return handle; },
        clearTimer(timer: unknown) { assert.equal(timer, handle); cleared = true; },
    };
    finish(ctx, conn, { ok: true });
    assert.equal(cleared, false); assert.ok(scheduled);
    scheduled(); assert.equal(conn.destroyed, true); assert.equal(cleared, true);
});
test("D1 hello, state, controls and errors send exactly one frame then EOF with an open peer", async () => {
    const f = await fixture(); try { const e = await f.start(); assert.ok(e, f.notes.join("\n"));
        for (let i = 0; i < 20; i++) {
            await Promise.all(["hello", "state", "abort", "list_models", "unknown"].map(async op => {
                const c = await client(e.socket);
                try {
                    c.socket.allowHalfOpen = true;
                    let eof = false; c.socket.once("end", () => { eof = true; });
                    c.socket.write(JSON.stringify(frame(op)) + "\n");
                    await c.until(() => eof);
                    assert.equal(c.frames.length, 1); assert.equal(c.frames[0].id, "r");
                    assert.equal(c.frames[0].ok, op !== "unknown");
                } finally { c.socket.destroy(); }
            }));
        }
    } finally { await f.close(); }
});

test("C2-6 compact without instructions reports omp's no-entry callback message", async () => {
    for (const compactOnError of [new Error("Nothing to compact (session too small)"), "Nothing to compact (session too small)"]) {
        const f = await fixture({ compactNoEntry: true, compactOnError });
        try {
            const e = await f.start(); assert.ok(e, f.notes.join("\n"));
            const [result] = await request(e.socket, frame("compact"));
            assert.equal(result.ok, false);
            assert.equal(result.error.code, "omp_error");
            assert.equal(result.error.message, "Nothing to compact (session too small)");
            assert.equal(f.calls.compact.length, 1);
            assert.deepEqual(Object.keys(f.calls.compact[0]), ["onError"]);
            assert.equal(typeof f.calls.compact[0].onError, "function");
        } finally { await f.close(); }
    }
});

test("C2-6 compact instructions require the string API and retain generic no-entry errors", async () => {
    for (const instructions of ["keep facts", ""]) {
        for (const compactResult of [undefined, "cancelled"]) {
            const f = await fixture({ compactNoEntry: true, compactResult, compactOnError: new Error("Nothing to compact (session too small)") });
            try {
                const e = await f.start(); assert.ok(e, f.notes.join("\n"));
                const [result] = await request(e.socket, frame("compact", { instructions }));
                assert.equal(result.ok, false);
                assert.equal(result.error.code, "omp_error");
                assert.equal(result.error.message, compactResult === "cancelled" ? "compaction cancelled" : "compaction did not run");
                // CompactOptions has no customInstructions field: this must be one string argument.
                assert.deepEqual(f.calls.compact, [instructions]);
            } finally { await f.close(); }
        }
    }
});
