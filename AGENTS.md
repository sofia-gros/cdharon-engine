# Charon Engine — Codex guidance

## Source of truth

- Start with [`codex/README.md`](codex/README.md) for current behavior and [`codex/design/00_INDEX.md`](codex/design/00_INDEX.md) for the product design.
- Treat `codex/design/02_CONTRACTS.md` as the shared target contract. Treat `src/` and `Cargo.toml` as evidence of what the code actually implements.
- Read only the system design documents relevant to the task. Do not silently present a planned feature as implemented.
- When implementation reveals a conflict in the design, identify the affected contract and update the linked design and audit documents with the code change.

## Engineering rules

- Keep simulation state independent from windowing, rendering, audio, wall-clock time, and asynchronous completion order.
- Preserve deterministic ordering, atomic world and inventory changes, explicit capacity errors, and save/replay state coverage from the design contracts.
- Keep rendering data separate from simulation data. Prefer a correct scalar reference implementation before optimizing.
- Avoid new dependencies unless the task needs them. Explain their purpose and alternatives in the change.
- Make focused changes and keep public API, content, save format, and simulation version changes explicit.
- Explain ownership and borrowing in plain language when teaching Rust concepts.
- Prototypes are prohibited in almost all cases. Implement production-intent behavior that satisfies the applicable contracts and milestone exit criteria. Do not mark a milestone complete when only a prototype or partial implementation exists.
- If a prototype is genuinely necessary, notify the user before presenting or relying on it. Clearly identify the prototype's scope, missing behavior, limitations, and the work required to replace it. Keep that status visible in code and relevant documentation so readers can distinguish it from complete behavior without searching the entire repository.

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

## Language and code documentation

- Always communicate with the user in Japanese.
- Code documentation and comments must be written in Japanese. English code documentation or comments are prohibited; translate existing English documentation or comments when modifying the relevant code.
- Write detailed, practical Japanese documentation so a human can understand the code by reading it. Explain purpose, assumptions, invariants, important control flow, failure behavior, and ownership or state changes where relevant. Avoid comments that merely restate the syntax.
- Keep public Rust API documentation (`///` and `//!`) in Japanese and generate/check it with [`cargo doc`](https://doc.rust-lang.org/cargo/commands/cargo-doc.html) when documentation changes warrant it. Generated documentation is placed in `target/doc`; a crate can be selected with `--lib` or `--bin` (for example, `cargo doc --lib`).
