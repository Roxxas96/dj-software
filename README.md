# dj-software

Open-source DJ software written in Rust. It aims for a VirtualDJ-like workflow, built around
precise controller mapping: every control on a supported controller is mapped to a named action,
and LEDs follow the app state.

**Status: early development.** Nothing is usable yet. The foundation (audio engine, file
decoding, audio output) is being built first.

## Supported controllers

| Controller | Status |
|---|---|
| Pioneer DDJ-SB3 | In progress |

New controllers are welcome, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Building from source

Install [mise](https://mise.jdx.dev), then:

```bash
mise install                # installs the pinned Rust toolchain and dev tools
cargo run -p dj-software
```

On Linux you also need the ALSA headers (`libasound2-dev` on Debian and Ubuntu).

## Architecture

See [docs/architecture.md](docs/architecture.md). Design specs live in
[docs/superpowers/specs/](docs/superpowers/specs/).

## Releasing

Run `cz bump` to update the version and changelog, then push the tag `vX.Y.Z`. Release binaries
come in a later plan.

## Disclaimer

Not affiliated with Pioneer DJ, AlphaTheta, Atomix Productions (VirtualDJ) or Serato. Product
names are trademarks of their owners, used only to describe compatibility.

## License

[Apache 2.0](LICENSE)
