// Common Measure's Pi extension. Written by `commonmeasure install pi`,
// removed by `commonmeasure uninstall pi`; `commonmeasure doctor pi` reads
// it back. Edits here are overwritten by the next install.
//
// Pi has no MCP client of its own, so this extension is the client: at
// session start it spawns the mediated MCP server from the binary named
// below, lists its tools over stdio JSON-RPC and registers each with Pi
// under the same name, so the agent calls context_fetch, context_search
// and context_status the way it does on any other host. Every call is
// carried by the server, which rules on it under operator policy and
// records it under Pi's own session id.
//
// A server that cannot start leaves Pi without the mediated tools and says
// so once; nothing here stands in for the server or answers in its place.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type, type TSchema } from "typebox";
import { spawn, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";

const BINARY = "__COMMONMEASURE_BINARY__";

type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void };

class Mediator {
	private child: ChildProcess | undefined;
	private next = 1;
	private readonly pending = new Map<number, Pending>();

	constructor(
		private readonly cwd: string,
		private readonly sessionId: string | undefined,
	) {}

	async start(): Promise<Array<Record<string, unknown>>> {
		const args = ["mcp", "--host", "pi"];
		if (this.sessionId) {
			args.push("--session", this.sessionId);
		}
		// The server inherits stderr, so its own notes reach Pi's log; stdout
		// is the protocol and is read line by line.
		const child = spawn(BINARY, args, { cwd: this.cwd, stdio: ["pipe", "pipe", "inherit"] });
		this.child = child;
		const exited = new Promise<never>((_, reject) => {
			child.once("error", (error) => reject(new Error(`${BINARY}: ${error.message}`)));
			child.once("exit", (code, signal) =>
				reject(new Error(`${BINARY} mcp exited (${code ?? signal ?? "unknown"}) before answering`)),
			);
		});
		const lines = createInterface({ input: child.stdout! });
		lines.on("line", (line) => this.receive(line));
		const handshake = (async () => {
			await this.request("initialize", {
				protocolVersion: "2025-06-18",
				capabilities: {},
				clientInfo: { name: "pi", version: "extension" },
			});
			const listed = (await this.request("tools/list", {})) as { tools?: Array<Record<string, unknown>> };
			return listed.tools ?? [];
		})();
		return Promise.race([handshake, exited]);
	}

	async call(name: string, args: unknown): Promise<{ content?: Array<{ type: string; text: string }>; isError?: boolean }> {
		return (await this.request("tools/call", { name, arguments: args })) as {
			content?: Array<{ type: string; text: string }>;
			isError?: boolean;
		};
	}

	stop(): void {
		this.child?.kill();
		this.child = undefined;
		for (const pending of this.pending.values()) {
			pending.reject(new Error("the mediated server was stopped"));
		}
		this.pending.clear();
	}

	private request(method: string, params: unknown): Promise<unknown> {
		const child = this.child;
		if (!child?.stdin) {
			return Promise.reject(new Error("the mediated server is not running"));
		}
		const id = this.next++;
		return new Promise((resolve, reject) => {
			this.pending.set(id, { resolve, reject });
			child.stdin!.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
		});
	}

	private receive(line: string): void {
		let message: { id?: number; result?: unknown; error?: { message?: string } };
		try {
			message = JSON.parse(line);
		} catch {
			return;
		}
		if (typeof message.id !== "number") {
			return;
		}
		const pending = this.pending.get(message.id);
		if (!pending) {
			return;
		}
		this.pending.delete(message.id);
		if (message.error) {
			pending.reject(new Error(message.error.message ?? "the mediated server returned an error"));
		} else {
			pending.resolve(message.result);
		}
	}
}

// The server describes each tool's arguments as JSON Schema; Pi wants a
// TypeBox schema. The three tools use flat objects of strings, integers and
// booleans, which is what is mapped; anything else is passed through as an
// unchecked value so the server, which validates its own arguments, still
// sees it.
function schemaOf(schema: unknown): TSchema {
	const object = schema as { type?: string; properties?: Record<string, Record<string, unknown>>; required?: string[] };
	if (object?.type !== "object" || !object.properties) {
		return Type.Object({});
	}
	const required = new Set(object.required ?? []);
	const fields: Record<string, TSchema> = {};
	for (const [name, property] of Object.entries(object.properties)) {
		const description = typeof property.description === "string" ? { description: property.description } : {};
		let field: TSchema;
		switch (property.type) {
			case "string":
				field = Type.String(description);
				break;
			case "integer":
				field = Type.Integer(description);
				break;
			case "number":
				field = Type.Number(description);
				break;
			case "boolean":
				field = Type.Boolean(description);
				break;
			default:
				field = Type.Unknown(description);
		}
		fields[name] = required.has(name) ? field : Type.Optional(field);
	}
	return Type.Object(fields);
}

export default function (pi: ExtensionAPI) {
	let mediator: Mediator | undefined;

	pi.on("session_start", async (_event, ctx) => {
		mediator?.stop();
		const sessionId = typeof ctx.sessionManager?.getSessionId === "function" ? ctx.sessionManager.getSessionId() : undefined;
		const started = new Mediator(ctx.cwd, sessionId);
		mediator = started;
		let tools: Array<Record<string, unknown>>;
		try {
			tools = await started.start();
		} catch (error) {
			ctx.ui.notify(`Common Measure: the mediated tools are unavailable: ${(error as Error).message}`, "error");
			return;
		}
		for (const tool of tools) {
			const name = String(tool.name);
			pi.registerTool({
				name,
				label: `Common Measure ${name}`,
				description: String(tool.description ?? ""),
				promptSnippet: `${name}: ${String(tool.description ?? "").split(". ")[0]}`,
				parameters: schemaOf(tool.inputSchema),
				async execute(_toolCallId, params) {
					const result = await started.call(name, params);
					const content = (result.content ?? []).filter((block) => block.type === "text");
					return {
						content: content.length > 0 ? content : [{ type: "text", text: JSON.stringify(result) }],
						details: { isError: result.isError === true },
					};
				},
			});
		}
	});

	pi.on("session_shutdown", async () => {
		mediator?.stop();
		mediator = undefined;
	});
}
