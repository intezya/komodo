---
name: rust-skills-checks
description: Use when editing, updating, auditing, or validating the bundled rust-skills rule library, especially after changing SKILL.md, README.md, or rules/*.md.
---

# Rust Skills Checks

Use this only for maintaining the `rust-skills` skill itself. Do not use it for normal Komodo Rust code review; use `rust-skills` and task-specific Rust skills instead.

## Target

By default, the bundled scripts validate the sibling skill at:

```text
.codex/skills/rust-skills
```

Override with `RUST_SKILLS_ROOT=/path/to/rust-skills` when validating another copy.

## Commands

Run structural validation only:

```bash
RUST_SKILLS_ROOT=.codex/skills/rust-skills python3 .codex/skills/rust-skills-checks/checks/validate.py
```

Run the full upstream check flow:

```bash
RUST_SKILLS_ROOT=.codex/skills/rust-skills bash .codex/skills/rust-skills-checks/checks/check.sh
```

The full flow extracts Rust code blocks from `rules/*.md`, type-checks generated examples, and compares expected compiler findings against `baseline.txt`.

## Notes

- Generated `examples/`, `manifest.json`, `check.json`, `check.err`, and `target/` are local validation artifacts.
- These checks validate the rule library, not Komodo application code.
