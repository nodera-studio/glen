import { test } from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { finish } from "../index.ts";
import { fixture, client, frame } from "./fake-pi.ts";

function socket() {
    const conn: any = new EventEmitter(), calls: string[] = [];
    const ctx: any = { setTimeout: (fn: () => void, ms: number) => setTimeout(fn, ms), clearTimer: (timer: any) => clearTimeout(timer) };
    conn.destroyed = false; conn.writableEnded = false; conn.readableEnded = true;
    conn.write = (data: string, callback: Function) => { calls.push(`write:${data}`); conn.flushed = callback; return false; };
    conn.end = (callback: Function) => { calls.push("end"); conn.writableEnded = true; conn.finished = callback; };
    conn.destroy = () => { calls.push("destroy"); conn.destroyed = true; conn.emit("close"); };
    return { conn, calls, ctx };
}
test("D1 final response waits for the write callback under backpressure before ending", () => {
    for (const response of [undefined, { ok: true }]) {
        const { conn, calls, ctx } = socket(); finish(ctx, conn, response);
        assert.deepEqual(calls, [response ? 'write:{"ok":true}\n' : "write:"]);
        conn.flushed(); assert.equal(calls.at(-1), "end");
        conn.finished(); assert.equal(calls.at(-1), "destroy");
    }
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
