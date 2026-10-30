[Español](CONTRIBUTING.md) | **English**

# How to contribute

Thank you for wanting to help. Illapa Engine is the API of a community open source project: it has no ads, it is not monetized, and it aims to be useful to people who live in or travel through areas of heavy rain in Peru.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.en.md).

## What helps most

- Reporting districts that show no warning when they should, or the other way round, with the ubigeo and the date.
- Reviewing how SENAMHI names are matched with INEI names.
- Tests, performance and robustness when SENAMHI's server fails.
- Reviewing and documenting the licenses of the data sources.

## Branches and workflow

We use [GitHub Flow](https://docs.github.com/en/get-started/using-github/github-flow): a single main branch and short branches for each change.

- `main` is the production branch: it must always build and be deployable. It is protected and nobody writes to it directly.
- Each change goes on a branch created from `main`, named `type/short-description`: `feat/forecast-endpoint`, `fix/district-names`.
- If you are not part of the team, work from your fork and send the pull request to `main`.
- To merge, a pull request needs at least one approval and passing checks. It is merged with a squash merge, so the title must follow Conventional Commits in English: it becomes the commit message on `main`.

## Before sending

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

If you change the database schema, add a new migration in `migrations/`; do not edit the existing ones.

## Style

- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org) in English, for example `feat(api): add the forecast endpoint`.
- Identifiers and technical comments are in English. API and database fields are in Spanish, like the data they describe (`ubigeo`, `aviso`, `distrito`).
- Units go in the field name when there are units (`pp_mm`, `area_km2`).
- Do not make up data: if a source is missing, the response says so; it is never filled with zero.
- No secrets in the repository: configuration goes in environment variables.

## License of your contributions

The project is distributed under the [AGPL-3.0-or-later](LICENSE). By sending a contribution you agree that it is published under that same license; you remain the author of what you write. Do not include code or data that you cannot publish under the AGPL.

## Security

If you find a security problem, do not publish it in an issue: write to contacto@illapalabs.org.
