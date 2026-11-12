// Win Island for opencode: permission questions as a card with buttons on
// the island, and a notice when a session goes idle.
// Install: copy to ~/.config/opencode/plugins/win-island.js
// Windows: island-hook.exe (cargo build --release). Linux: installed
// /usr/bin/island-hook, then a local developer build. WIN_ISLAND_HOOK overrides
// either default when the executable lives somewhere else.
import { spawn } from "node:child_process"
import { accessSync, constants } from "node:fs"
import { homedir } from "node:os"
import { basename, join } from "node:path"

function defaultHook() {
  const checkout = join(homedir(), "Documents", "win-island")
  if (process.platform !== "linux") return join(checkout, "target", "release", "island-hook.exe")
  const candidates = [
    "/usr/bin/island-hook",
    join(checkout, "crates", "tidemark-agent", "target", "release", "island-hook"),
    join(checkout, "target", "release", "island-hook"),
  ]
  return candidates.find((path) => {
    try { accessSync(path, constants.X_OK); return true } catch { return false }
  }) ?? "island-hook"
}

const HOOK = process.env.WIN_ISLAND_HOOK ?? defaultHook()

// Characters a card may show next to ✓ (same rule as island-hook, which
// checks it again): printable ASCII, the Russian alphabet, « » ° § € №,
// and no word mixing Latin and Cyrillic letters. Everything else leaves
// only Терминал / Нет.
const SHOWN_WHOLE = /^[\x20-\x7E\u00A7\u00AB\u00B0\u00BB\u0401\u0410-\u044F\u0451\u20AC\u2116]*$/u
const mixedScript = (s) => s.split(/[^\p{L}]/u).some((w) => /[A-Za-z]/.test(w) && /[\u0400-\u04FF]/.test(w))
const fitsIn = (s, max) => SHOWN_WHOLE.test(s) && !mixedScript(s) && s === s.trim() && [...s].length <= max

/** Sends one message to the island; resolves to the pressed button or null. */
function island(msg, waitSeconds = 50) {
  return new Promise((resolve) => {
    let out = ""
    let child
    try {
      child = spawn(HOOK, ["--raw", "--wait", String(waitSeconds)], { windowsHide: true })
    } catch {
      return resolve(null)
    }
    child.on("error", () => resolve(null))
    child.stdout.on("data", (d) => (out += d))
    child.on("close", () => {
      try {
        resolve(JSON.parse(out).choice ?? null)
      } catch {
        resolve(null)
      }
    })
    child.stdin.end(JSON.stringify({ source: "opencode", ...msg }))
  })
}

export const WinIsland = async ({ directory }) => {
  const project = basename(directory ?? "")
  const where = (s) => [project, s].filter(Boolean).join(": ")
  return {
    "permission.ask": async (input, output) => {
      const pattern = Array.isArray(input.pattern) ? input.pattern.join(" ") : (input.pattern ?? "")
      const text = where(pattern)
      // ✓ only when the card shows all of it; "Терминал" leaves the
      // question to opencode's own prompt.
      const title = input.title || "Разрешить?"
      const complete = pattern !== "" && fitsIn(text, 160) && fitsIn(title, 80)
      const choice = await island({
        kind: "ask",
        title,
        text,
        options: complete ? ["Терминал", "Нет", "✓"] : ["Терминал", "Нет"],
      })
      if (complete && choice === 2) output.status = "allow"
      else if (choice === 1) output.status = "deny"
    },
    event: async ({ event }) => {
      if (event.type === "session.idle") await island({ kind: "notice", title: "opencode закончил", text: where("") })
    },
  }
}
