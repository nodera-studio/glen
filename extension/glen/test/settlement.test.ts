import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import { fixture, client, request, frame } from "./fake-pi.ts";

const user = (content: string) => ({ role: "user", content });
const assistant = (content: string) => ({ role: "assistant", content, stopReason: "stop" });
async function send(f: Awaited<ReturnType<typeof fixture>>, id: string, text: string, deliver_as = "auto") {
    const c = await client(f.handle.endpoint()!.socket, frame("send", { text, deliver_as, job_id: id }, id));
    await c.until(() => c.frames.length > 0);
    assert.equal(c.frames[0].ok, true);
    return c;
}
test("O6 replays all 42 events of 05-trace in exact order with transcript message bodies", async () => {
    // The trace records event order/flags but omits bodies. Bodies below come from
    // 05-transcript.txt; the initial pong exchange precedes that transcript excerpt.
    const trace = fs.readFileSync(new URL("./fixtures/05-trace.ndjson", import.meta.url), "utf8").trim().split("\n").map(line => JSON.parse(line));
    assert.equal(trace.length, 42);
    const texts = ["Reply pong", "Use the bash tool to run: for i in 1 2 3 4 5 6 7 8 9 10; do sleep 1; done; then answer done.", "Then answer with the word second.", "Use the bash tool to run: sleep 20; then answer slow.", "Stop waiting and answer with the word steered."];
    const bodies = new Map([
        [3, user(texts[0])], [4, assistant("")], [5, { role: "toolResult", content: "delivered" }], [7, assistant("pong")],
        [13, user(texts[1])], [14, assistant("")], [15, { role: "toolResult", content: "(no output)\nWall time: 10.05 seconds" }], [17, assistant("done")],
        [20, user(texts[2])], [21, assistant("second")], [27, user(texts[3])], [28, assistant("")],
        [29, { role: "toolResult", content: "Backgrounded early to handle an incoming message; the command keeps running." }],
        [32, user(texts[4])], [33, assistant("steered")], [37, { role: "custom", customType: "async-result", content: "<system-notice>Background job bg_1 has completed.</system-notice>" }],
        [38, assistant("The sleep 20 job finished; I already answered steered.")],
    ]);
    const f = await fixture();
    const clients = new Map<string, Awaited<ReturnType<typeof client>>>();
    const ids: string[] = [];
    const seen: string[] = [];
    try {
        for (const [i, row] of trace.entries()) {
            for (const r of row.requests) if (!clients.has(r.id)) {
                const n = ids.length;
                ids.push(r.id);
                f.fields.idle = r.deliveredAs === "prompt";
                clients.set(r.id, await send(f, r.id, texts[n], r.deliveredAs === "steer" ? "steer" : "auto"));
            }
            f.fields.idle = row.idle; f.fields.pending = row.queued;
            seen.push(row.event);
            if (row.event === "quiescent") {
                // A polled observation, not an omp event. Let the real managed poll run.
                for (const c of clients.values()) await c.until(() => c.ended);
            } else {
                await f.emit(row.event, { reason: row.reason, willContinue: row.willContinue, awaitingAsyncWork: row.awaitingAsyncWork, message: bodies.get(i) });
            }
            if (i === 20) {
                const a = clients.get(ids[1])!;
                await a.until(() => a.ended);
                assert.equal(a.frames.at(-1).assistant_text, "done");
            }
            if (i === 32) {
                const c = clients.get(ids[3])!;
                await c.until(() => c.ended);
                assert.equal(c.frames.at(-1).assistant_text, null); // bash/tool-only span
            }
            if (i === 37) {
                const d = clients.get(ids[4])!;
                await d.until(() => d.ended);
                assert.equal(d.frames.at(-1).assistant_text, "steered");
                await f.tools[0].execute("async-reply", { text: "unowned wake-up" }, null, null, f.ctx);
            }
        }
        assert.deepEqual(seen, trace.map(row => row.event));
        assert.deepEqual(ids.map(id => clients.get(id)!.frames.at(-1).assistant_text), ["pong", "done", "second", null, "steered"]);
        for (const c of clients.values()) assert.equal(c.frames.filter(r => r.event === "settled").length, 1);
        const inbox = fs.readFileSync(`${f.home}/inbox/${f.fields.sessionId}.ndjson`, "utf8").trim().split("\n").map(line => JSON.parse(line));
        assert.equal(inbox.at(-1).request_id, null);
    } finally { await f.close(); }
});
test("O6 replays all 37 events of 05b-trace in exact order with transcript message bodies", async () => {
    // 05b-transcript.txt supplies the user/assistant text and async-result body.
    // It renders tool calls as summaries, so their assistant text is empty and
    // the omitted tool-result bodies are empty placeholders.
    const trace = fs.readFileSync(new URL("./fixtures/05b-trace.ndjson", import.meta.url), "utf8").trim().split("\n").map(line => JSON.parse(line));
    assert.equal(trace.length, 37);
    const texts = ["Use the bash tool to run: for i in 1 2 3 4 5 6 7 8 9 10; do sleep 1; done; then answer done.", "Then answer with the word second.", "Use the bash tool to run: sleep 20; then answer slow.", "Stop waiting and answer with the word steered."];
    const a = user(texts[0]), b = user(texts[1]), c = user(texts[2]), d = user(texts[3]);
    const tool = assistant(""), result = { role: "toolResult", content: "" };
    const notice = { role: "custom", customType: "async-result", content: "<system-notice>\nBackground job bg_1 has completed. Resume your work using the result below.\n(no output)\n\nWall time: 10.05 seconds\n</system-notice>" };
    const bodies = new Map([
        [3, a], [4, a], [5, tool], [6, tool], [7, result], [8, result],
        [11, d], [12, d], [13, assistant("")], [14, assistant("steered")],
        [17, b], [18, b], [19, assistant("")], [20, assistant("second")],
        [23, c], [24, c], [25, tool], [26, tool], [27, result], [28, result],
        [30, notice], [31, notice], [32, assistant("")], [33, assistant("slow")],
    ]);
    const f = await fixture();
    const clients = new Map<string, Awaited<ReturnType<typeof client>>>();
    const ids: string[] = [];
    const seen: string[] = [];
    try {
        for (const [i, row] of trace.entries()) {
            for (const r of row.requests) if (!clients.has(r.id)) {
                const n = ids.length;
                ids.push(r.id);
                f.fields.idle = r.deliveredAs === "prompt";
                clients.set(r.id, await send(f, r.id, texts[n], r.deliveredAs === "steer" ? "steer" : "auto"));
            }
            f.fields.idle = row.idle; f.fields.pending = row.queued;
            seen.push(row.event);
            if (row.event === "quiescent") {
                for (const c of clients.values()) await c.until(() => c.ended);
            } else {
                if (row.event === "message_start" || row.event === "message_end") assert.ok(bodies.has(i), `missing body for event ${i}`);
                await f.emit(row.event, { reason: row.reason, willContinue: row.willContinue, awaitingAsyncWork: row.awaitingAsyncWork, message: bodies.get(i) });
            }
            if (i === 11) {
                const a = clients.get(ids[0])!;
                await a.until(() => a.ended);
                assert.equal(a.frames.at(-1).assistant_text, null); // closed by the steer, never answered
            }
            if (i === 30 || i === 31) {
                assert.equal(f.handle.requests().some(r => r.id === ids[2] && r.consumed), true);
                assert.equal(clients.get(ids[2])!.frames.some(r => r.event === "settled"), false);
            }
        }
        assert.deepEqual(seen, trace.map(row => row.event));
        assert.deepEqual(ids.map(id => clients.get(id)!.frames.at(-1).assistant_text), [null, "second", "slow", "steered"]);
        for (const c of clients.values()) {
            assert.equal(c.frames.filter(r => r.event === "consumed").length, 1);
            assert.equal(c.frames.filter(r => r.event === "settled").length, 1);
            assert.deepEqual(c.frames.at(-1).warnings, []);
        }
    } finally { await f.close(); }
});
const notices = [
    { role: "custom", content: "custom notice" },
    { role: "custom", customType: "async-result", content: "<system-notice>background result</system-notice>" },
    { role: "async-result", content: "background result" },
    { role: "system", content: "system notice" },
    { role: "system-notice", content: "system notice" },
    { role: "developer", content: "developer notice" },
];
test("notices before assistant text retain the span through tool turns and glen_reply", async () => {
    for (const notice of notices) for (const starts of [true, false]) {
        const f = await fixture();
        try {
            await f.start(); const c = await send(f, "waiting", "waiting for an answer");
            await f.emit("message_end", { message: user("waiting for an answer") });
            await f.emit("message_start", { message: assistant("") });
            await f.emit("message_end", { message: assistant("") });
            await f.emit("message_end", { message: { role: "toolResult", content: "backgrounded" } });
            await f.tools[0].execute("progress", { text: "still working" }, null, null, f.ctx);
            if (starts) await f.emit("message_start", { message: notice });
            await f.emit("message_end", { message: notice });
            assert.equal(f.handle.requests().length, 1);
            await f.tools[0].execute("owned", { text: "still owned" }, null, null, f.ctx);
            await c.until(() => c.frames.filter(r => r.event === "reply").length === 2);
            assert.equal(c.frames.findLast(r => r.event === "reply").reply.request_id, "waiting");
            assert.equal(c.frames.some(r => r.event === "settled"), false);
            await f.emit("message_end", { message: assistant("draft") });
            await f.emit("message_end", { message: assistant("real answer") });
            f.fields.idle = true; await f.emit("agent_end"); await c.until(() => c.ended);
            assert.equal(c.frames.at(-1).assistant_text, "real answer");
            assert.equal(c.frames.filter(r => r.event === "settled").length, 1);
        } finally { await f.close(); }
    }
});
test("notices after assistant text close the span at start or the end-only fallback", async () => {
    for (const notice of notices) for (const starts of [true, false]) {
        const f = await fixture();
        try {
            await f.start(); const c = await send(f, "answered", "answer me");
            await f.emit("message_end", { message: user("answer me") });
            await f.emit("message_end", { message: assistant("last answer") });
            if (starts) await f.emit("message_start", { message: notice });
            else await f.emit("message_end", { message: notice });
            await c.until(() => c.ended);
            assert.equal(c.frames.at(-1).assistant_text, "last answer");
            await f.tools[0].execute("unowned", { text: "wake-up reply" }, null, null, f.ctx);
            if (starts) await f.emit("message_end", { message: notice });
            await f.emit("message_end", { message: assistant("later unowned answer") });
            assert.equal(c.frames.filter(r => r.event === "settled").length, 1);
            assert.equal(c.frames.some(r => r.event === "reply"), false);
            const inbox = fs.readFileSync(`${f.home}/inbox/${f.fields.sessionId}.ndjson`, "utf8").trim().split("\n").map(line => JSON.parse(line));
            assert.equal(inbox.at(-1).request_id, null);
        } finally { await f.close(); }
    }
});
test("failed asynchronous submission times out not_delivered and never adopts an unrelated run", async () => {
    const f = await fixture(); f.deps.deliveryMs = 150;
    try {
        await f.start(); const c = await send(f, "failed", "never appended");
        f.fields.idle = false;
        await f.emit("before_agent_start", { reason: "prompt", prompt: "unrelated never appended substring" });
        await f.emit("agent_start");
        await f.emit("message_end", { message: user("unrelated") });
        await f.emit("message_end", { message: assistant("someone else's answer") });
        await f.tools[0].execute("other", { text: "someone else's reply" }, null, null, f.ctx);
        await c.until(() => c.ended);
        assert.equal(c.frames.at(-1).error.code, "not_delivered");
        assert.deepEqual(c.frames.map(r => r.event).filter(Boolean), ["error"]);
    } finally { await f.close(); }
});
test("removed follow-up fails not_delivered at quiescence without adopting user text", async () => {
    const f = await fixture({ idle: false, pending: true });
    try {
        await f.start(); const c = await send(f, "removed", "removed follow-up");
        await f.emit("message_end", { message: user("user's own message") });
        await f.emit("message_end", { message: assistant("user's own answer") });
        f.fields.pending = false; f.fields.idle = true; await f.emit("agent_end");
        await c.until(() => c.ended);
        assert.equal(c.frames.at(-1).error.code, "not_delivered");
        assert.equal(c.frames.some(r => r.event === "consumed" || r.event === "settled"), false);
    } finally { await f.close(); }
});
test("message_start closes the old span and identical injections consume once in order", async () => {
    const f = await fixture();
    try {
        await f.start(); const a = await send(f, "a", "same");
        await f.emit("message_end", { message: user("same") });
        await f.emit("message_end", { message: assistant("first") });
        f.fields.idle = false; const b = await send(f, "b", "same");
        await f.emit("message_start", { message: user("same") });
        await a.until(() => a.ended); assert.equal(a.frames.at(-1).assistant_text, "first");
        await f.tools[0].execute("gap", { text: "between spans" }, null, null, f.ctx);
        assert.equal(b.frames.some(r => r.event === "reply"), false);
        await f.emit("message_end", { message: user("same") });
        await f.tools[0].execute("b-reply", { text: "second reply" }, null, null, f.ctx);
        await f.emit("message_end", { message: assistant("second") });
        await f.emit("message_end", { message: { role: "developer", content: "notice" } });
        await b.until(() => b.ended); assert.equal(b.frames.at(-1).assistant_text, "second");
        assert.equal(b.frames.filter(r => r.event === "reply").length, 1);
    } finally { await f.close(); }
});
test("session_switch new and resume terminate even after hello or state updated cached identity", async () => {
    for (const op of ["hello", "state"]) for (const reason of ["new", "resume"]) {
        const f = await fixture();
        try {
            const e = await f.start(); const c = await send(f, "old", "waiting");
            f.fields.sessionId = "next";
            assert.equal((await request(e.socket, frame(op)))[0].data.session_id, "next");
            await f.emit("session_switch", { reason }); await c.until(() => c.ended);
            assert.equal(c.frames.at(-1).error.code, "session_switched");
            assert.equal(f.handle.requests().length, 0);
        } finally { await f.close(); }
    }
});
test("glen_reply survives registry refresh failure and retains a durable inbox record", async () => {
    const f = await fixture();
    try {
        const e = await f.start();
        const saved = `${f.home}/saved-live`;
        fs.renameSync(`${f.home}/run/live`, saved);
        fs.writeFileSync(`${f.home}/run/live`, "blocked");
        f.fields.sessionFile = "/fixture/new-session.jsonl";
        const result = await f.tools[0].execute("durable", { text: "must survive" }, null, null, f.ctx);
        assert.equal(result.isError, undefined);
        assert.equal(result.content[0].text, "delivered");
        const records = fs.readFileSync(`${f.home}/inbox/${f.fields.sessionId}.ndjson`, "utf8").trim().split("\n").map(line => JSON.parse(line));
        assert.equal(records[0].text, "must survive");
        assert.ok(f.notes.some(note => note.includes("registry refresh failed")));
        fs.unlinkSync(`${f.home}/run/live`); fs.renameSync(saved, `${f.home}/run/live`);
        assert.ok(fs.existsSync(e.registry));
    } finally { await f.close(); }
});
test("adjacent queued requests share one batch span, identical replies and peer warnings", async () => {
    const f = await fixture({ idle: false, pending: true });
    try {
        await f.start(); const a = await send(f, "job-a-1234567", "first queued");
        const b = await send(f, "job-b-7654321", "second queued", "steer");
        await f.emit("before_agent_start", { reason: "prompt", prompt: "first queued\n\nsecond queued" });
        await f.emit("agent_start");
        for (const text of ["first queued", "second queued"]) {
            await f.emit("message_start", { message: user(text) });
            await f.emit("message_end", { message: user(text) });
        }
        assert.equal(f.handle.requests().length, 2);
        assert.equal(a.frames.some(r => r.event === "settled"), false);
        await f.emit("message_end", { message: assistant("shared answer") });
        await f.tools[0].execute("batch-tool", { text: "shared reply" }, null, null, f.ctx);
        await f.emit("message_start", { message: { role: "custom", content: "async wake-up" } });
        await a.until(() => a.ended); await b.until(() => b.ended);
        for (const [c, peer] of [[a, "j7654321"], [b, "j1234567"]] as const) {
            const settled = c.frames.at(-1);
            assert.equal(settled.assistant_text, "shared answer");
            assert.equal(settled.warnings[0].code, "batched");
            assert.match(settled.warnings[0].message, new RegExp(peer));
            assert.equal(c.frames.filter(r => r.event === "consumed").length, 1);
            assert.equal(c.frames.filter(r => r.event === "settled").length, 1);
        }
        assert.deepEqual(a.frames.find(r => r.event === "reply").reply, b.frames.find(r => r.event === "reply").reply);
        const inbox = fs.readFileSync(`${f.home}/inbox/${f.fields.sessionId}.ndjson`, "utf8").trim().split("\n").map(line => JSON.parse(line));
        assert.equal(inbox.length, 1);
        assert.equal(inbox[0].final, false);
        assert.equal(inbox[0].job_id, null);
        assert.equal(inbox[0].request_id, null);
        await f.emit("message_end", { message: { role: "custom", content: "async wake-up" } });
        await f.emit("message_end", { message: assistant("unowned") });
        assert.equal(f.handle.requests().length, 0);
    } finally { await f.close(); }
});

test("an unrelated user or a new run separates empty queued spans instead of batching", async () => {
    for (const separation of ["user", "start"]) {
        const f = await fixture({ idle: false });
        try {
            await f.start(); const a = await send(f, "a", "first"); const b = await send(f, "b", "second");
            await f.emit("message_end", { message: user("first") });
            if (separation === "user") await f.emit("message_end", { message: user("unrelated") });
            else await f.emit("agent_start");
            await f.emit("message_end", { message: user("second") });
            await a.until(() => a.ended);
            await f.emit("message_end", { message: assistant("second only") });
            f.fields.idle = true; await f.emit("agent_end"); await b.until(() => b.ended);
            assert.equal(a.frames.at(-1).assistant_text, null);
            assert.equal(b.frames.at(-1).assistant_text, "second only");
            assert.deepEqual(a.frames.at(-1).warnings, []);
            assert.deepEqual(b.frames.at(-1).warnings, []);
        } finally { await f.close(); }
    }
});
test("notices, tool-only turns and glen_reply keep queued requests from batching across them", async () => {
    for (const activity of ["message_start", "message_end", "reply", "notice"]) {
        const f = await fixture({ idle: false });
        try {
            await f.start(); const a = await send(f, "a", "first"); const b = await send(f, "b", "second");
            await f.emit("message_end", { message: user("first") });
            if (activity === "reply") await f.tools[0].execute("progress", { text: "working" }, null, null, f.ctx);
            else if (activity === "notice") await f.emit("message_end", { message: { role: "custom", content: "notice before an answer" } });
            else await f.emit(activity, { message: assistant("") });
            await f.emit("message_end", { message: user("second") });
            await a.until(() => a.ended);
            await f.emit("message_end", { message: assistant("second only") });
            f.fields.idle = true; await f.emit("agent_end"); await b.until(() => b.ended);
            assert.equal(a.frames.at(-1).assistant_text, null);
            assert.equal(b.frames.at(-1).assistant_text, "second only");
            assert.deepEqual(a.frames.at(-1).warnings, []);
            assert.deepEqual(b.frames.at(-1).warnings, []);
        } finally { await f.close(); }
    }
});
