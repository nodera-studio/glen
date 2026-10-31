// glen-extension version=dev protocol=1 digest=none
import type { ExtensionAPI, ExtensionContext } from "@oh-my-pi/pi-coding-agent";
import * as fs from "node:fs";
import * as net from "node:net";
import * as path from "node:path";
import * as os from "node:os";
import * as crypto from "node:crypto";
const GLEN_VERSION = "__GLEN_VERSION__";
const BAKED_GLEN_HOME = "__GLEN_HOME__";
export const PROTOCOL = 1;
export const OPS = ["hello", "state", "send", "abort", "list_models", "set_model", "set_thinking_level", "compact"];
export const RESERVED_OPS: string[] = [];
export const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
export const LIMITS = { maxLine: 4 * 1024 * 1024, maxText: 524288, maxInstructions: 524288, maxReply: 65536, idleMs: 5000, deliveryMs: 30000 };
type Deps = { env: Record<string, string | undefined>; pid: number; platform: string; homedir: string; isAlive: (pid: number) => boolean; ompVersion: () => Promise<string | null>; idleMs: number; deliveryMs?: number; abortWaitMs?: number; uid?: number | null };
type Request = { id: string; sessionId: string; jobId: string | null; text: string; deliveredAs: string; conn: net.Socket | null; ackSeq: number; acked: boolean; outbox: object[]; consumed: boolean; consumptionSeq: number | null; deliveryTimer?: ReturnType<typeof setTimeout>; lastAssistant: any; lastTextAssistant: any; settled: boolean };
type Endpoint = { compacting: boolean; server: net.Server; connections: Set<net.Socket>; socket: string; home: string; sessionId: string; eventSessionId: string; sessionFile: string | null; ctx: any; requests: Request[]; settledIds: Set<string>; seq: number; lastStart: number; active: Request[]; spanHasOutput: boolean; spanHasActivity: boolean; spanStart: number; boundaryStarted: boolean; mode: string; startedAt: string; ompVersion: string | null; timer?: any; deps: Deps; pi: any };
const ENDPOINTS = new Map<number, Endpoint>();
export function isPlainToken(id: unknown): id is string { return typeof id === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(id); }
export function sunPathMax(platform: string) { return platform === "darwin" ? 104 : 108; }
export function resolveGlenHome(env: Deps["env"], baked: string, homedir: string): string {
    let home = env.GLEN_HOME?.trim() ? env.GLEN_HOME : (!baked.startsWith("__") ? baked : path.join(homedir, ".glen"));
    if (home === "~") home = homedir;
    else if (home.startsWith("~/")) home = path.join(homedir, home.slice(2));
    if (!path.isAbsolute(home)) throw new Error("GLEN_HOME must be an absolute path");
    return path.resolve(home);
}
export function parseReplyArgs(params: any): { text: string; final: boolean } {
    if (typeof params?.text !== "string" || !params.text.trim() || Buffer.byteLength(params.text) > LIMITS.maxReply) throw new Error("glen_reply: text must be a non-empty string of at most 65536 bytes");
    if (params.final !== undefined && typeof params.final !== "boolean") throw new Error("glen_reply: final must be a boolean");
    return { text: params.text, final: params.final ?? false };
}
// GLEN_HOME and its runtime directories must be real directories owned by this user with no
// group/other write bits (W8-SEC-3, the same rule glen enforces in Rust). Missing ones pass.
export function unsafeDir(dir: string, uid: number | null | undefined): string | null {
    let st: fs.Stats;
    try { st = fs.lstatSync(dir); } catch (err) { if ((err as NodeJS.ErrnoException).code === "ENOENT") return null; throw err; }
    if (!st.isDirectory()) return `${dir} is not a directory`;
    if (uid !== null && uid !== undefined && st.uid !== uid) return `${dir} is owned by uid ${st.uid}, not ${uid}`;
    if ((st.mode & 0o022) !== 0) return `${dir} is writable by group or other (mode ${(st.mode & 0o777).toString(8)})`;
    return null;
}
function unsafeHome(home: string, uid: number | null | undefined): string | null {
    for (const dir of [home, path.join(home, "run"), path.join(home, "run/live")]) { const problem = unsafeDir(dir, uid); if (problem) return problem; }
    return null;
}
// One NDJSON record per call. A torn tail (an append cut off before its newline) is terminated
// first so the new record never merges into it (W8-ERR-4, as glen's Rust append_line does).
export function appendLine(file: string, line: string) {
    const fd = fs.openSync(file, "a+", 0o600);
    try {
        const size = fs.fstatSync(fd).size, last = Buffer.alloc(1);
        const torn = size > 0 && fs.readSync(fd, last, 0, 1, size - 1) === 1 && last[0] !== 0x0a;
        fs.appendFileSync(fd, (torn ? "\n" : "") + line + "\n");
    } finally { fs.closeSync(fd); }
}
function quietly(fn: () => void) { try { fn(); } catch { /* best effort cleanup/trace */ } }
function unlink(file: string) { quietly(() => fs.unlinkSync(file)); }
function version() { return GLEN_VERSION.startsWith("__") ? "dev" : GLEN_VERSION; }
function registry(e: Endpoint, id = e.sessionId) { return path.join(e.home, "run/live", `${id}.json`); }
export function removeRegistryIfUnchanged(file: string, expected: string): boolean {
    const tombstone = path.join(path.dirname(file), `.${path.basename(file)}.${crypto.randomUUID()}.stale`);
    try { fs.renameSync(file, tombstone); } catch (err) { if ((err as NodeJS.ErrnoException).code === "ENOENT") return false; throw err; }
    if (fs.readFileSync(tombstone, "utf8") !== expected) {
        // link is create-only: restoring an unexpected registry must not overwrite
        // a concurrent publisher. In that case retain the hidden tombstone.
        try { fs.linkSync(tombstone, file); fs.unlinkSync(tombstone); }
        catch (err) { if ((err as NodeJS.ErrnoException).code !== "EEXIST") throw err; }
        return false;
    }
    fs.unlinkSync(tombstone);
    return true;
}
function removeRegistry(e: Endpoint, id = e.sessionId) {
    if (!isPlainToken(id)) return;
    quietly(() => { const file = registry(e, id), bytes = fs.readFileSync(file, "utf8"); if (JSON.parse(bytes).pid === e.deps.pid) removeRegistryIfUnchanged(file, bytes); });
}
function publish(e: Endpoint, id: string, file: string | null) {
    if (!isPlainToken(id)) return;
    const target = registry(e, id), tmp = path.join(path.dirname(target), `.${id}.${e.deps.pid}.${crypto.randomUUID()}.tmp`);
    const body = { v: 1, pid: e.deps.pid, session_id: id, session_file: file, socket: e.socket, ext_version: version(), omp_version: e.ompVersion, protocol: 1, mode: e.mode, started_at: e.startedAt, cwd: e.ctx.cwd ?? null };
    try { fs.writeFileSync(tmp, JSON.stringify(body) + "\n", { mode: 0o600, flag: "wx" }); fs.renameSync(tmp, target); }
    finally { unlink(tmp); }
}
function trace(e: Endpoint | undefined, event: string, payload: any = {}) {
    if (!e || !path.isAbsolute(e.deps.env.GLEN_EXT_TRACE ?? "")) return;
    quietly(() => fs.appendFileSync(e.deps.env.GLEN_EXT_TRACE!, JSON.stringify({ t: new Date().toISOString(), event, reason: payload.reason, willContinue: payload.willContinue, awaitingAsyncWork: payload.awaitingAsyncWork, idle: e.ctx.isIdle(), queued: e.ctx.hasPendingMessages(), requests: e.requests.map(r => ({ id: r.id, deliveredAs: r.deliveredAs, consumptionSeq: r.consumptionSeq, consumed: r.consumed })) }) + "\n", { mode: 0o600 }));
}
function notify(ctx: any, message: string) { quietly(() => { if (ctx.hasUI) ctx.ui.notify(message, "info"); }); }
function write(conn: net.Socket | null, frame: object): boolean {
    if (!conn || conn.destroyed || conn.writableEnded) return false;
    try { conn.write(JSON.stringify(frame) + "\n"); return true; } catch { return false; }
}
export function finish(ctx: ExtensionContext, conn: net.Socket, frame?: object) {
    if (conn.destroyed || conn.writableEnded) return;
    const timer = ctx.setTimeout(() => quietly(() => conn.destroy()), 3000);
    conn.once("close", () => quietly(() => ctx.clearTimer(timer)));
    const end = () => conn.end(() => {
        // Keep draining ignored input until peer EOF. Destroying with
        // unread input can reset the socket and discard the response.
        if (conn.readableEnded) quietly(() => conn.destroy());
    });
    try {
        if (frame === undefined && conn.writableLength === 0) { end(); return; }
        // An empty write is a flush barrier for already queued stream events.
        conn.write(frame === undefined ? "" : JSON.stringify(frame) + "\n", err => {
            if (err) { quietly(() => conn.destroy()); return; }
            quietly(end);
        });
    } catch { quietly(() => conn.destroy()); }
}
function event(r: Request, kind: string, body: object = {}): boolean {
    const frame = { v: PROTOCOL, id: r.id, event: kind, ...body };
    if (!r.acked) {
        r.outbox.push(frame);
        return !!r.conn && !r.conn.destroyed && !r.conn.writableEnded;
    }
    return write(r.conn, frame);
}
function terminal(e: Endpoint, r: Request, kind: string, body: object) {
    if (r.settled) return;
    event(r, kind, body); r.settled = true;
    if (r.deliveryTimer !== undefined) e.ctx.clearTimer(r.deliveryTimer);
    e.active = e.active.filter(x => x !== r);
    if (r.acked && r.conn) finish(e.ctx, r.conn);
    e.settledIds.add(r.id);
    if (e.settledIds.size > 4096) e.settledIds.delete(e.settledIds.values().next().value!);
    e.requests = e.requests.filter(x => x !== r);
}
function identity(e: Endpoint, reason = "moved") {
    const id = e.ctx.sessionManager.getSessionId(), file = e.ctx.sessionManager.getSessionFile() ?? null;
    if (id === e.sessionId) { if (file !== e.sessionFile) { publish(e, id, file); e.sessionFile = file; } return; }
    const from = e.sessionId;
    if (isPlainToken(id)) publish(e, id, file);
    removeRegistry(e); e.sessionId = id; e.sessionFile = file;
    for (const r of [...e.requests]) {
        event(r, "session_changed", { from, to: id, reason });
        if (!isPlainToken(id)) terminal(e, r, "error", { error: { code: "session_switched", message: "the omp session switched conversations" } });
    }
}
function textOf(message: any): string { return typeof message?.content === "string" ? message.content : (Array.isArray(message?.content) ? message.content.filter((b: any) => b?.type === "text" && typeof b.text === "string").map((b: any) => b.text).join("\n") : ""); }
function jobHandle(r: Request) { return r.jobId ? "j" + r.jobId.replaceAll("-", "").slice(-7) : r.id; }
function closeSpan(e: Endpoint) {
    const batch = [...e.active];
    for (const r of batch) {
        const m = r.lastAssistant, stop = m?.stopReason ?? null;
        const warnings = batch.length > 1 ? [{ code: "batched", message: `omp consumed this request in a batch with ${batch.filter(other => other !== r).map(jobHandle).join(", ")}; they share the reply span` }] : [];
        terminal(e, r, "settled", { status: stop === "aborted" ? "aborted" : stop === "error" ? "error" : "completed", assistant_text: textOf(r.lastTextAssistant) || null, stop_reason: stop, error_message: typeof m?.errorMessage === "string" ? m.errorMessage : null, warnings });
    }
    e.spanHasOutput = false; e.spanHasActivity = false;
}
function pendingRequest(e: Endpoint, message: any) {
    return message?.role === "user" ? e.requests.find(r => !r.consumed && r.text === textOf(message)) : undefined;
}
function boundary(e: Endpoint, message: any) {
    // Notices only end an answered span; a waiting request keeps ownership.
    if (message.role !== "user" && !e.spanHasOutput) { e.spanHasActivity = true; return; }
    const next = pendingRequest(e, message);
    // Only adjacent queued user messages from the same run can share a span.
    // Notices, tool-only turns and glen_reply interrupt adjacency without text.
    // Other user input always breaks ownership, even before an answer.
    const batched = next && next.deliveredAs !== "prompt" && e.active.length > 0 && e.active.every(r => r.deliveredAs !== "prompt") && !e.spanHasActivity && e.spanStart === e.lastStart;
    if (!batched) closeSpan(e);
}
function notDelivered(e: Endpoint, r: Request) {
    if (!r.consumed) terminal(e, r, "error", { error: { code: "not_delivered", message: "omp did not append this request's user message before the delivery deadline or the queue drained" } });
}
function isBoundary(message: any) {
    // User input always separates spans; boundary() gates notices on output.
    return typeof message?.role === "string" && message.role !== "assistant" && message.role !== "toolResult";
}
function settle(e: Endpoint) {
    if (e.timer !== undefined) e.ctx.clearTimer(e.timer);
    const endSeq = e.seq, started = Date.now();
    const poll = () => {
        try {
            if (!ENDPOINTS.has(e.deps.pid) || e.lastStart > endSeq || e.ctx.hasPendingMessages() || Date.now() - started >= 30000) { trace(e, "poll_stopped"); return; }
            if (!e.ctx.isIdle()) { e.timer = e.ctx.setTimeout(poll, 50); return; }
            closeSpan(e);
            for (const r of [...e.requests]) if (r.ackSeq < endSeq) notDelivered(e, r);
            trace(e, "quiescent");
        } catch (error) { trace(e, "poll_error", { reason: String(error) }); }
    };
    e.timer = e.ctx.setTimeout(poll, 50);
}
function state(e: Endpoint, hello: boolean) {
    const busy = !e.ctx.isIdle();
    return { ...(hello ? { protocol: PROTOCOL, protocols: [PROTOCOL], ops: OPS } : {}), ext_version: version(), omp_version: e.ompVersion, pid: e.deps.pid, session_id: e.sessionId, session_file: e.sessionFile, mode: e.mode, state: busy ? "busy" : "idle", busy, queued: e.ctx.hasPendingMessages(), active_requests: e.requests.length };
}
export function thinkingLevelsOf(model: any): string[] { return ["off", ...(model?.reasoning ? model.thinking?.efforts ?? [] : [])]; }
export function modelRef(model: any) { return model ? { provider: model.provider, id: model.id } : null; }
export function modelRow(model: any) { return { provider: model.provider, id: model.id, name: model.name, reasoning: !!model.reasoning, thinking_levels: thinkingLevelsOf(model), context_window: model.contextWindow ?? null, max_tokens: model.maxTokens ?? null }; }
async function control(e: Endpoint, conn: net.Socket, frame: any) {
    const { id, op } = frame, a = frame.args === undefined ? {} : frame.args;
    const error = (code: string, message: string, details?: object) => finish(e.ctx, conn, { v: PROTOCOL, id, ok: false, error: { code, message, ...(details ? { details } : {}) } });
    const success = (data: object) => { identity(e); finish(e.ctx, conn, { v: PROTOCOL, id, ok: true, data: { ...data, session_id: e.sessionId } }); };
    try {
        // A socket belongs only to the cached main-agent context.
        if ((e.ctx.agent?.kind ?? "main") !== "main") { conn.destroy(); return; }
        identity(e);
        if (typeof a !== "object" || a === null || Array.isArray(a) || (a.expect_session_id != null && typeof a.expect_session_id !== "string")) { error("bad_request", `invalid ${op} arguments`); return; }
        if (op === "set_model" && (typeof a.provider !== "string" || !a.provider.trim() || typeof a.model_id !== "string" || !a.model_id.trim())) { error("bad_request", "invalid set_model arguments"); return; }
        if (op === "set_thinking_level" && !THINKING_LEVELS.includes(a.level)) { error("bad_request", "invalid set_thinking_level arguments"); return; }
        if (op === "compact" && a.instructions != null && (typeof a.instructions !== "string" || Buffer.byteLength(a.instructions) > LIMITS.maxInstructions)) { error("bad_request", "invalid compact arguments"); return; }
        if (a.expect_session_id != null && a.expect_session_id !== e.sessionId) { error("session_mismatch", "session identity changed", { session_id: e.sessionId }); return; }
        if (op === "abort") {
            if (e.ctx.isIdle() && !e.compacting) { success({ was: "idle", aborted: false, idle_after: true, queued: e.ctx.hasPendingMessages() }); return; }
            e.ctx.abort(); notify(e.ctx, "glen: stopping the running turn (requested by a glen client)");
            const end = Date.now() + (e.deps.abortWaitMs ?? 3000);
            while ((!e.ctx.isIdle() || e.compacting) && Date.now() < end) { await new Promise<void>(resolve => e.ctx.setTimeout(resolve, 50)); identity(e); }
            success({ was: "busy", aborted: true, idle_after: e.ctx.isIdle() && !e.compacting, queued: e.ctx.hasPendingMessages() });
        } else if (op === "list_models") {
            success({ current: modelRef(e.ctx.model), thinking_level: e.pi.getThinkingLevel() ?? null, models: e.ctx.models.list().map(modelRow) });
        } else if (op === "set_model") {
            const model = e.ctx.models.list().find((m: any) => m.provider === a.provider && m.id === a.model_id);
            if (!model) { error("model_not_found", `Model not found: ${a.provider}/${a.model_id}`); return; }
            const previous = modelRef(e.ctx.model), busy = !e.ctx.isIdle() || e.ctx.hasPendingMessages();
            const applied = await e.pi.setModel(model); identity(e);
            if (!applied) { error("no_auth", `No API key for ${model.provider}/${model.id}`); return; }
            notify(e.ctx, `glen: model set to ${model.provider}/${model.id}`);
            success({ model: { ...modelRef(model), name: model.name }, previous, thinking_level: e.pi.getThinkingLevel() ?? "off", thinking_levels: thinkingLevelsOf(model), busy });
        } else if (op === "set_thinking_level") {
            const available = thinkingLevelsOf(e.ctx.model);
            if (!available.includes(a.level)) { error("invalid_level", `thinking level ${a.level} is unavailable for the current model`, { available, model: modelRef(e.ctx.model) }); return; }
            const previous = e.pi.getThinkingLevel() ?? null, busy = !e.ctx.isIdle() || e.ctx.hasPendingMessages();
            e.pi.setThinkingLevel(a.level); identity(e);
            notify(e.ctx, `glen: thinking set to ${a.level}`);
            success({ thinking_level: e.pi.getThinkingLevel() ?? null, previous, model: modelRef(e.ctx.model), available, busy });
        } else if (op === "compact") {
            if (!e.ctx.isIdle() || e.ctx.hasPendingMessages() || e.compacting) { error("busy", "abort the run first (`glen abort <s>`), or wait for it to finish"); return; }
            e.compacting = true;
            try {
                const tokens_before = e.ctx.getContextUsage()?.tokens ?? null;
                const compactions = () => e.ctx.sessionManager.getEntries().filter((entry: any) => entry.type === "compaction");
                const before = compactions(), lastId = before.at(-1)?.id;
                notify(e.ctx, "glen: compacting the session");
                let capturedMessage: string | undefined;
                const result = await e.ctx.compact(a.instructions != null ? a.instructions : {
                    onError: (err: unknown) => { capturedMessage = err instanceof Error ? err.message : String(err); },
                }); identity(e);
                const after = compactions();
                if (result === "cancelled" || result === "failed" || (after.length <= before.length && after.at(-1)?.id === lastId)) {
                    error("omp_error", capturedMessage ?? (result === "cancelled" ? "compaction cancelled" : "compaction did not run")); return;
                }
                success({ tokens_before, tokens_after: e.ctx.getContextUsage()?.tokens ?? null });
            } finally { e.compacting = false; }
        }
    } catch (err) { quietly(() => identity(e)); error("omp_error", err instanceof Error ? err.message : String(err)); }
}
function connection(e: Endpoint, conn: net.Socket) {
    e.connections.add(conn);
    const chunks: Buffer[] = [];
    let length = 0, handled = false;
    const error = (id: any, code: string, message: string, details?: object) => finish(e.ctx, conn, { v: PROTOCOL, id, ok: false, error: { code, message, ...(details ? { details } : {}) } });
    conn.on("error", () => {});
    // A complete request must arrive within the budget, including trickled bytes.
    const deadline = e.ctx.setTimeout(() => quietly(() => { if (!handled) conn.destroy(); }), e.deps.idleMs);
    conn.on("close", () => { e.connections.delete(conn); quietly(() => e.ctx.clearTimer(deadline)); });
    conn.setTimeout(e.deps.idleMs, () => { if (!handled) conn.destroy(); });
    conn.on("data", chunk => {
        if (handled) return;
        try {
            // Scan each chunk once and retain it without re-copying the prefix.
            // Only bytes before the first newline belong to this request.
            const end = chunk.indexOf(10);
            const added = end < 0 ? chunk.length : end;
            if (added > LIMITS.maxLine - length) {
                handled = true; chunks.length = 0;
                conn.setTimeout(0); e.ctx.clearTimer(deadline);
                error(null, "bad_request", "request line exceeds 4 MiB"); return;
            }
            length += added;
            if (end < 0) { chunks.push(chunk); return; }
            handled = true; conn.setTimeout(0); e.ctx.clearTimer(deadline);
            const line = chunks.length === 0 ? chunk.subarray(0, end) : Buffer.concat([...chunks, chunk.subarray(0, end)], length);
            chunks.length = 0;
            let frame: any;
            try { frame = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(line)); } catch { error(null, "bad_request", "malformed JSON"); return; }
            const id = typeof frame?.id === "string" ? frame.id : null;
            if (id === null) { error(null, "bad_request", "id must be a string"); return; }
            if (frame.v !== PROTOCOL) { error(id, "unsupported_version", "unsupported protocol version", { supported: [PROTOCOL] }); return; }
            identity(e);
            if (frame.op === "hello" || frame.op === "state") { finish(e.ctx, conn, { v: PROTOCOL, id, ok: true, data: state(e, frame.op === "hello") }); return; }
            if (OPS.includes(frame.op) && frame.op !== "send") { void control(e, conn, frame); return; }
            if (frame.op !== "send") { error(id, "unsupported_op", "unsupported operation", { op: frame.op ?? null, ops: OPS, reserved: RESERVED_OPS.includes(frame.op) }); return; }
            if (e.requests.some(r => r.id === id) || e.settledIds.has(id)) { error(id, "bad_request", "duplicate request id"); return; }
            const a = frame.args;
            if (typeof a?.text !== "string" || !a.text.trim() || Buffer.byteLength(a.text) > LIMITS.maxText || !["auto", "steer", "follow_up"].includes(a.deliver_as) || (a.job_id != null && typeof a.job_id !== "string") || (a.expect_session_id != null && typeof a.expect_session_id !== "string")) { error(id, "bad_request", "invalid send arguments"); return; }
            if (a.expect_session_id != null && a.expect_session_id !== e.sessionId) { error(id, "session_mismatch", "session identity changed", { session_id: e.sessionId }); return; }
            const idle = e.ctx.isIdle(), deliveredAs = idle ? "prompt" : a.deliver_as === "steer" ? "steer" : "follow_up";
            const r: Request = { id, sessionId: e.sessionId, jobId: a.job_id ?? null, text: a.text, deliveredAs, conn, ackSeq: e.seq, acked: false, outbox: [], consumed: false, consumptionSeq: null, lastAssistant: null, lastTextAssistant: null, settled: false };
            e.requests.push(r);
            conn.on("close", () => { r.conn = null; });
            try { if (idle) e.pi.sendUserMessage(a.text); else e.pi.sendUserMessage(a.text, { deliverAs: deliveredAs === "steer" ? "steer" : "followUp" }); }
            catch (err) { e.requests = e.requests.filter(x => x !== r); error(id, "send_failed", String(err)); return; }
            write(conn, { v: PROTOCOL, id, ok: true, data: { delivered_as: deliveredAs, session_id: e.sessionId, request_id: id } });
            r.acked = true; r.ackSeq = e.seq;
            if (!r.consumed && !r.settled) r.deliveryTimer = e.ctx.setTimeout(() => notDelivered(e, r), e.deps.deliveryMs ?? LIMITS.deliveryMs);
            for (const frame of r.outbox) write(conn, frame);
            r.outbox = [];
            if (r.settled) finish(e.ctx, conn);
            notify(e.ctx, `glen: message from job ${r.jobId ? "j" + r.jobId.replaceAll("-", "").slice(-7) : "a glen client"} delivered as ${deliveredAs}`);
        } catch (err) { error(null, "internal", String(err)); }
    });
}
function shutdown(e: Endpoint) {
    if (e.timer !== undefined) e.ctx.clearTimer(e.timer);
    const active = new Set(e.requests.map(r => r.conn));
    for (const r of [...e.requests]) terminal(e, r, "error", { error: { code: "shutdown", message: "the omp session is shutting down" } });
    for (const conn of e.connections) if (!active.has(conn)) quietly(() => conn.destroy());
    quietly(() => e.server.close()); unlink(e.socket); removeRegistry(e); ENDPOINTS.delete(e.deps.pid);
}
function sweep(home: string, deps: Deps) {
    const dir = path.join(home, "run/live");
    for (const name of fs.readdirSync(dir)) {
        if (name.startsWith(".")) continue;
        if (name.endsWith(".json")) quietly(() => { const file = path.join(dir, name), bytes = fs.readFileSync(file, "utf8"), r = JSON.parse(bytes); if (Number.isInteger(r.pid) && r.pid > 0 && !deps.isAlive(r.pid) && removeRegistryIfUnchanged(file, bytes)) unlink(path.join(dir, `${r.pid}.sock`)); });
        const m = /^(\d+)\.sock$/.exec(name);
        if (m && !deps.isAlive(Number(m[1]))) unlink(path.join(dir, name));
    }
}
export function createGlen(pi: ExtensionAPI, deps: Deps) {
    const handle = { managed: deps.env.GLEN_MANAGED === "1", endpoint: () => { const e = ENDPOINTS.get(deps.pid); return e ? { socket: e.socket, registry: registry(e), sessionId: e.sessionId } : null; }, requests: () => ENDPOINTS.get(deps.pid)?.requests ?? [] };
    if (handle.managed) return handle;
    const z = pi.zod;
    pi.registerTool({ name: "glen_reply", label: "glen reply", description: "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress.", loadMode: "essential", approval: "read", parameters: z.object({ text: z.string().describe("The message for the calling agent."), final: z.boolean().optional().describe("true when this is your final answer to the request.") }).strict(),
        async execute(toolCallId: string, params: any, _signal: any, _onUpdate: any, ctx: any) {
            try {
                const args = parseReplyArgs(params), e = ENDPOINTS.get(deps.pid);
                if (e && (ctx.agent?.kind ?? "main") === "main") {
                    e.ctx = ctx;
                    try { identity(e); } catch (err) { trace(e, "registry_error", { reason: String(err) }); notify(ctx, `glen: registry refresh failed: ${String(err)}`); }
                }
                const active = (ctx.agent?.kind ?? "main") === "main" ? e?.active ?? [] : [];
                const id = e?.sessionId ?? ctx.sessionManager.getSessionId();
                // One tool call is one durable inbox record. A shared batch has
                // no single owning job; every observer receives this same record.
                const owner = active.length === 1 ? active[0] : null;
                const record = { v: 1, id: crypto.randomUUID(), session_id: id, job_id: owner?.jobId ?? null, request_id: owner?.id ?? null, tool_call_id: toolCallId ?? null, text: args.text, final: args.final, at: new Date().toISOString() };
                let failure: unknown;
                try {
                    if (!isPlainToken(id)) throw new Error("invalid session id");
                    const home = e?.home ?? resolveGlenHome(deps.env, BAKED_GLEN_HOME, deps.homedir);
                    fs.mkdirSync(path.join(home, "inbox"), { recursive: true, mode: 0o700 });
                    appendLine(path.join(home, "inbox", `${id}.ndjson`), JSON.stringify(record));
                } catch (err) { failure = err; }
                let streamed = false;
                for (const r of active) { if (event(r, "reply", { reply: record })) streamed = true; }
                if (e && active.length > 0) e.spanHasActivity = true;
                if (failure && !streamed) throw new Error(`glen_reply: cannot write the glen inbox: ${String(failure)}`);
                return { content: [{ type: "text", text: "delivered" }] };
            } catch (error) { return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], isError: true }; }
        }
    });
    const on = (name: string, fn: (payload: any, ctx: any, e?: Endpoint) => any) => {
        (pi.on as any)(name, async (payload: any, ctx: any) => {
            if ((ctx.agent?.kind ?? "main") !== "main") return;
            const e = ENDPOINTS.get(deps.pid);
            try {
                if (e) { e.ctx = ctx; e.seq++; try { identity(e, name === "session_switch" ? payload.reason : "moved"); } catch (err) { trace(e, "registry_error", { reason: String(err) }); } trace(e, name, payload); }
                await fn(payload, ctx, e);
            } catch (err) { trace(e, "handler_error", { reason: String(err) }); }
        });
    };
    on("session_start", async (_payload, ctx, existing) => {
        if (ctx.mode !== "tui" || existing) return;
        let home: string;
        try { home = resolveGlenHome(deps.env, BAKED_GLEN_HOME, deps.homedir); }
        catch (err) { notify(ctx, `glen: ${String(err)}`); return; }
        const id = ctx.sessionManager.getSessionId();
        if (!isPlainToken(id)) return;
        const socket = path.join(home, "run/live", `${deps.pid}.sock`);
        if (Buffer.byteLength(socket) >= sunPathMax(deps.platform)) { notify(ctx, "glen: GLEN_HOME is too long for a unix socket; live delivery is off"); return; }
        const uid = deps.uid !== undefined ? deps.uid : (process.getuid?.() ?? null);
        try {
            let problem = unsafeHome(home, uid);
            if (!problem) { fs.mkdirSync(path.dirname(socket), { recursive: true, mode: 0o700 }); problem = unsafeHome(home, uid); }
            if (problem) { notify(ctx, `glen: refusing GLEN_HOME: ${problem}; point GLEN_HOME at a private directory you own (mkdir -m 700); live delivery is off`); return; }
        } catch (err) { notify(ctx, `glen: cannot check GLEN_HOME: ${String(err)}; live delivery is off`); return; }
        sweep(home, deps); unlink(socket);
        const server = net.createServer(conn => { try { connection(e, conn); } catch { conn.destroy(); } });
        server.on("error", err => trace(e, "server_error", { reason: String(err) }));
        const e: Endpoint = { compacting: false, server, connections: new Set(), socket, home, sessionId: id, eventSessionId: id, sessionFile: ctx.sessionManager.getSessionFile() ?? null, ctx, requests: [], settledIds: new Set(), seq: 0, lastStart: 0, active: [], spanHasOutput: false, spanHasActivity: false, spanStart: 0, boundaryStarted: false, mode: ctx.mode, startedAt: new Date().toISOString(), ompVersion: null, deps, pi };
        try {
            await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(socket, () => { server.removeListener("error", reject); resolve(); }); });
            fs.chmodSync(socket, 0o600);
            e.ompVersion = await new Promise<string | null>(resolve => {
                const timer = ctx.setTimeout(() => resolve(null), 500);
                Promise.resolve().then(deps.ompVersion).then(v => { quietly(() => ctx.clearTimer(timer)); resolve(v); }, () => { quietly(() => ctx.clearTimer(timer)); resolve(null); });
            });
            publish(e, id, e.sessionFile); ENDPOINTS.set(deps.pid, e);
            trace(e, "session_start");
        } catch (err) { shutdown(e); notify(ctx, `glen: cannot start live delivery: ${String(err)}`); }
    });
    on("before_agent_start", () => {});
    on("turn_end", () => {});
    on("message_start", (p, _ctx, e) => {
        if (!e) return;
        if (isBoundary(p.message)) { boundary(e, p.message); e.boundaryStarted = true; }
        else if (p.message?.role === "assistant") e.spanHasActivity = true;
    });
    on("message_end", (p, _ctx, e) => {
        if (!e) return;
        if (isBoundary(p.message)) {
            if (!e.boundaryStarted) boundary(e, p.message);
            e.boundaryStarted = false;
            if (p.message.role === "user") {
                // sendUserMessage returns void. Match one pending injection, in submission order.
                const r = pendingRequest(e, p.message);
                if (r) { r.consumed = true; r.consumptionSeq = e.seq; if (e.active.length === 0) e.spanStart = e.lastStart; e.active.push(r); if (r.deliveryTimer !== undefined) e.ctx.clearTimer(r.deliveryTimer); event(r, "consumed"); }
            }
        } else if (p.message?.role === "assistant") { e.spanHasActivity = true; const hasText = !!textOf(p.message); if (hasText) e.spanHasOutput = true; for (const r of e.active) { r.lastAssistant = p.message; if (hasText) r.lastTextAssistant = p.message; } }
    });
    on("agent_start", (_p, _ctx, e) => { if (e) e.lastStart = e.seq; });
    on("agent_end", (p, _ctx, e) => { if (e && !p.willContinue && !p.awaitingAsyncWork) settle(e); });
    on("session_switch", (p, _ctx, e) => {
        // A hello/state may already have refreshed identity in the pre-event gap.
        if (e && (p.reason === "new" || p.reason === "resume")) {
            for (const r of [...e.requests]) if (r.sessionId !== e.ctx.sessionManager.getSessionId() || e.eventSessionId === e.ctx.sessionManager.getSessionId()) terminal(e, r, "error", { error: { code: "session_switched", message: "the omp session switched conversations" } });
        }
        if (e) e.eventSessionId = e.ctx.sessionManager.getSessionId();
    });
    on("session_branch", () => {});
    on("session_shutdown", (_p, _ctx, e) => { if (e) shutdown(e); });
    return handle;
}
function realDeps(): Deps {
    return { env: process.env, pid: process.pid, platform: process.platform, homedir: os.homedir(), idleMs: LIMITS.idleMs, isAlive(pid) { try { process.kill(pid, 0); return true; } catch (err: any) { return err.code !== "ESRCH"; } }, ompVersion: () => import("@oh-my-pi/pi-coding-agent").then(m => m.VERSION ?? null).catch(() => null) };
}
// Modules are rebound for subagents; distinct installed/-e copies must not register twice.
const COPY = Symbol.for("glen.extension");
const GLOBAL = globalThis as any;
const OWNER = {};
export default function glen(pi: ExtensionAPI) {
    if (process.env.GLEN_MANAGED === "1") return;
    if (GLOBAL[COPY] && GLOBAL[COPY] !== OWNER) return;
    if (!GLOBAL[COPY]) {
        GLOBAL[COPY] = OWNER;
        process.once("exit", () => { for (const e of ENDPOINTS.values()) { unlink(e.socket); removeRegistry(e); } });
    }
    createGlen(pi, realDeps());
}
