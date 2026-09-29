# Contribution conventions

## Branches

- `main` is the releasable branch.
- Use `<type>/<short-description>` for work branches, for example
  `feat/jwt-auth` or `fix/cursor-validation`.

## Commits

Use Conventional Commits:

```text
<type>(optional-scope): imperative summary
```

Common types are `feat`, `fix`, `test`, `docs`, `refactor`, `perf`, `build`,
and `chore`. Keep the subject concise and describe one logical change.

## Rust and API naming

- Rust modules and files use `snake_case`.
- Types and traits use `PascalCase`.
- Functions, variables, and JSON fields use `snake_case`.
- HTTP paths use lowercase kebab-case where a compound resource name is needed.
- Database tables and columns use lowercase `snake_case`.
- Environment variables use uppercase `SCREAMING_SNAKE_CASE`.

Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace` before opening a change.
