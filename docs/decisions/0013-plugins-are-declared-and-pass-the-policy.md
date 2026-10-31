# ADR-0013: Plugins are declared, sandboxed by ADR-0012 when they run code, and never grant a verdict

- **Status:** Accepted; first slice built 2026-10-31 (progress.md). Manifests, the classification rule and the loader
  are in place; the class offered is the stricter of the declared and the assessed class, and only read-only and
  safe-execution tools are offered.
- **Status (original):** Accepted (2026-10-31); the first plugin is decided below
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

The owner wants to add capabilities later without changing the core: new tools, new checks, new policy rules, and
eventually new model providers. Bridle already has one extension form: declarative adapters in `[[tools.adapters]]`,
which name a program, fixed arguments and a description (rule V25). Adapters are set in the machine config or an explicit
config only, never in the repository (`config::trust`).

OpenCode has its own plugin system (`opencode-plugin/local-agent.ts`). That is OpenCode's extension point and is not the
one proposed here.

## Decision (proposed)

1. **A plugin is a manifest, not code, in release 1.** A manifest declares: a name, a version, the tools it adds (each
   with a class: READ_ONLY, EXECUTE_SAFE or EXECUTE_RISKY), the files it reads, and the network hosts it needs. The
   loader refuses a plugin whose manifest does not validate.
2. **Adapters are the first plugin form.** They already fit the manifest idea. The plugin manifest generalises them;
   existing adapters keep working unchanged.
3. **No plugin grants a verdict.** A plugin tool goes through the same policy, classes and loop rules as a built-in
   tool. A plugin cannot mark its own tools as safe. An unknown tool stays EXECUTE_RISKY, as it is today.
4. **Code plugins wait for ADR-0012.** A plugin that runs its own code runs in the sandbox that ADR-0012 defines, or it
   is not loaded. Until the sandbox exists, only manifest plugins are allowed.
5. **Only from machine config.** The same trust rule that applies to adapters applies here: a repository cannot add a
   plugin. A cloned repository must not be able to add tools to the user's agent.
6. **Provider plugins** (a new model provider) go through the endpoint policy and the existing provider trait
   (`ModelProvider`), not through the manifest. They come after the routing decision (ADR-0010).

## Consequences

- A plugin is one more place where a malicious or buggy component could act, so each addition is reviewed like a change
  to the policy.
- The manifest format becomes a public contract and needs a version field from the first release.

## Decided (2026-10-31, by the owner)

- **The first plugin adds a new tool class.** Its classification rule (READ_ONLY, EXECUTE_SAFE or EXECUTE_RISKY) must be
  written, tested and reviewed before any code. An unknown or unclassified tool stays EXECUTE_RISKY until then.
