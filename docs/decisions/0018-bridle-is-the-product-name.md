# ADR-0018: Bridle is the product name; the first working name is read, never written

- **Status:** Accepted (2026-11-03)
- **Date:** 2026-11-03
- **Deciders:** Kushagra Srivastava

## Context

The project began under the working name `local-agent`. [ADR-0001](0001-rust-core-thin-host-adapter.md) isolated every
derived name in `harness-core::identity` so that a rename would be a one-file change. The product is called Bridle, and every
name a person sees should say so. Projects and a machine were already set up under the old names.

## Decision

1. **Bridle names everywhere.** `bridle.toml`, the `.bridle/` state folder, `BRIDLE_*` variables, the `bridle` machine config
   folder, the `bridle` MCP server and provider prefix (`bridle_git_status`, `bridle-main`), the plugin file
   `.opencode/plugins/bridle.ts`, and the command `bridle`.
2. **The old names are a read-only fallback.** `local-agent.toml` is used when `bridle.toml` is absent; `.local-agent/` when
   `.bridle/` is absent; `LOCAL_AGENT_*` when the `BRIDLE_*` variable is not set; `%APPDATA%\local-agent\config.toml` when the
   `bridle` folder has no config. Nothing is ever written under an old name, and nothing is moved without being asked: an
   old file keeps working until the person renames it. The constants are `LEGACY_*` in the identity module.
3. **One program, `bridle`.** With no command it opens the window (in a detached copy, so the terminal is free); with a command
   (`bridle doctor`, `bridle models serve main`) it runs that command and prints in the terminal. `bridle-cli` is the same
   commands without the window, for scripts, tests and machines without a display.
4. **The generated OpenCode files change name.** `bridle init` writes `opencode.json` and `.opencode/plugins/bridle.ts`. A
   project that has the old `local-agent.ts` plugin keeps it until it is deleted by hand; the two do not conflict.

## Consequences

- Every existing project, state folder and machine config keeps working; a test covers each fallback.
- Tool names change from `local-agent_<tool>` to `bridle_<tool>` in OpenCode. A model prompt or a permission rule that named
  the old prefix needs the new one.
- Documentation and configs were rewritten to the new names; `docs/archive` keeps the old ones as the record it is.
