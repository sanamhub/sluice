// Writes `src/data/mcp-tools.json` from the tool list a real `sluice mcp` server answers, so the
// agents page cannot describe tools the binary does not have. A test in `crates/sluice-cli` compares
// the file with the same list, so a change to a tool name or description fails CI until both move.
//
//   node scripts/mcp-tools.mjs [--binary path/to/sluice]
//
// The server speaks JSON-RPC over standard input and output, one message per line.

import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const argv = process.argv.slice(2);
const at = argv.indexOf("--binary");
const binary = at >= 0 ? argv[at + 1] : "sluice";
const out = resolve(import.meta.dirname, "../src/data/mcp-tools.json");
const root = mkdtempSync(join(tmpdir(), "sluice-mcp-"));

const child = spawn(binary, ["mcp", "--loopback", "--root", root], {
    stdio: ["pipe", "pipe", "inherit"],
});

let buffer = "";
const waiting = new Map();

child.stdout.setEncoding("utf8");
child.stdout.on("data", (chunk) => {
    buffer += chunk;
    let newline;
    while ((newline = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (!line) continue;
        let message;
        try {
            message = JSON.parse(line);
        } catch {
            continue; // A log line, not a message.
        }
        const settle = waiting.get(message.id);
        if (settle) {
            waiting.delete(message.id);
            settle(message);
        }
    }
});

function send(message) {
    child.stdin.write(`${JSON.stringify(message)}\n`);
}

function request(id, method, params) {
    return new Promise((resolveMessage, reject) => {
        const timer = setTimeout(() => {
            waiting.delete(id);
            reject(new Error(`${method} was not answered in 20 s`));
        }, 20_000);
        waiting.set(id, (message) => {
            clearTimeout(timer);
            if (message.error)
                reject(new Error(`${method}: ${message.error.message}`));
            else resolveMessage(message.result);
        });
        send({ jsonrpc: "2.0", id, method, params });
    });
}

try {
    await request(1, "initialize", {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "sluice-site", version: "1" },
    });
    send({ jsonrpc: "2.0", method: "notifications/initialized" });
    const listed = await request(2, "tools/list", {});
    const tools = listed.tools
        .map((tool) => ({ name: tool.name, description: tool.description }))
        .sort((a, b) => a.name.localeCompare(b.name));
    writeFileSync(out, `${JSON.stringify(tools, null, 2)}\n`);
    console.log(`${tools.length} tools written to ${out}`);
} finally {
    child.stdin.end();
    child.kill();
}
