# Contributing

New controllers are the most welcome contribution. Follow
[Adding a controller](docs/controllers/adding-a-controller.md); the mapping file format is in the
[mapping reference](docs/controllers/mapping-reference.md). If you get stuck or want to claim a
controller, open a "Controller support" issue.

## Setup

```bash
mise install    # pulls rust, prek, cargo-nextest, cargo-deny (pinned in mise.toml)
                # — see https://mise.jdx.dev to install mise itself
prek install    # activates the git hooks
```

`prek run --all-files` runs every check on demand without a commit.

On Linux, install the ALSA headers first (`sudo apt-get install libasound2-dev`); cpal needs
them to build.

## Making a change

- `mise run test` runs the test suite.
- `mise run lint` runs `prek run --all-files`: formatting, clippy, `cargo deny`, typos and the
  other checks CI enforces.
- See [`docs/architecture.md`](docs/architecture.md) for the thread model and crate layout. It
  has real-time rules that the audio code must follow. Design history is in
  `docs/superpowers/specs/`.

## Commit messages

This repo follows [Conventional Commits](https://www.conventionalcommits.org/)
(`feat:`, `fix:`, `docs:`, `ci:`, `build:`, ...) — look at `git log` for the
established style.

## Pull requests

All changes go through a PR against `main`. CI (`lint`, `test`) must pass. See the root
[`README.md`](README.md#releasing) for how releases are cut once a change lands.

By contributing, you agree your contributions are licensed under this
project's [Apache 2.0 license](LICENSE).
