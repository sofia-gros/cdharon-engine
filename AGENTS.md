# Charon Engine — Codex guidance

## Source of truth

- Start with [`new_docs/README.md`](new_docs/README.md). The user's explicit requirements and the documents under `new_docs/` are the product design authority.
- The previous documentation, including the root `README.md`, `codex/`, and `gemini/`, is retired and retained only as historical files. Do not read it to guide development or import its requirements, restrictions, milestones, or completion claims. Consult it only when the user explicitly requests a historical comparison.
- Treat `src/`, `Cargo.toml`, and executed checks as evidence of current implementation, never as limits on the user's intended product.
- Read only the new design pages relevant to the task. Distinguish user requirements, proposed designs, unresolved decisions, and verified implementation. Do not silently present a planned feature as implemented.
- Charon Engine is an open-source, general-purpose 2D sandbox game engine. Creators own game rules, assets, entities, and progression. Specific games demonstrate the engine's capabilities and must not define mandatory gameplay in the engine core.
- The product combines Fabric API-style feature access and extensibility with Phaser-style scene, input, resource, and runtime management, integrated with 2D sandbox simulation. Interpret those reference experiences as Rust APIs: structs and traits for Scenes and systems, typed builders for construction, ordinary methods for runtime changes, and typed handles and scoped callbacks for access. Do not mechanically translate the user's illustrative JavaScript, class inheritance, dynamic options objects, prototype methods, or tentative spellings into the specification. Apply engineering judgment and explain design decisions.
- Cargo selects compiled implementations and dependencies; YAML configures engine runtime behavior and services; Rust code defines game content and registers Scenes, plugins, and custom behavior. Use those responsibilities to classify settings, rather than treating examples as an exhaustive list. YAML cannot enable absent compiled code or unsupported hardware. Block dimensions belong to material builders; player composition and controls belong to creator code and object APIs.
- Prioritize engine correctness, performance, extensibility, and maintainability. Human-readable API design is the default within those constraints. Expose and explain necessary batching, reservations, jobs, ownership, and synchronization costs instead of compromising the engine to hide complexity.
- Target commercial v1.0.0 using the requirements, architecture, data contracts, acceptance games, and release gates under `new_docs/`. Both Terraria-style and Noita-style games must be possible through public APIs, including destructible rotating bodies, material interaction, AI/navigation, spatial queries, and configurable offscreen activity. Do not downgrade required mechanisms to visual demonstrations or claim implementation completion from documentation alone.
- When implementation reveals a conflict, identify the affected user requirement and update the owning documents under `new_docs/`. Never narrow the user's goal to match existing code or make a milestone easier to pass.
- Apply P001 in `new_docs/25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md`. Standard solvers, next-tick deferral, numeric formats, randomness, and networking policies must not limit creator-authored behavior. Provide public provider, channel, state, schedule, collision, persistence, rendering, and networking integration routes. Judge Terraria and Noita reproduction by the full-clone acceptance ledger in `new_docs/26_FULL_CLONE_ACCEPTANCE.md`, never by genre similarity or representative mechanics alone. Distinguish design feasibility, verified SDK capability, and observed clone fidelity.
- Product changes are tracked as P (Plus) items. An explicitly adopted P takes precedence over overlapping earlier specifications and implementation. Record the affected requirements and implementation work; do not claim completion from documentation alone.

## Engineering rules

- Keep simulation state independent from windowing, rendering, audio, wall-clock time, and asynchronous completion order.
- Follow the agreed contracts under `new_docs/` for ordering, atomic state changes, capacity errors, and save/replay coverage. The retired documents do not establish these product contracts.
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

Keep product purpose, creator workflows, requirements, design decisions, and verified implementation records under `new_docs/`. Preserve retired documentation without treating it as an active specification. Use relative links that work on GitHub.

## Language and code documentation

- Always communicate with the user in Japanese.
- Code documentation and comments must be written in Japanese. English code documentation or comments are prohibited; translate existing English documentation or comments when modifying the relevant code.
- Write detailed, practical Japanese documentation so a human can understand the code by reading it. Explain purpose, assumptions, invariants, important control flow, failure behavior, and ownership or state changes where relevant. Avoid comments that merely restate the syntax.
- Keep public Rust API documentation (`///` and `//!`) in Japanese and generate/check it with [`cargo doc`](https://doc.rust-lang.org/cargo/commands/cargo-doc.html) when documentation changes warrant it. Generated documentation is placed in `target/doc`; a crate can be selected with `--lib` or `--bin` (for example, `cargo doc --lib`).
