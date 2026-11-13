// 实测 omp RPC：发一条消息，把收到的全部帧写进日志，并打印事件摘要。
// 会调用真实模型，消耗订阅额度。
//
//   bun scripts/omp-probe.ts --model claude/claude-sonnet-5-5 "列出当前目录的文件"
//   bun scripts/omp-probe.ts --cwd /path/to/project --approve "创建文件 hello.txt"
//
// --model    magpie 提供方下的模型 ID，不传则用 omp 的默认模型
// --cwd      omp 的工作目录，默认是一个临时目录
// --approval omp 的审批模式：always-ask、always-ask-cached、write、yolo，不传则用 omp 的配置
// --approve  自动同意工具审批和 confirm 请求；不加则对所有需要回复的请求回「取消」
// --out      日志路径，默认 omp-probe.jsonl

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";

const { values, positionals } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		model: { type: "string" },
		cwd: { type: "string" },
		approval: { type: "string" },
		approve: { type: "boolean", default: false },
		out: { type: "string", default: "omp-probe.jsonl" },
	},
	allowPositionals: true,
});

const message = positionals.join(" ");
if (!message) {
	console.error("用法：bun scripts/omp-probe.ts [--model ID] [--cwd DIR] [--approval MODE] [--approve] <消息>");
	process.exit(1);
}

const cwd = values.cwd ?? mkdtempSync(join(tmpdir(), "omp-probe-"));
const log = Bun.file(values.out).writer();
const argv = ["omp", "--mode", "rpc-ui"];
if (values.approval) argv.push("--approval-mode", values.approval);
const proc = Bun.spawn(argv, { cwd, stdin: "pipe", stdout: "pipe", stderr: "inherit" });

type Frame = Record<string, unknown> & { type: string };

function send(frame: Record<string, unknown>) {
	log.write(`> ${JSON.stringify(frame)}\n`);
	proc.stdin.write(`${JSON.stringify(frame)}\n`);
}

// 需要回复的界面请求；其余的（notify、setStatus 等）只是通知。
const DIALOGS = new Set(["confirm", "select", "input", "editor", "ask"]);

function onFrame(frame: Frame) {
	switch (frame.type) {
		case "ready":
			send({ id: "proto", type: "negotiate_protocol", protocolVersion: 2 });
			if (values.model) send({ id: "model", type: "set_model", provider: "magpie", modelId: values.model });
			send({ id: "prompt", type: "prompt", message });
			break;
		case "message_update": {
			const event = frame.assistantMessageEvent as { type?: string; delta?: string } | undefined;
			if (event?.type === "text_delta" && event.delta) process.stdout.write(event.delta);
			break;
		}
		case "extension_ui_request": {
			const method = String(frame.method);
			console.log(`\n[界面请求] ${method} ${JSON.stringify(frame).slice(0, 400)}`);
			if (!DIALOGS.has(method)) break;
			// 工具审批是一条 select，选项里有 "Approve"。
			const canApprove = method === "select" && (frame.options as string[] | undefined)?.includes("Approve");
			if (values.approve && canApprove) send({ type: "extension_ui_response", id: frame.id, value: "Approve" });
			else if (values.approve && method === "confirm") send({ type: "extension_ui_response", id: frame.id, confirmed: true });
			else send({ type: "extension_ui_response", id: frame.id, cancelled: true });
			break;
		}
		case "response":
			console.log(`\n[响应] ${frame.command} ${frame.success ? "成功" : `失败：${frame.error}`}`);
			break;
		case "prompt_result":
			console.log(`\n[本轮结束] ${frame.status}`);
			if (frame.sessionSettled) proc.stdin.end();
			break;
		case "session_settled":
			proc.stdin.end();
			break;
		case "rpc_chunk":
			console.log("\n[分片帧] 这个脚本不拼接分片，完整内容见日志");
			break;
		default:
			console.log(`\n[事件] ${frame.type}`);
	}
}

const counts = new Map<string, number>();
const decoder = new TextDecoder();
let buffer = "";
for await (const chunk of proc.stdout) {
	buffer += decoder.decode(chunk, { stream: true });
	let newline = buffer.indexOf("\n");
	while (newline >= 0) {
		const line = buffer.slice(0, newline);
		buffer = buffer.slice(newline + 1);
		newline = buffer.indexOf("\n");
		if (!line.trim()) continue;
		log.write(`${line}\n`);
		const frame = JSON.parse(line) as Frame;
		counts.set(frame.type, (counts.get(frame.type) ?? 0) + 1);
		onFrame(frame);
	}
}

await log.end();
console.log(`\n退出码 ${await proc.exited}，工作目录 ${cwd}，日志 ${values.out}`);
console.log([...counts].map(([type, n]) => `${type}×${n}`).join("  "));
