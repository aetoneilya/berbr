# Berbr for Herdr

**Keep each coding task together: agents, worktrees, notes and merge requests.**

Berbr is a native Herdr panel for developers working on several coding tasks
with Claude Code or Codex. See which agent needs you and pick up where you left off.

![Berbr with synthetic tasks](preview.png)

This branch prepares a **macOS public preview**. GitHub, task/MR/PR views and the
distributable skill are being developed separately. This build supports local
tasks and configured Jira/GitLab integrations.

## Install or update

Download this repository's macOS release archive for your architecture:
`aarch64-apple-darwin` (Apple Silicon) or `x86_64-apple-darwin` (Intel).
Compare its SHA-256 with the accompanying checksum, unpack, then run:

```sh
python3 install.py --activate
```

Archives include the backend; Rust is not needed. Requires Herdr, Python 3, Git
and your chosen installed/authenticated agent CLI. Tested with Herdr App 0.1.14.
A public release has not yet been published.

Updates use the same command. The installer validates in an isolated home, backs
up the installed mod/backend, then replaces them together. Layouts, keys and
project config are preserved. A new installation docks on the right.
`--activate` opens the panel; otherwise use the command palette.
No new layout or replacement tab bar is installed.

Add `~/.config/herdr-app/mods/berbr` to PATH for terminal use.
`HERDR_APP_HOME` selects another Herdr home.

## First task

**Connect repository** asks for a local Git repository and Codex or Claude Code.
It writes project config, then offers **New task**; no manual TOML is needed.
No task or agent is created merely by connecting. Existing configs load as before.

```sh
berbr init --repo /path/to/repository --agent codex
berbr doctor
berbr --agent codex start "Improve upload retries" --name upload-retries
```

For CLI/TUI use with the desktop backend, set `BERBR_BACKEND=herdr-app`.
Local tasks need no Jira or GitLab. Configure integrations later using the main
README. Explicit config paths and vault-backed notes remain supported.
An invalid existing config is reported and preserved.

The mod runs programs and reads/writes task notes, repository state and relevant
agent history. Configured integrations access the network. Workflow actions can
create worktrees/agents and close tasks/tickets through their normal confirmation.
No credentials or personal config are shipped.

## Source install

```sh
cargo build --release --locked
python3 scripts/install-herdr-app.py --activate
```

Existing personal Berbr layouts keep working. The public installer updates
`berbr` only; it does not install or change the old `tabs-berbr` source.

## Validation and packaging

```sh
env -u BERBR_BACKEND cargo test --locked
node --test --test-concurrency=1 herdr-app/berbr/tests/*.test.mjs
python3 -m unittest discover -s scripts/tests
herdr-app check herdr-app/berbr
python3 scripts/preview-herdr-app.py
python3 scripts/package-herdr-app.py --binary target/release/berbr --target aarch64-apple-darwin --version v0.2.0-preview.1
python3 scripts/package-herdr-app.py --gallery --version v0.2.0-preview.1
```

The preview opens a separate native window with a temporary home, synthetic
data and a private core socket. External actions are disabled. It closes only
its own app/core. `--setup --output /tmp/berbr-setup.png` captures onboarding.

Archives and checksums go to `dist/`. `dist/berbr-gallery/` has
`mods/berbr/`, `RECIPE.md`, `preview.png`, an installer and release metadata,
without a `layouts/` folder. This source-only folder still needs a compiled
backend. The distributable skill is included once `skills/berbr` is integrated.

The **macOS preview** workflow builds both architectures on manual dispatch.
Publishing a GitHub Release triggers builds and attaches archives/checksums.
Adding the workflow alone publishes nothing.

## Public preview release gate

Before announcing:

- Integrate and validate GitHub (berbr-8), views (berbr-9) and the agent skill
  (berbr-10).
- Test a clean installation without Rust, connecting a repo, starting a task,
  returning to an agent, viewing a PR and updating without losing config.
- Refresh the recipe and synthetic screenshots to match integrated features.
- Validate both macOS architectures. The code and release package use MIT.
- Generate the gallery folder, include it in the public repository (for example
  under `gallery/berbr/`), review/publish, then add the topic `herdr-mod`.

[Herdr documents](https://herdr.dev/gallery/?type=mods) automatic listing within
an hour; community images are screened separately.
This branch does not commit, push, create a release or change repository topics.
