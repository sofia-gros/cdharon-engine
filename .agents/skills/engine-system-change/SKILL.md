---
name: engine-system-change
description: Implement or review Charon Engine simulation, materials, chunks, collision, gameplay, persistence, or renderer changes while keeping the product design and shared contracts consistent. Use when a task changes engine behavior or a cross-system boundary.
---

# Engine system change

1. Read `new_docs/README.md` and the relevant pages under `new_docs/`. The user's explicit requirements take precedence. Do not consult retired documentation in the root README, `codex/`, or `gemini/` unless the user explicitly requests a historical comparison.
2. Inspect the current implementation and its tests. Separate existing behavior from the user's design target before editing. Existing code and previous milestones do not limit the product's intended capabilities.
3. Trace the change through its read/write state, tick phase, coordinate and units, capacity limits, deterministic ordering, save/hash/replay coverage, and sleep or wake behavior.
4. For material or attachment changes, also trace transfer, reaction, inventory, schema version, content hash, and save restore. For chunk changes, distinguish visible, prefetched, active, frozen, and requested states.
5. Implement the smallest reviewable change. Add dependencies only when the benefit and alternatives are clear.
6. When the task includes verification or the change needs regression coverage, add or update targeted tests. Run focused checks, then the repository checks in `AGENTS.md` when the change warrants them. Report checks accurately.
7. Update the owning pages and any applicable requirement, decision, or implementation records under `new_docs/` when behavior, a contract, API, configuration, or supported workflow changes. Record product changes as P (Plus) items when applicable, with explicit precedence over conflicting earlier specifications. Do not introduce game-specific requirements into the engine core.
8. Finish with a short summary of behavior, validation, design or version impact, and unresolved risks.
