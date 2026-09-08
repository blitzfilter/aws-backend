# DOX

## Purpose

- Own project-local agent skills.
- Keep repeated backend workflows out of broad `AGENTS.md` files.

## Core Design

- Skills in `.agents/skills/` hold focused task playbooks.
- Repo `AGENTS.md` files still own durable rules and path contracts.

## Ownership

- This doc rule `.agents/**`.
- Skill `SKILL.md` rule its skill directory.

## Local Contracts

- Keep skill descriptions specific so agent loads them only when useful.
- Update skills when repo workflow, infra wiring, docs contract, or test flow changes.
- Projection skill follows durable SQS publication-before-ACK, complete-only delete, retention-bounded replay, and persistent deletion fences. Effective operational values live in `docs/durable-worker-runbook.md`; verify code and external deployment handoffs, never assume rollout.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Prefer short checklist skills over giant shared docs.

## Verification

- Check skill frontmatter has matching `name` and directory.

## Child DOX Index

- `skills/aura-rust-enum/SKILL.md` — enum design, canonical machine identifiers, iteration, and persisted enum evolution.
- `skills/aura-rust-api-endpoint/SKILL.md` — API route, controller, DTO, auth, error mapping.
- `skills/aura-rust-projection/SKILL.md` — CDC, durable SQS/Sequin acknowledgment, deletion fences, replay/rebuild.
- `skills/aura-rust-reader/SKILL.md` — readers, read models, hydration.
- `skills/aura-rust-repository/SKILL.md` — aggregate repositories, Postgres mapping, versions.
- `skills/aura-rust-review-architecture/SKILL.md` — final/review architecture gate.
- `skills/aura-rust-test/SKILL.md` — test placement and validation.
- `skills/aura-rust-transactional-flow/SKILL.md` — UnitOfWork and transactional writes.
- `skills/aura-rust-use-case/SKILL.md` — service use cases, ports, auth policy.
