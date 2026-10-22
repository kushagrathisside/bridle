// local-agent OpenCode plugin shim (ADR-0001).
//
// This file holds NO policy logic. It forwards OpenCode hook payloads as JSON to the Rust
// `agent hook <event>` command and applies the verdict it returns. Policy hooks fail closed:
// if the harness cannot answer, the tool call is blocked (tool.execute.before) and the
// permission is denied (permission.ask). Telemetry-only hooks fail open.
//
// Generated into `.opencode/plugins/local-agent.ts` by `agent init`, which replaces the
// placeholder below with the absolute path of the agent executable. Edit the template in the
// harness repository (opencode-plugin/local-agent.ts), not the generated copy.
// Pinned against @opencode-ai/plugin 1.19.3 and type-checked against it by `npm run typecheck`
// (in opencode-plugin/); the type import is erased at runtime.

import type { Plugin } from "@opencode-ai/plugin"
import { spawn } from "node:child_process"

const AGENT_BIN: string = process.env.LOCAL_AGENT_BIN || "__AGENT_BIN__"
const HOOK_TIMEOUT_MS = 20_000

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

    "permission.ask": async (input, output) => {
      try {
        const result = await callHook("permission", { permission: input }, workspace)
        if (result.status === "allow" || result.status === "ask" || result.status === "deny") {
          output.status = result.status
        }
      } catch {
        // Decision D3: if the policy engine cannot answer, the permission is denied.
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
