# Repository Guidelines

## Project Structure & Module Organization
Komodo is a Rust workspace with TypeScript packages. Rust binaries live in `bin/core`, `bin/periphery`, and `bin/cli`; shared crates are under `lib/*`; Rust clients are in `client/core/rs` and `client/periphery/rs`. The generated TypeScript client is in `client/core/ts`, the React/Vite UI is in `ui/src`, and the Docusaurus site is in `docsite`. Assets live in `compose`, `config`, `scripts`, `example`, and `screenshots`. Use `xtask` for generators.

## Build, Test, and Development Commands
- `rtk cargo build --verbose`: build all Rust workspace crates.
- `rtk cargo test --verbose`: run the Rust test suite.
- `rtk cargo fmt --all -- --check`: verify Rust formatting before pushing.
- `rtk cargo run -p xtask -- generate resource-schema --pretty --stdout`: preview the generated resources schema.
- `rtk yarn --cwd client/core/ts` and `rtk yarn --cwd client/core/ts build`: install and build the TypeScript client package.
- `rtk yarn --cwd ui` and `rtk yarn --cwd ui dev`: run the UI locally. If using the local client, follow `ui/README.md` to build and `yarn link komodo_client` first.
- `rtk yarn --cwd docsite start` or `rtk yarn --cwd docsite build`: run or build docs.

## Coding Style & Naming Conventions
Rust uses edition 2024 and `rustfmt.toml` (`max_width = 70`, two-space indentation). Keep modules aligned with existing crate boundaries and move code to `lib/*` only when reused. TypeScript is strict; UI imports commonly use the `@/*` alias from `ui/tsconfig.json`. Match existing React component naming and colocate feature UI under `ui/src/resources`, `ui/src/pages`, or `ui/src/components`.

## Testing Guidelines
CI runs Rust build, tests, and format checks. Add unit tests near the code with `#[cfg(test)]` modules and descriptive `#[test]` names. No dedicated UI test runner is configured; validate UI/client changes with the relevant `rtk yarn --cwd ... build` command and include manual verification notes.

## Commit & Pull Request Guidelines
Recent history uses short subjects such as `fix ...`, `chore: ...`, and release/version messages. Keep commits focused and imperative; add a scoped prefix when useful. PRs should describe the change, link issues, list verification commands, and include screenshots for visible UI changes.
Name branches as `scope-do-something-interesting`, without agent prefixes such as `codex/`, `claude/`, or `agent/`. PR titles should be plain descriptive titles and must not start with bracketed agent labels such as `[codex]`.
Never open PRs or MRs against the source/upstream repository; all pull or merge requests for this fork must target `intezya/komodo`.

## Security & Configuration Tips
Do not commit local secrets or environment files. For UI development, place local values such as `VITE_KOMODO_HOST=https://demo.komo.do` in `ui/.env.development`, which is gitignored.

## Agent-Specific Instructions
When running shell commands through Codex, prefix them with `rtk`. Check whether target files already exist before creating root-level guidance files.
