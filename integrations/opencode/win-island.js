// Win Island for opencode: permission questions as a card with buttons on
// the island, and a notice when a session goes idle.
// Install: copy to ~/.config/opencode/plugins/win-island.js
// Needs island-hook.exe (cargo build --release); set WIN_ISLAND_HOOK if it
// lives somewhere else.
import { spawn } from "node:child_process"
import { homedir } from "node:os"
import { basename, join } from "node:path"

const HOOK =
  process.env.WIN_ISLAND_HOOK ??
  join(homedir(), "Documents", "win-island", "target", "release", "island-hook.exe")

// Characters a card may show next to ✓ (same allowlist as island-hook,
// which checks it again): printable ASCII, Latin-1, the Russian alphabet,
// common punctuation. Everything else leaves only Терминал / Нет.
const SHOWN_WHOLE = /^[\x20-\x7E¡-¬®-ÿЁА-яё‐-‧‰-⁞€№]*$/u
const fitsIn = (s, max) => SHOWN_WHOLE.test(s) && s === s.trim() && [...s].length <= max

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
      if (choice === 2) output.status = "allow"
      else if (choice === 1) output.status = "deny"
    },
    event: async ({ event }) => {
      if (event.type === "session.idle") await island({ kind: "notice", title: "opencode закончил", text: where("") })
    },
  }
}
