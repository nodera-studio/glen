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
export const OPS = ["hello", "state", "send"];
export const RESERVED_OPS = ["abort", "list_models", "set_model", "set_thinking_level", "compact"];
export const LIMITS = { maxLine: 4 * 1024 * 1024, maxText: 524288, maxReply: 65536, idleMs: 5000 };
type Deps = { env: Record<string, string | undefined>; pid: number; platform: string; homedir: string; isAlive: (pid: number) => boolean; ompVersion: () => Promise<string | null>; idleMs: number };
type Request = { id: string; jobId: string | null; text: string; deliveredAs: string; conn: net.Socket | null; ackSeq: number; acked: boolean; outbox: object[]; delivered: boolean; startedAfterAck: boolean; consumed: boolean; lastAssistant: any; settled: boolean };
type Endpoint = { server: net.Server; connections: Set<net.Socket>; socket: string; home: string; sessionId: string; sessionFile: string | null; ctx: any; requests: Request[]; settledIds: Set<string>; seq: number; lastStart: number; mode: string; startedAt: string; ompVersion: string | null; timer?: any; deps: Deps; pi: any };
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
function quietly(fn: () => void) { try { fn(); } catch { /* best effort cleanup/trace */ } }
function unlink(file: string) { quietly(() => fs.unlinkSync(file)); }
function version() { return GLEN_VERSION.startsWith("__") ? "dev" : GLEN_VERSION; }
function registry(e: Endpoint, id = e.sessionId) { return path.join(e.home, "run/live", `${id}.json`); }
function removeRegistry(e: Endpoint, id = e.sessionId) {
    if (!isPlainToken(id)) return;
    quietly(() => { const file = registry(e, id); if (JSON.parse(fs.readFileSync(file, "utf8")).pid === e.deps.pid) fs.unlinkSync(file); });
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
    quietly(() => fs.appendFileSync(e.deps.env.GLEN_EXT_TRACE!, JSON.stringify({ t: new Date().toISOString(), event, reason: payload.reason, willContinue: payload.willContinue, awaitingAsyncWork: payload.awaitingAsyncWork, idle: e.ctx.isIdle(), queued: e.ctx.hasPendingMessages(), requests: e.requests.map(r => ({ id: r.id, deliveredAs: r.deliveredAs, delivered: r.delivered, startedAfterAck: r.startedAfterAck, consumed: r.consumed })) }) + "\n", { mode: 0o600 }));
}
function notify(ctx: any, message: string) { quietly(() => { if (ctx.hasUI) ctx.ui.notify(message, "info"); }); }
function write(conn: net.Socket | null, frame: object): boolean {
    if (!conn || conn.destroyed || conn.writableEnded) return false;
    try { conn.write(JSON.stringify(frame) + "\n"); return true; } catch { return false; }
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
    if (r.acked && r.conn) { const conn = r.conn; quietly(() => conn.end(() => quietly(() => conn.destroy()))); }
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
        if (!isPlainToken(id) || reason === "new" || reason === "resume") terminal(e, r, "error", { error: { code: "session_switched", message: "the omp session switched conversations" } });
    }
}
function consumed(e: Endpoint) {
    for (const r of e.requests) if (!r.consumed && (r.delivered || (r.deliveredAs === "prompt" && r.startedAfterAck))) { r.consumed = true; event(r, "consumed"); }
}
function textOf(message: any): string { return typeof message?.content === "string" ? message.content : (Array.isArray(message?.content) ? message.content.filter((b: any) => b?.type === "text" && typeof b.text === "string").map((b: any) => b.text).join("\n") : ""); }
function settle(e: Endpoint) {
    if (e.timer !== undefined) e.ctx.clearTimer(e.timer);
    const endSeq = e.seq, started = Date.now();
    const poll = () => {
        try {
            if (!ENDPOINTS.has(e.deps.pid) || e.lastStart > endSeq || e.ctx.hasPendingMessages() || Date.now() - started >= 30000) { trace(e, "poll_stopped"); return; }
            if (!e.ctx.isIdle()) { e.timer = e.ctx.setTimeout(poll, 50); return; }
            for (const r of [...e.requests]) if (r.ackSeq < endSeq && (r.consumed || r.deliveredAs !== "prompt")) {
                const m = r.lastAssistant, stop = m?.stopReason ?? null;
                terminal(e, r, "settled", { status: stop === "aborted" ? "aborted" : stop === "error" ? "error" : "completed", assistant_text: m ? textOf(m) : null, stop_reason: stop, error_message: typeof m?.errorMessage === "string" ? m.errorMessage : null });
            }
            trace(e, "quiescent");
        } catch (error) { trace(e, "poll_error", { reason: String(error) }); }
    };
    e.timer = e.ctx.setTimeout(poll, 50);
}
function state(e: Endpoint, hello: boolean) {
    const busy = !e.ctx.isIdle();
    return { ...(hello ? { protocol: PROTOCOL, protocols: [PROTOCOL], ops: OPS } : {}), ext_version: version(), omp_version: e.ompVersion, pid: e.deps.pid, session_id: e.sessionId, session_file: e.sessionFile, mode: e.mode, state: busy ? "busy" : "idle", busy, queued: e.ctx.hasPendingMessages(), active_requests: e.requests.length };
}
function connection(e: Endpoint, conn: net.Socket) {
    e.connections.add(conn);
    const chunks: Buffer[] = [];
    let length = 0, handled = false;
    const error = (id: any, code: string, message: string, details?: object) => conn.end(JSON.stringify({ v: PROTOCOL, id, ok: false, error: { code, message, ...(details ? { details } : {}) } }) + "\n");
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
            if (frame.op === "hello" || frame.op === "state") { conn.end(JSON.stringify({ v: PROTOCOL, id, ok: true, data: state(e, frame.op === "hello") }) + "\n"); return; }
            if (frame.op !== "send") { error(id, "unsupported_op", "unsupported operation", { op: frame.op ?? null, ops: OPS, reserved: RESERVED_OPS.includes(frame.op) }); return; }
            if (e.requests.some(r => r.id === id) || e.settledIds.has(id)) { error(id, "bad_request", "duplicate request id"); return; }
            const a = frame.args;
            if (typeof a?.text !== "string" || !a.text.trim() || Buffer.byteLength(a.text) > LIMITS.maxText || !["auto", "steer", "follow_up"].includes(a.deliver_as) || (a.job_id != null && typeof a.job_id !== "string") || (a.expect_session_id != null && typeof a.expect_session_id !== "string")) { error(id, "bad_request", "invalid send arguments"); return; }
            if (a.expect_session_id != null && a.expect_session_id !== e.sessionId) { error(id, "session_mismatch", "session identity changed", { session_id: e.sessionId }); return; }
            const idle = e.ctx.isIdle(), deliveredAs = idle ? "prompt" : a.deliver_as === "steer" ? "steer" : "follow_up";
            const r: Request = { id, jobId: a.job_id ?? null, text: a.text, deliveredAs, conn, ackSeq: e.seq, acked: false, outbox: [], delivered: false, startedAfterAck: false, consumed: false, lastAssistant: null, settled: false };
            e.requests.push(r);
            conn.on("close", () => { r.conn = null; });
            try { if (idle) e.pi.sendUserMessage(a.text); else e.pi.sendUserMessage(a.text, { deliverAs: deliveredAs === "steer" ? "steer" : "followUp" }); }
            catch (err) { e.requests = e.requests.filter(x => x !== r); error(id, "send_failed", String(err)); return; }
            write(conn, { v: PROTOCOL, id, ok: true, data: { delivered_as: deliveredAs, session_id: e.sessionId, request_id: id } });
            r.acked = true; r.ackSeq = e.seq;
            for (const frame of r.outbox) write(conn, frame);
            r.outbox = [];
            if (r.settled) conn.end(() => quietly(() => conn.destroy()));
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
        if (name.endsWith(".json")) quietly(() => { const r = JSON.parse(fs.readFileSync(path.join(dir, name), "utf8")); if (Number.isInteger(r.pid) && r.pid > 0 && !deps.isAlive(r.pid)) { unlink(path.join(dir, name)); unlink(path.join(dir, `${r.pid}.sock`)); } });
        const m = /^(\d+)\.sock$/.exec(name);
        if (m && !deps.isAlive(Number(m[1]))) unlink(path.join(dir, name));
    }
}
export function createGlen(pi: ExtensionAPI, deps: Deps) {
    const handle = { managed: deps.env.GLEN_MANAGED === "1", endpoint: () => { const e = ENDPOINTS.get(deps.pid); return e ? { socket: e.socket, registry: registry(e), sessionId: e.sessionId } : null; }, requests: () => ENDPOINTS.get(deps.pid)?.requests ?? [] };
    if (handle.managed) return handle;
    let notified = false;
    const z = pi.zod;
    pi.registerTool({ name: "glen_reply", label: "glen reply", description: "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress.", loadMode: "essential", approval: "read", parameters: z.object({ text: z.string().describe("The message for the calling agent."), final: z.boolean().optional().describe("true when this is your final answer to the request.") }).strict(),
        async execute(toolCallId: string, params: any, _signal: any, _onUpdate: any, ctx: any) {
            try {
                const args = parseReplyArgs(params), e = ENDPOINTS.get(deps.pid);
                if (e && (ctx.agent?.kind ?? "main") === "main") { e.ctx = ctx; identity(e); }
                const active = e?.requests.filter(r => r.consumed && !r.settled).at(-1);
                const id = e?.sessionId ?? ctx.sessionManager.getSessionId();
                const record = { v: 1, id: crypto.randomUUID(), session_id: id, job_id: active?.jobId ?? null, request_id: active?.id ?? null, tool_call_id: toolCallId ?? null, text: args.text, final: args.final, at: new Date().toISOString() };
                let failure: unknown;
                try {
                    if (!isPlainToken(id)) throw new Error("invalid session id");
                    const home = e?.home ?? resolveGlenHome(deps.env, BAKED_GLEN_HOME, deps.homedir);
                    fs.mkdirSync(path.join(home, "inbox"), { recursive: true, mode: 0o700 });
                    fs.appendFileSync(path.join(home, "inbox", `${id}.ndjson`), JSON.stringify(record) + "\n", { mode: 0o600 });
                } catch (err) { failure = err; }
                const streamed = active ? event(active, "reply", { reply: record }) : false;
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
                if (e) { e.ctx = ctx; e.seq++; identity(e, name === "session_switch" ? payload.reason : "moved"); trace(e, name, payload); }
                await fn(payload, ctx, e);
            } catch (err) { trace(e, "handler_error", { reason: String(err) }); }
        });
    };
    on("session_start", async (_payload, ctx, existing) => {
        if (ctx.mode !== "tui" || existing) return;
        let home: string;
        try { home = resolveGlenHome(deps.env, BAKED_GLEN_HOME, deps.homedir); }
        catch (err) { if (!notified) notify(ctx, `glen: ${String(err)}`); notified = true; return; }
        const id = ctx.sessionManager.getSessionId();
        if (!isPlainToken(id)) return;
        const socket = path.join(home, "run/live", `${deps.pid}.sock`);
        if (Buffer.byteLength(socket) >= sunPathMax(deps.platform)) { if (!notified) notify(ctx, "glen: GLEN_HOME is too long for a unix socket; live delivery is off"); notified = true; return; }
        fs.mkdirSync(path.dirname(socket), { recursive: true, mode: 0o700 }); sweep(home, deps); unlink(socket);
        const server = net.createServer(conn => { try { connection(e, conn); } catch { conn.destroy(); } });
        server.on("error", err => trace(e, "server_error", { reason: String(err) }));
        const e: Endpoint = { server, connections: new Set(), socket, home, sessionId: id, sessionFile: ctx.sessionManager.getSessionFile() ?? null, ctx, requests: [], settledIds: new Set(), seq: 0, lastStart: 0, mode: ctx.mode, startedAt: new Date().toISOString(), ompVersion: null, deps, pi };
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
    on("before_agent_start", (p, _ctx, e) => { if (e && p.reason === "prompt" && typeof p.prompt === "string") { for (const r of e.requests) if (!r.delivered && p.prompt.includes(r.text.trim())) r.delivered = true; consumed(e); } });
    on("message_end", (p, _ctx, e) => {
        if (!e) return;
        if (p.message?.role === "user") { const r = e.requests.find(r => !r.delivered && r.text.trim() === textOf(p.message).trim()); if (r) r.delivered = true; consumed(e); }
        if (p.message?.role === "assistant") for (const r of e.requests) if (r.acked) r.lastAssistant = p.message;
    });
    on("turn_end", (p, _ctx, e) => { if (e && p.message?.role === "assistant") for (const r of e.requests) if (r.acked) r.lastAssistant = p.message; });
    on("agent_start", (_p, _ctx, e) => { if (e) { e.lastStart = e.seq; for (const r of e.requests) { /* Prompt-start fallback applies only after acknowledgement. */ if (r.acked) r.startedAfterAck = true; } consumed(e); } });
    on("agent_end", (p, _ctx, e) => { if (e && !p.willContinue && !p.awaitingAsyncWork) settle(e); });
    on("session_switch", () => {});
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
