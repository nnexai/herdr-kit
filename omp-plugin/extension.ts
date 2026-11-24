/**
 * herdr-kit restart bridge for OMP.
 *
 * `herdr-kit omp restart` asks a live session to run OMP's own `/restart` after an
 * upgrade. The session decides, in one synchronous step on its own event loop, whether
 * that is safe: nothing running or queued, no background job, an empty composer that has
 * keyboard focus, a saved transcript, a launcher that now names the requested executable,
 * and no transcript (its own or a subagent's) that `/restart`'s `--restore` could go on
 * with (`transcript.ts`, a rule stricter than OMP's own). Only then does it submit
 * `/restart` to its own composer. Terminal input cannot interleave with that step, so a
 * draft is never submitted and an open dialog or picker never receives the Enter.
 *
 * OMP 19.1 offers extensions no restart call (can1357/oh-my-pi#15236). The submit goes
 * through pi-tui's public `TUI.injectDebugInput`, which OMP documents as debug and test
 * input, not as an extension contract; every member used is checked first and the bridge
 * refuses when one is missing. Replace it with `ctx.restart()` once OMP ships one.
 *
 * The channel is a private directory, `$XDG_RUNTIME_DIR/herdr-kit/omp` (mode 0700, owned
 * by this user; anything else disables the bridge). Each interactive main session keeps
 * `<pid>.json` there describing itself, takes a `<pid>.request.json` addressed to its exact
 * process incarnation, and answers in `<pid>.reply.json`. Linux only.
 */
import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@oh-my-pi/pi-coding-agent";
import type { TUI } from "@oh-my-pi/pi-tui";
import { type Entry, replayRisk } from "./transcript.ts";

const PROTOCOL = 1;
const POLL_MS = 500;
const SETTLED_AFTER_MS = 30_000;
const WIDGET_KEY = "herdr-kit-omp";
const RESTART = "/restart";
/** One id per loaded image: a successful restart shows up as the same pid with a new instance. */
const INSTANCE = randomUUID();
const LOADED_AT = new Date().toISOString();

interface Request {
	protocol: number;
	nonce: string;
	pid: number;
	instance: string;
	session_file: string;
	target: { dev: number; ino: number };
	expires_at_ms: number;
}

function channelDir(): string | undefined {
	if (process.platform !== "linux") return undefined;
	const runtime = process.env.XDG_RUNTIME_DIR;
	if (!runtime || !path.isAbsolute(runtime)) return undefined;
	return path.join(runtime, "herdr-kit", "omp");
}

/** Create the channel if needed and accept it only when it is a private directory of ours. */
function privateDir(dir: string): boolean {
	try {
		fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
		const uid = process.getuid?.();
		for (const d of [path.dirname(dir), dir]) {
			const st = fs.lstatSync(d);
			if (!st.isDirectory() || st.uid !== uid || (st.mode & 0o077) !== 0) return false;
		}
		return true;
	} catch {
		return false;
	}
}

/** Kernel start time of this process (clock ticks since boot); unchanged across exec. */
function startTime(): string | undefined {
	try {
		const stat = fs.readFileSync("/proc/self/stat", "utf8");
		return stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19];
	} catch {
		return undefined;
	}
}

/** Remove channel files of processes that are gone: a killed session never removes its own. */
function sweep(dir: string): void {
	for (const name of fs.readdirSync(dir)) {
		const pid = Number(/^(\d+)\./.exec(name)?.[1]);
		if (!pid || pid === process.pid) continue;
		try {
			process.kill(pid, 0);
		} catch (error) {
			const code = error && typeof error === "object" && "code" in error ? error.code : undefined;
			if (code === "ESRCH") fs.rmSync(path.join(dir, name), { force: true });
		}
	}
}

function writeAtomic(file: string, value: unknown): void {
	const tmp = `${file}.${process.pid}.${randomUUID()}.tmp`;
	fs.writeFileSync(tmp, `${JSON.stringify(value)}\n`, { mode: 0o600 });
	fs.renameSync(tmp, file);
}

/**
 * The executable `/restart` will exec, resolved as OMP resolves its launcher: argv0 when it
 * is an absolute path, else its PATH lookup. Stat follows symlinks, so an upgraded launcher
 * symlink reports the new image.
 */
function launcher(): { path: string; dev: number; ino: number } | undefined {
	const argv0 = process.argv0;
	const resolved = argv0.includes("/") ? (path.isAbsolute(argv0) ? argv0 : undefined) : Bun.which(argv0);
	if (!resolved) return undefined;
	try {
		const st = fs.statSync(resolved);
		return { path: resolved, dev: st.dev, ino: st.ino };
	} catch {
		return undefined;
	}
}

function hasText(component: unknown): component is { getText(): string } {
	return !!component && typeof component === "object" && "getText" in component && typeof component.getText === "function";
}

export default function herdrKitOmp(pi: ExtensionAPI) {
	pi.setLabel("herdr-kit restart bridge");
	const dir = channelDir();
	if (!dir) return;
	const presenceFile = path.join(dir, `${process.pid}.json`);
	const requestFile = path.join(dir, `${process.pid}.request.json`);
	const replyFile = path.join(dir, `${process.pid}.reply.json`);
	const started = startTime();
	const loadedAt = Date.now();

	let live: ExtensionContext | undefined;
	let tui: TUI | undefined;
	let usable = false;
	let polling = false;
	let lastPresence = "";
	let inputPending = false;
	/** Agent runs that began without a person's submission since this image loaded. */
	let unpromptedRuns = 0;
	let riskKey = "";
	let riskNow: string | undefined;

	pi.on("input", () => {
		inputPending = true;
		return undefined;
	});
	pi.on("agent_start", () => {
		if (!inputPending) unpromptedRuns++;
		inputPending = false;
	});

	/** What `/restart`'s `--restore` might go on with in this session ({@link replayRisk}). */
	function risk(ctx: ExtensionContext, sessionFile: string): string | undefined {
		return replayRisk(ctx.sessionManager.getBranch() as Entry[], sessionFile);
	}

	function snapshot(ctx: ExtensionContext) {
		const sessionFile = ctx.sessionManager.getSessionFile();
		const jobs = ctx.getAsyncJobSnapshot();
		// Rescan only when settled and something was appended since: busy sessions are skipped
		// anyway, and the request itself always rescans.
		const key = `${sessionFile}|${ctx.sessionManager.getLeafId()}`;
		if (sessionFile && ctx.isIdle() && jobs?.running.length === 0 && key !== riskKey) {
			riskKey = key;
			riskNow = risk(ctx, sessionFile);
		}
		return {
			protocol: PROTOCOL,
			pid: process.pid,
			start_time: started,
			instance: INSTANCE,
			loaded_at: LOADED_AT,
			session_id: ctx.sessionManager.getSessionId(),
			session_file: sessionFile ?? null,
			session_saved: sessionFile ? fs.existsSync(sessionFile) : false,
			model: ctx.model ? `${ctx.model.provider}/${ctx.model.id}` : null,
			thinking: pi.getThinkingLevel() ?? null,
			idle: ctx.isIdle(),
			queued: ctx.hasPendingMessages(),
			running_jobs: jobs?.running.length ?? 0,
			unprompted_runs: unpromptedRuns,
			composer_empty: ctx.ui.getEditorText() === "",
			launcher: launcher() ?? null,
			replay_risk: riskNow ?? null,
		};
	}

	function publish(ctx: ExtensionContext): void {
		const text = JSON.stringify(snapshot(ctx));
		if (text === lastPresence) return;
		writeAtomic(presenceFile, JSON.parse(text));
		lastPresence = text;
	}

	function reply(nonce: string | null, outcome: "restarting" | "refused", reason?: string, before?: unknown): void {
		writeAtomic(replyFile, { protocol: PROTOCOL, nonce, pid: process.pid, instance: INSTANCE, outcome, reason, before });
	}

	/** Why this process must not restart for `req` right now, or undefined when it may. */
	function refusal(ctx: ExtensionContext, req: Request): string | undefined {
		if (req.protocol !== PROTOCOL) return "protocol_mismatch";
		if (req.pid !== process.pid || req.instance !== INSTANCE) return "not_this_process";
		if (!(req.expires_at_ms > Date.now())) return "request_expired";
		const sessionFile = ctx.sessionManager.getSessionFile();
		if (!sessionFile || sessionFile !== req.session_file) return "session_changed";
		if (!fs.existsSync(sessionFile)) return "session_not_saved";
		if (!ctx.isIdle()) return "busy";
		if (ctx.hasPendingMessages()) return "queued_messages";
		const jobs = ctx.getAsyncJobSnapshot();
		if (!jobs) return "jobs_unknown";
		if (jobs.running.length > 0) return "background_jobs";
		if (ctx.ui.getEditorText() !== "") return "draft";
		// A freshly started TUI may still hold keystrokes back; an Enter would queue behind them.
		if (Date.now() - loadedAt < SETTLED_AFTER_MS) return "too_soon";
		const exe = launcher();
		if (!exe) return "launcher_unresolved";
		if (exe.dev !== req.target.dev || exe.ino !== req.target.ino) return "launcher_not_target";
		if (!tui || typeof tui.getFocused !== "function" || typeof tui.injectDebugInput !== "function") {
			return "composer_unavailable";
		}
		// `/restart` relaunches with `--restore`; refuse while it would find work to go on with.
		return risk(ctx, sessionFile);
	}

	function serve(ctx: ExtensionContext): void {
		if (!fs.existsSync(requestFile)) return;
		let req: Request;
		try {
			req = JSON.parse(fs.readFileSync(requestFile, "utf8"));
		} catch {
			fs.rmSync(requestFile, { force: true });
			return reply(null, "refused", "bad_request");
		}
		// Consume first: a request is answered at most once, by this image only.
		fs.rmSync(requestFile, { force: true });
		const nonce = typeof req.nonce === "string" ? req.nonce : null;
		const refused = refusal(ctx, req);
		if (refused) return reply(nonce, "refused", refused);
		const before = snapshot(ctx);
		// Everything from here to the Enter runs without yielding, so no keystroke lands between.
		ctx.ui.setEditorText(RESTART);
		const focused = tui!.getFocused();
		if (!hasText(focused) || focused.getText() !== RESTART) {
			ctx.ui.setEditorText("");
			return reply(nonce, "refused", "composer_not_focused");
		}
		reply(nonce, "restarting", undefined, before);
		tui!.injectDebugInput("\r");
		// OMP takes the submission synchronously; anything left in the composer means it did not.
		if (ctx.ui.getEditorText() !== "") {
			ctx.ui.setEditorText("");
			reply(nonce, "refused", "submit_not_accepted");
		}
	}

	function tick(): void {
		const ctx = live;
		if (!ctx || !usable) return;
		try {
			publish(ctx);
			serve(ctx);
		} catch (error) {
			pi.logger?.warn?.(`herdr-kit-omp: ${error instanceof Error ? error.message : String(error)}`);
		}
	}

	function attach(ctx: ExtensionContext): void {
		if (ctx.mode !== "tui" || !ctx.hasUI || ctx.agent?.kind !== "main") return;
		live = ctx;
		usable = privateDir(dir!);
		if (!usable) return;
		sweep(dir!);
		// A zero-row widget hands over the TUI so the composer's focus can be checked.
		ctx.ui.setWidget(WIDGET_KEY, ui => {
			tui = ui;
			return { render: () => [], invalidate() {} };
		});
		lastPresence = "";
		tick();
		if (!polling) {
			polling = true;
			ctx.setInterval(tick, POLL_MS);
		}
	}

	pi.on("session_start", (_event, ctx) => attach(ctx));
	pi.on("session_switch", (_event, ctx) => attach(ctx));
	pi.on("session_shutdown", () => {
		if (!usable) return;
		try {
			const current = JSON.parse(fs.readFileSync(presenceFile, "utf8"));
			if (current.instance === INSTANCE) fs.rmSync(presenceFile, { force: true });
		} catch {}
	});
}
