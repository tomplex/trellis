/**
 * trellis pane status
 * -------------------
 * Tells trellis what this pi session is doing so the session list can show
 * `✦ working…` / `✦ needs input` / `✦ idle` next to the tmux window and name
 * the window after the conversation's first message.
 *
 * Writes `~/.local/share/trellis/panes/<TMUX_PANE>.json` (leading `%` dropped):
 *
 *   { "agent": "pi", "pid": 12345, "state": "working", "title": "fix login" }
 *
 * `needs input` comes from pi's `ui_prompt_start`/`ui_prompt_end` events, so
 * any extension that blocks on `ctx.ui.confirm()` & co. (e.g. an approval
 * gate) is reported without knowing about trellis.
 *
 * Install: symlink or copy into `~/.pi/agent/extensions/`. No-op outside tmux.
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { mkdirSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

type State = "idle" | "working" | "prompting";

const STATUS_DIR = join(homedir(), ".local/share/trellis/panes");

function statusFile(): string | undefined {
	const pane = process.env.TMUX_PANE;
	if (!pane) return undefined;
	return join(STATUS_DIR, `${pane.replace(/^%/, "")}.json`);
}

/** First user message on the active branch, or undefined for a fresh session. */
function firstUserMessage(ctx: ExtensionContext): string | undefined {
	for (const entry of ctx.sessionManager.getBranch()) {
		if (entry.type !== "message" || entry.message.role !== "user") continue;
		const content = entry.message.content;
		if (typeof content === "string") return content;
		for (const part of content) {
			if (part.type === "text" && part.text.trim()) return part.text;
		}
	}
	return undefined;
}

export default function (pi: ExtensionAPI) {
	const file = statusFile();
	if (!file) return;

	let state: State = "idle";
	let prompting = false;
	let title: string | undefined;

	const write = () => {
		const body = JSON.stringify({
			agent: "pi",
			pid: process.pid,
			state: prompting ? "prompting" : state,
			title,
		});
		try {
			mkdirSync(STATUS_DIR, { recursive: true });
			// Write-then-rename so trellis never reads a torn file.
			const tmp = `${file}.${process.pid}.tmp`;
			writeFileSync(tmp, body);
			renameSync(tmp, file);
		} catch {
			// Status is advisory; never let it disturb the session.
		}
	};

	const remove = () => {
		try {
			unlinkSync(file);
		} catch {
			// Already gone.
		}
	};

	pi.on("session_start", (_event, ctx) => {
		state = "idle";
		prompting = false;
		title = firstUserMessage(ctx);
		write();
	});

	pi.on("input", (event) => {
		if (!title && event.text.trim()) {
			title = event.text;
			write();
		}
	});

	pi.on("agent_start", () => {
		state = "working";
		write();
	});

	pi.on("agent_end", () => {
		state = "idle";
		write();
	});

	pi.on("ui_prompt_start", () => {
		prompting = true;
		write();
	});

	pi.on("ui_prompt_end", () => {
		prompting = false;
		write();
	});

	pi.on("session_shutdown", (event) => {
		if (event.reason === "quit") remove();
	});
}
