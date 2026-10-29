// local-agent OpenCode plugin shim (ADR-0001).
//
// This file holds NO policy logic. It forwards OpenCode hook payloads as JSON to the Rust
// `agent hook <event>` command and applies the verdict it returns. Policy hooks fail closed:
// if the harness cannot answer, the tool call is blocked (tool.execute.before) and the
// permission is denied (permission.ask). Telemetry-only hooks and the context hooks fail open:
// the request is sent unchanged. Compaction is a view of the history, not a security control.
//
// Generated into `.opencode/plugins/local-agent.ts` by `agent init`, which replaces the
// placeholder below with the absolute path of the agent executable. Edit the template in the
// harness repository (opencode-plugin/local-agent.ts), not the generated copy.
// Pinned against @opencode-ai/plugin 1.19.3 and type-checked against it by `npm run typecheck`
// (in opencode-plugin/); the type import is erased at runtime.

import type { Plugin } from "@opencode-ai/plugin"
import { spawn } from "node:child_process"

const AGENT_BIN: string = process.env.LOCAL_AGENT_BIN || "__AGENT_BIN__"
// A deep escalation swaps GPU models inside the tool-before hook, which takes about 20 s in all (measured 2026-10-28).
// The limit leaves room for two swap waits of 45 s and the deep answer, and still gives up before OpenCode does.
const HOOK_TIMEOUT_MS = 150_000

// Only low-frequency lifecycle events are forwarded; streaming updates would spawn a process
// per token.
const FORWARDED_EVENTS = new Set([
  "session.created",
  "session.idle",
  "session.error",
  "session.compacted",
  "file.edited",
  "permission.replied",
])

type Json = Record<string, unknown>

// The parts of a message this shim reads when it applies the context changes.
type ContextPart = { type: string; state?: { status?: string; output?: string } }
type ContextMessage = { parts: ContextPart[] }
type ContextChange = { message: number; part: number; output: string }

function callHook(event: string, payload: Json, workspace: string): Promise<Json> {
  return new Promise((resolve, reject) => {
    const child = spawn(AGENT_BIN, ["hook", event, "--workspace", workspace], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    })
    let stdout = ""
    let stderr = ""
    const timer = setTimeout(() => {
      child.kill()
      reject(new Error(`agent hook ${event} timed out after ${HOOK_TIMEOUT_MS} ms`))
    }, HOOK_TIMEOUT_MS)
    child.stdout.on("data", (chunk) => (stdout += chunk))
    child.stderr.on("data", (chunk) => (stderr += chunk))
    child.on("error", (err) => {
      clearTimeout(timer)
      reject(err)
    })
    child.on("close", (code) => {
      clearTimeout(timer)
      if (code !== 0) {
        reject(new Error(`agent hook ${event} exited with ${code}: ${stderr.trim()}`))
        return
      }
      try {
        resolve(JSON.parse(stdout) as Json)
      } catch {
        reject(new Error(`agent hook ${event} returned invalid JSON`))
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

// Applies the outputs the harness chose for this request. Messages are replaced, not changed in place, so a
// part object shared with the stored session is never modified.
function applyContextChanges(messages: ContextMessage[], changes: unknown): void {
  if (!Array.isArray(changes)) return
  const copies = new Map<number, ContextMessage>()
  for (const change of changes as ContextChange[]) {
    const original = messages[change.message]
    if (!original || typeof change.output !== "string") continue
    const message = copies.get(change.message) ?? { ...original, parts: [...original.parts] }
    copies.set(change.message, message)
    const part = message.parts[change.part]
    if (!part || part.type !== "tool" || part.state?.status !== "completed") continue
    message.parts[change.part] = { ...part, state: { ...part.state, output: change.output } }
  }
  for (const [index, message] of copies) messages[index] = message
}

export const LocalAgentPlugin: Plugin = async ({ directory, worktree }) => {
  // OpenCode reports "/" as the worktree outside git repositories; never use a filesystem root.
  const workspace = worktree && worktree !== "/" && !/^[A-Za-z]:[\\/]?$/.test(worktree) ? worktree : directory
  const startedAt = new Map<string, number>()

  return {
    "tool.execute.before": async (input, output) => {
      startedAt.set(input.callID, Date.now())
      let verdict: Json
      try {
        verdict = await callHook(
          "tool-before",
          { tool: input.tool, sessionID: input.sessionID, callID: input.callID, args: output.args },
          workspace,
        )
      } catch (err) {
        throw new Error(`[local-agent] policy engine unavailable, tool call blocked: ${message(err)}`)
      }
      if (verdict.action !== "allow") {
        throw new Error(`[local-agent] ${String(verdict.message ?? "blocked by policy")}`)
      }
    },

    "tool.execute.after": async (input, output) => {
      const started = startedAt.get(input.callID)
      startedAt.delete(input.callID)
      try {
        const result = await callHook(
          "tool-after",
          {
            tool: input.tool,
            sessionID: input.sessionID,
            callID: input.callID,
            args: input.args,
            title: output.title,
            output: output.output,
            durationMs: started === undefined ? null : Date.now() - started,
          },
          workspace,
        )
        if (typeof result.output === "string") output.output = result.output
      } catch (err) {
        // Redaction is a security control: if it cannot run, withhold the output.
        output.output = `[local-agent] tool output withheld: redaction unavailable (${message(err)})`
      }
    },

    "experimental.chat.messages.transform": async (_input, output) => {
      try {
        const result = await callHook("messages-transform", { messages: output.messages }, workspace)
        applyContextChanges(output.messages, result.changes)
      } catch {
        // Fail open: the request keeps its full history.
      }
    },

    "experimental.chat.system.transform": async (input, output) => {
      try {
        const result = await callHook("system-transform", { sessionID: input.sessionID }, workspace)
        if (typeof result.text !== "string" || result.text.length === 0) return
        // One system message only: the Qwen chat template rejects a second one, so the block joins the first.
        if (output.system.length === 0) output.system.push(result.text)
        else output.system[0] = `${output.system[0]}\n\n${result.text}`
      } catch {
        // Fail open: the prompt is sent without the task state.
      }
    },

    "permission.ask": async (input, output) => {
      try {
        const result = await callHook("permission", { permission: input }, workspace)
        if (result.status === "allow" || result.status === "ask" || result.status === "deny") {
          output.status = result.status
        }
      } catch {
        // if the policy engine cannot answer, the permission is denied.
        output.status = "deny"
      }
    },

    "shell.env": async (input, output) => {
      try {
        const result = await callHook(
          "shell-env",
          { cwd: input.cwd, sessionID: input.sessionID, callID: input.callID },
          workspace,
        )
        const set = result.set
        if (set && typeof set === "object") {
          for (const [key, value] of Object.entries(set as Record<string, unknown>)) {
            if (typeof value === "string") output.env[key] = value
          }
        }
      } catch {
        // Execution itself is still gated by tool.execute.before.
      }
    },

    event: async ({ event }) => {
      if (!FORWARDED_EVENTS.has(event.type)) return
      try {
        await callHook("event", { event }, workspace)
      } catch {
        // Telemetry only.
      }
    },
  }
}
