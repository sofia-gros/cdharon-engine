---
name: engine-system-change
description: Implement or review Charon Engine simulation, materials, chunks, collision, gameplay, persistence, or renderer changes while keeping the product design and shared contracts consistent. Use when a task changes engine behavior or a cross-system boundary.
---

# Engine system change

1. Read `codex/README.md`, `codex/design/00_INDEX.md`, `02_CONTRACTS.md`, and the design pages that own the affected behavior.
2. Inspect the current implementation and its tests. Separate existing behavior from the design target before editing.
3. Trace the change through its read/write state, tick phase, coordinate and units, capacity limits, deterministic ordering, save/hash/replay coverage, and sleep or wake behavior.
4. For material or attachment changes, also trace transfer, reaction, inventory, schema version, content hash, and save restore. For chunk changes, distinguish visible, prefetched, active, frozen, and requested states.
5. Implement the smallest reviewable change. Add dependencies only when the benefit and alternatives are clear.
6. When the task includes verification or the change needs regression coverage, add or update targeted tests. Run focused checks, then the repository checks in `AGENTS.md` when the change warrants them. Report checks accurately.
7. Update the owning design page, `10_DECISIONS_AND_AUDIT.md`, and `codex/README.md` when behavior, a contract, API, configuration, or supported workflow changes.
8. Finish with a short summary of behavior, validation, design or version impact, and unresolved risks.
