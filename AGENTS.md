# Charon Engine — Codex guidance

## Source of truth

- Start with [`codex/README.md`](codex/README.md) for current behavior and [`codex/design/00_INDEX.md`](codex/design/00_INDEX.md) for the product design.
- Treat `codex/design/02_CONTRACTS.md` as the shared target contract. Treat `src/` and `Cargo.toml` as evidence of what the prototype actually implements.
- Read only the system design documents relevant to the task. Do not silently present a planned feature as implemented.
- When implementation reveals a conflict in the design, identify the affected contract and update the linked design and audit documents with the code change.

## Engineering rules

- Keep simulation state independent from windowing, rendering, audio, wall-clock time, and asynchronous completion order.
- Preserve deterministic ordering, atomic world and inventory changes, explicit capacity errors, and save/replay state coverage from the design contracts.
- Keep rendering data separate from simulation data. Prefer a correct scalar reference implementation before optimizing.
- Avoid new dependencies unless the task needs them. Explain their purpose and alternatives in the change.
- Make focused changes and keep public API, content, save format, and simulation version changes explicit.
- Explain ownership and borrowing in plain language when teaching Rust concepts.

## Useful commands

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked
```

Run the narrowest relevant checks while iterating, then the full set when implementation changes affect the engine. Never claim a check passed unless it was run successfully.

## Documentation

Keep user-facing current behavior in `codex/README.md`, normative product design in `codex/design/`, and historical/background material in `gemini/`. Use relative links that work on GitHub.
