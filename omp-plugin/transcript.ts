/**
 * The no-replay guard of the herdr-kit restart bridge, kept free of OMP imports so it can
 * be tested on its own.
 *
 * `/restart` always relaunches with `--restore <session file>`, and the new image then goes
 * on with whatever its rules call unfinished: the session's own branch
 * (`AgentSession.restoreInterruptedTurn`) and every subagent transcript below the session
 * (`task/restore.ts`). Neither can be switched off, so the bridge restarts only a session
 * where no transcript could be unfinished by a rule stricter than OMP's (see
 * {@link settled}).
 */
import * as fs from "node:fs";
import * as path from "node:path";

export type Entry = {
	type?: string;
	customType?: string;
	data?: { kind?: unknown; reason?: unknown; recordedAt?: unknown };
	message?: { role?: string; stopReason?: string; content?: unknown };
};

/** Every record of a transcript but its header, or undefined when any line is not JSON. */
export function readEntries(file: string): Entry[] | undefined {
	const entries: Entry[] = [];
	for (const line of fs.readFileSync(file, "utf8").split("\n")) {
		if (!line.trim()) continue;
		try {
			const entry: Entry = JSON.parse(line);
			if (entry?.type !== "session") entries.push(entry);
		} catch {
			return undefined;
		}
	}
	return entries;
}

/**
 * Index of the last record that can carry a turn: a `message` or a `custom_message` prompt.
 * An idle eval context snapshot (`custom_message` of type `eval-state-context`) is passed
 * over, as OMP's own tail search passes over it.
 */
function lastTurnRecord(entries: readonly Entry[]): number {
	for (let i = entries.length - 1; i >= 0; i--) {
		const entry = entries[i];
		if (entry?.type === "custom_message" && entry.customType === "eval-state-context") continue;
		if (entry?.type === "message" || entry?.type === "custom_message") return i;
	}
	return -1;
}

function normalExit(entry: Entry): boolean {
	return (
		entry?.type === "custom" &&
		entry.customType === "session_exit" &&
		entry.data?.kind === "normal" &&
		typeof entry.data.reason === "string" &&
		typeof entry.data.recordedAt === "string"
	);
}

/**
 * A transcript `--restore` has nothing to go on with, by a rule stricter than OMP's own
 * (`hasRestorableTurn`): its last turn-carrying record is a finished answer (an assistant
 * `stop` without tool calls), or a normal `session_exit` comes after that record. Anything
 * else (a prompt, tool calls or results, an aborted or failed answer, an extension prompt,
 * a context snapshot) counts as unfinished, whether or not OMP would go on with it.
 */
export function settled(entries: readonly Entry[]): boolean {
	const last = lastTurnRecord(entries);
	if (last < 0) return true;
	const tail = entries[last]!;
	if (tail.type === "message" && tail.message?.role === "assistant" && tail.message.stopReason === "stop") {
		const content = Array.isArray(tail.message.content) ? tail.message.content : [];
		if (!content.some(block => !!block && typeof block === "object" && "type" in block && block.type === "toolCall")) {
			return true;
		}
	}
	return entries.slice(last + 1).some(normalExit);
}

/** Subagent transcripts, a superset of what `--restore` scans: every `*.jsonl` below `<session file stem>/`. */
function subagentTranscripts(sessionFile: string): string[] {
	const found: string[] = [];
	const walk = (dir: string) => {
		let names: fs.Dirent[];
		try {
			names = fs.readdirSync(dir, { withFileTypes: true });
		} catch {
			return;
		}
		for (const entry of names) {
			const full = path.join(dir, entry.name);
			if (entry.isDirectory()) walk(full);
			else if (entry.isFile() && entry.name.endsWith(".jsonl")) found.push(full);
		}
	};
	walk(sessionFile.replace(/\.jsonl$/, ""));
	return found;
}

/**
 * What `--restore` might go on with after a restart of the session whose current branch is
 * `branch`: `restorable_tail`, `restorable_subagent` (also for a subagent transcript that
 * cannot be read whole), or undefined when nothing.
 */
export function replayRisk(branch: readonly Entry[], sessionFile: string): string | undefined {
	if (!settled(branch)) return "restorable_tail";
	for (const file of subagentTranscripts(sessionFile)) {
		let entries: Entry[] | undefined;
		try {
			entries = readEntries(file);
		} catch {
			entries = undefined;
		}
		if (!entries || !settled(entries)) return "restorable_subagent";
	}
	return undefined;
}
