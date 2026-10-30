import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { fixture, client, request, frame } from "./fake-pi.ts";
const pause = (ms = 80) => new Promise(r => setTimeout(r, ms));
async function send(f: any, id = "r", deliver_as = "auto") { assert.ok(f.handle.endpoint(), f.notes.join("\n")); const c = await client(f.handle.endpoint().socket, frame("send", { text: "hi", deliver_as, job_id: "job", expect_session_id: f.fields.sessionId }, id)); await c.until(() => c.frames.length > 0); assert.equal(c.frames[0].ok, true); return c; }
async function end(f: any, c: any) { f.fields.idle = true; f.fields.pending = false; await f.emit("agent_end"); await c.until(() => c.ended); return c.frames.at(-1); }
test("AC309 delivery table; ack first with synchronous agent_start", async () => {
    for (const idle of [true, false]) for (const how of ["auto", "steer", "follow_up"]) { const f = await fixture({ idle }); try {
        await f.start(); f.fields.onSend = () => { void f.emit("agent_start"); void f.emit("message_end", { message: { role: "user", content: "hi" } }); }; const c = await send(f, "r", how); assert.equal(c.frames[0].data.delivered_as, idle ? "prompt" : how === "steer" ? "steer" : "follow_up"); assert.deepEqual(f.sends[0].opts, idle ? undefined : { deliverAs: how === "steer" ? "steer" : "followUp" });
        await c.until(() => c.frames.some((r: any) => r.event === "consumed")); assert.equal(c.frames.filter((r: any) => r.event === "consumed").length, 1); await end(f, c);
    } finally { await f.close(); } }
});
test("AC309 bad arguments, mismatch, duplicate ids and thrown send", async () => {
    const f = await fixture(); try { const e = await f.start();
        for (const args of [{}, { text: " ", deliver_as: "auto" }, { text: "x".repeat(524289), deliver_as: "auto" }, { text: "ok", deliver_as: "aside" }]) assert.equal((await request(e.socket, frame("send", args)))[0].error.code, "bad_request");
        assert.equal((await request(e.socket, frame("send", { text: "hi", deliver_as: "auto", expect_session_id: "other" })))[0].error.code, "session_mismatch"); assert.equal(f.sends.length, 0);
        f.fields.throwSend = true; assert.equal((await request(e.socket, frame("send", { text: "hi", deliver_as: "auto" })))[0].error.code, "send_failed"); f.fields.throwSend = false;
        const c = await send(f); assert.equal((await request(e.socket, frame("send", { text: "hi", deliver_as: "auto" })))[0].error.message, "duplicate request id"); await f.emit("message_end", { message: { role: "user", content: "hi" } }); await f.emit("agent_start"); await end(f, c);
        assert.equal((await request(e.socket, frame("send", { text: "hi", deliver_as: "auto" })))[0].error.code, "bad_request"); assert.equal(f.sends.length, 2);
    } finally { await f.close(); }
});
test("AC310 polls until idle, scopes assistant text and stops on intervening start", async () => {
    const f = await fixture(); try { await f.start(); const c = await send(f);
        f.fields.idle = false; await f.emit("agent_end", { messages: [{ role: "assistant", content: "OLD" }] }); await pause(); assert.equal(c.ended, false);
        await f.emit("message_end", { message: { role: "user", content: "hi" } });
        await f.emit("agent_start"); f.fields.idle = false; await f.emit("agent_end", { messages: [{ role: "assistant", content: "OLD" }] }); await pause(180); assert.equal(c.ended, false); f.fields.idle = true;
        await c.until(() => c.ended); assert.equal(c.frames.at(-1).assistant_text, null); assert.equal(c.frames.filter((r: any) => r.event === "settled").length, 1);
        const d = await send(f, "second"); await f.emit("message_end", { message: { role: "user", content: "hi" } }); await f.emit("agent_start"); f.fields.idle = false; await f.emit("agent_end"); await f.emit("agent_start"); f.fields.idle = true; await pause(); assert.equal(d.ended, false);
        await f.emit("message_end", { message: { role: "assistant", content: [{ type: "text", text: "new" }], stopReason: "stop" } }); assert.equal((await end(f, d)).assistant_text, "new");
    } finally { await f.close(); }
});
test("AC310 continuation, pending, consumption and subagent gates", async () => {
    const f = await fixture({ idle: false }); try { await f.start(); const c = await send(f);
        for (const payload of [{ willContinue: true }, { awaitingAsyncWork: true }]) { await f.emit("agent_end", payload); await pause(); assert.equal(c.ended, false); }
        f.fields.pending = true; f.fields.idle = true; await f.emit("agent_end"); await pause(); assert.equal(c.ended, false); f.fields.pending = false;
        f.ctx.agent.kind = "sub"; await f.emit("agent_end"); await pause(); assert.equal(c.ended, false); f.ctx.agent.kind = "main";
        await f.emit("before_agent_start", { reason: "prompt", prompt: "other\n\nhi" }); await f.emit("message_end", { message: { role: "user", content: "hi" } });
        await f.emit("message_end", { message: { role: "assistant", content: [{ type: "text", text: "bad" }], stopReason: "error", errorMessage: "model error" } });
        const final = await end(f, c); assert.equal(final.status, "error"); assert.equal(final.error_message, "model error"); assert.equal(c.frames.filter((r: any) => r.event === "consumed").length, 1);
    } finally { await f.close(); }
});
test("AC311 attribution uses consumed requests and inbox is durable after disconnect", async () => {
    const f = await fixture({ idle: false }); try { await f.start(); const c = await send(f); const tool = f.tools[0];
        await tool.execute("tui", { text: "user run" }, null, null, f.ctx); let records = fs.readFileSync(path.join(f.home, "inbox", `${f.fields.sessionId}.ndjson`), "utf8").trim().split("\n").map(JSON.parse); assert.equal(records[0].job_id, null); assert.equal(records[0].request_id, null);
        await f.emit("message_end", { message: { role: "user", content: "hi" } }); await tool.execute("tool", { text: "reply", final: true }, null, null, f.ctx); await c.until(() => c.frames.some((r: any) => r.event === "reply"));
        records = fs.readFileSync(path.join(f.home, "inbox", `${f.fields.sessionId}.ndjson`), "utf8").trim().split("\n").map(JSON.parse); assert.deepEqual(c.frames.find((r: any) => r.event === "reply").reply, records[1]); assert.equal(records[1].job_id, "job");
        c.socket.destroy(); await c.until(() => c.ended); await tool.execute("late", { text: "late" }, null, null, f.ctx); assert.equal(f.handle.requests()[0].jobId, "job");
        await f.emit("agent_end");
    } finally { await f.close(); }
});
test("AC312 identity refreshed by socket requests, switch ordering and pid-safe cleanup", async () => {
    const f = await fixture(); try { const e = await f.start(); const c = await send(f); await f.emit("message_end", { message: { role: "user", content: "hi" } }); await f.emit("agent_start"); f.fields.sessionId = "next";
        const [hello] = await request(e.socket, frame("hello")); assert.equal(hello.data.session_id, "next"); await c.until(() => c.frames.some((r: any) => r.event === "session_changed")); assert.equal(c.frames.at(-1).reason, "moved"); assert.ok(fs.existsSync(path.join(path.dirname(e.registry), "next.json"))); assert.ok(!fs.existsSync(e.registry)); await end(f, c);
        const d = await send(f, "new"); f.fields.sessionId = "new-session"; await f.emit("session_switch", { reason: "new" }); await d.until(() => d.ended); assert.deepEqual(d.frames.slice(-2).map((r: any) => r.event), ["session_changed", "error"]); assert.equal(d.frames.at(-1).error.code, "session_switched");
        const current = f.handle.endpoint()!; fs.writeFileSync(current.registry, JSON.stringify({ pid: 123 })); const active = await send(f, "shutdown"); await f.emit("session_shutdown"); await active.until(() => active.ended); assert.equal(active.frames.at(-1).error.code, "shutdown"); assert.ok(fs.existsSync(current.registry)); assert.ok(!fs.existsSync(current.socket));
    } finally { await f.close(); }
});
test("AC310 agent_start before the ack alone is not consumption", async () => {
    const f = await fixture(); try { await f.start(); f.fields.onSend = () => { void f.emit("agent_start"); }; const c = await send(f);
        f.fields.idle = false; await f.emit("agent_end"); await pause(); assert.equal(c.ended, false); assert.equal(c.frames.some((r: any) => r.event === "consumed"), false);
        await f.emit("message_end", { message: { role: "user", content: "hi" } }); await end(f, c); assert.equal(c.frames.filter((r: any) => r.event === "consumed").length, 1);
    } finally { await f.close(); }
});
test("AC311 both inbox and socket write failure returns a tool error", async () => {
    const f = await fixture(); try { await f.start(); const c = await send(f); await f.emit("message_end", { message: { role: "user", content: "hi" } }); await f.emit("agent_start");
        fs.writeFileSync(path.join(f.home, "inbox"), "blocked");
        const r = f.handle.requests()[0], original = r.conn!.write;
        r.conn!.write = () => { throw new Error("failed socket write"); };
        const result = await f.tools[0].execute("t", { text: "reply" }, null, null, f.ctx);
        assert.equal(result.isError, true); assert.match(result.content[0].text, /cannot write the glen inbox/);
        r.conn!.write = original; await end(f, c);
    } finally { await f.close(); }
});
