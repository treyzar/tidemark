import test from "node:test"
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { runInNewContext } from "node:vm"
import { posix, win32 } from "node:path"
import { EventEmitter } from "node:events"
import { PassThrough } from "node:stream"

const source = readFileSync(new URL("../win-island.js", import.meta.url), "utf8")
// Evaluate the unchanged adapter body with offline OS/process boundaries. No child
// executable, real player, live agent, user environment or configuration is used.
function load({ platform = "linux", override, executable = [], choice = null } = {}) {
  const calls = []
  const paths = platform === "win32" ? win32 : posix
  const home = platform === "win32" ? "C:\\Users\\test" : "/home/test"
  const context = {
    process: { platform, env: override === undefined ? {} : { WIN_ISLAND_HOOK: override } },
    homedir: () => home,
    basename: paths.basename,
    join: paths.join,
    constants: { X_OK: 1 },
    accessSync(path) { if (!executable.includes(path)) throw new Error("missing") },
    spawn(path, args) {
      const child = new EventEmitter()
      child.stdout = new PassThrough()
      child.stdin = new PassThrough()
      let input = ""
      child.stdin.on("data", (d) => { input += d })
      child.stdin.on("finish", () => {
        calls.push({ path, args, message: JSON.parse(input) })
        child.stdout.write(JSON.stringify({ choice }))
        child.emit("close", 0)
      })
      return child
    },
  }
  const body = source.replace(/^import .* from .*$/gm, "").replace("export const WinIsland", "const WinIsland")
  runInNewContext(`${body}\nglobalThis.plugin = WinIsland`, context)
  return { plugin: context.plugin, calls }
}
async function permission(options = {}, input = { pattern: "ls", title: "Run?" }) {
  const adapter = load(options)
  const hooks = await adapter.plugin({ directory: options.platform === "win32" ? "C:\\project" : "/work/project" })
  const output = {}
  await hooks["permission.ask"](input, output)
  return { ...adapter, output }
}

test("Linux prefers installed executable over a developer checkout", async () => {
  const { calls } = await permission({ executable: ["/usr/bin/island-hook", "/home/test/Documents/win-island/crates/tidemark-agent/target/release/island-hook"] })
  assert.equal(calls[0].path, "/usr/bin/island-hook")
})
test("Linux uses standalone agent developer build when installed hook is absent", async () => {
  const path = "/home/test/Documents/win-island/crates/tidemark-agent/target/release/island-hook"
  const { calls } = await permission({ executable: [path] })
  assert.equal(calls[0].path, path)
})
test("Windows keeps the existing executable location", async () => {
  const { calls } = await permission({ platform: "win32" })
  assert.equal(calls[0].path, "C:\\Users\\test\\Documents\\win-island\\target\\release\\island-hook.exe")
})
test("explicit hook override wins on Linux and Windows", async () => {
  for (const platform of ["linux", "win32"]) {
    const { calls } = await permission({ platform, override: "/custom/hook", executable: ["/usr/bin/island-hook"] })
    assert.equal(calls[0].path, "/custom/hook")
  }
})
test("only explicit index 2 approves complete visible content", async () => {
  for (const choice of [null, 0, 1, 2, 3, -1, "2"]) {
    const { output, calls } = await permission({ choice })
    assert.equal(output.status, choice === 2 ? "allow" : choice === 1 ? "deny" : undefined)
    assert.deepEqual(calls[0].message.options, ["Терминал", "Нет", "✓"])
  }
})
test("hidden, truncated and mixed-script content never approves even a forged index 2", async () => {
  for (const pattern of ["ls\nrm -rf ~", "x".repeat(200), "pаypal", "echo\u202Eok", ""]) {
    const { calls, output } = await permission({ choice: 2 }, { pattern, title: "Run?" })
    assert.deepEqual(calls[0].message.options, ["Терминал", "Нет"])
    assert.equal(output.status, undefined)
  }
})

test("Linux supports a shared target dev build and PATH as final fallback", async () => {
  const path = "/home/test/Documents/win-island/target/release/island-hook"
  assert.equal((await permission({ executable: [path] })).calls[0].path, path)
  assert.equal((await permission()).calls[0].path, "island-hook")
})
test("a hidden or oversized title cannot approve; explicit refusal still works", async () => {
  for (const title of ["Run?\nHidden", "x".repeat(81), "pаypal"]) {
    const { output, calls } = await permission({ choice: 2 }, { pattern: "ls", title })
    assert.equal(output.status, undefined)
    assert.deepEqual(calls[0].message.options, ["Терминал", "Нет"])
  }
  assert.equal((await permission({ choice: 1 }, { pattern: "ls\nmore", title: "Run?" })).output.status, "deny")
})
test("idle notices use the selected Linux hook and canonical raw invocation", async () => {
  const adapter = load({ executable: ["/usr/bin/island-hook"] })
  const hooks = await adapter.plugin({ directory: "/work/project" })
  await hooks.event({ event: { type: "session.idle" } })
  assert.equal(adapter.calls.length, 1)
  assert.equal(adapter.calls[0].path, "/usr/bin/island-hook")
  assert.deepEqual(Array.from(adapter.calls[0].args), ["--raw", "--wait", "50"])
  assert.deepEqual(adapter.calls[0].message, { source: "opencode", kind: "notice", title: "opencode закончил", text: "project" })
})
