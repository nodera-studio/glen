import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import { createGlen } from "../index.ts";
let pid = 4000000;
export function createFakeHost(options: any = {}) {
    const handlers = new Map<string, Function[]>(), tools: any[] = [], sends: any[] = [], notes: string[] = [];
    const fields = { mode: "tui", agentKind: "main", sessionId: "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de", sessionFile: null, idle: true, pending: false, ...options };
    const timers = new Set<any>();
    const builder = () => { const b: any = { optional() { b.isOptional = true; return b; }, strict() { b.isStrict = true; return b; }, describe(text: string) { b.description = text; return b; } }; return b; };
    const pi: any = { on(name: string, fn: Function) { handlers.set(name, [...(handlers.get(name) ?? []), fn]); }, registerTool(t: any) { tools.push(t); }, sendUserMessage(text: string, opts?: any) { sends.push({ text, opts }); if (fields.throwSend) throw new Error("send failed"); fields.onSend?.(); }, zod: { object: builder, string: builder, boolean: builder } };
    const ctx: any = { mode: fields.mode, cwd: "/work/fixture", hasUI: true, agent: { kind: fields.agentKind }, ui: { notify: (text: string) => notes.push(text) }, sessionManager: { getSessionId: () => fields.sessionId, getSessionFile: () => fields.sessionFile }, isIdle: () => fields.idle, hasPendingMessages: () => fields.pending, setTimeout(fn: Function, ms: number) { const timer = setTimeout(() => { timers.delete(timer); fn(); }, ms); timers.add(timer); return timer; }, clearTimer(timer: any) { clearTimeout(timer); timers.delete(timer); } };
    async function emit(name: string, payload = {}) { for (const fn of handlers.get(name) ?? []) await fn(payload, ctx); }
    return { pi, ctx, fields, handlers, tools, sends, notes, emit, clear() { for (const t of timers) clearTimeout(t); } };
}
export async function fixture(options: any = {}, env: any = {}) {
    const home = fs.mkdtempSync("/tmp/glen-ts-");
    const host = createFakeHost(options);
    const deps: any = { env: { GLEN_HOME: home, ...env }, pid: ++pid, platform: process.platform, homedir: os.homedir(), isAlive: (p: number) => p === pid, ompVersion: async () => "18.14.0", idleMs: 100 };
    const handle = createGlen(host.pi, deps);
    return { ...host, home, deps, handle, async start() { await host.emit("session_start"); return handle.endpoint()!; }, async close() { await host.emit("session_shutdown"); host.clear(); fs.rmSync(home, { recursive: true, force: true }); } };
}
export async function client(socketPath: string, frame?: any) {
    const socket = net.createConnection(socketPath); socket.on("error", () => {});
    await new Promise<void>((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject); });
    const frames: any[] = []; let buffer = ""; let ended = false;
    socket.on("data", chunk => { buffer += chunk; while (buffer.includes("\n")) { const end = buffer.indexOf("\n"); frames.push(JSON.parse(buffer.slice(0, end))); buffer = buffer.slice(end + 1); } });
    socket.on("close", () => { ended = true; });
    if (frame !== undefined) socket.write(typeof frame === "string" ? frame : JSON.stringify(frame) + "\n");
    return { socket, frames, get ended() { return ended; }, async until(predicate: () => boolean, ms = 2000) { const end = Date.now() + ms; while (!predicate()) { if (Date.now() >= end) throw new Error("client deadline: " + JSON.stringify(frames)); await new Promise(r => setTimeout(r, 5)); } return frames; } };
}
export async function request(socket: string, frame: any) { const c = await client(socket, frame); await c.until(() => c.ended); return c.frames; }
export function frame(op: string, args: any = {}, id = "r") { return { v: 1, id, op, args }; }
