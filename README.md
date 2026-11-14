# Private repository runner

Clone a private GitHub repository with a personal access token, then
automatically detect and run its project from a temporary checkout.

## Run locally

Copy `.env.repo.example` to `.env.repo`, then set:

- `REPO_URL`: HTTPS URL of the private GitHub repository.
- `GITHUB_TOKEN`: a narrowly scoped, read-only GitHub token.
- `RUN_COMMAND`: optional command override. If omitted, the runner detects a
  runnable project and starts it automatically.
- `RUN_DIRECTORY`: optional path relative to the cloned repository. Use it
  when the repository contains multiple projects or when an override should
  run from a specific subdirectory.

Run `cargo run`. Git and the runtime required by the private project must be
installed. Automatic detection supports Node.js projects with a `start` or
`dev` script, Rust projects with `Cargo.toml`, Go projects with `go.mod` and
`main.go`, and Python projects with `main.py`. Node dependencies are installed
using the detected package manager and Python dependencies are installed from
`requirements.txt` when present. Detection runs against the cloned repository,
not the runner's working directory. When it finds a project in a subdirectory,
the runner starts the command from that directory. An explicit `RUN_COMMAND`
also uses the detected project directory; if there are multiple candidate
directories, set `RUN_DIRECTORY` to select one. An optional `.env` beside
`.env.repo` supplies variables to the private project.

For example, for a Rust service whose `Cargo.toml` is in `server/`, automatic
detection runs `cargo run` from `server/`. To override the command explicitly,
set `RUN_COMMAND=cargo run --release`; the runner still uses `server/` as its
working directory.

## Deploy on Render

Create a Render Web Service from this public repository. Set the build command
to `cargo build --release` and the start command to
`./target/release/private-repository-runner`. Pancake installs the Copilot CLI
automatically on Linux/macOS when it starts; no custom Render install command
is needed. The service must allow outbound HTTPS to `gh.io` and GitHub, and
have `bash` and `curl` available. Add `REPO_URL` and
`GITHUB_TOKEN` in the service's environment settings, along with any
environment variables required by the private project. `RUN_COMMAND` can be
set to override automatic detection; `RUN_DIRECTORY` selects the working
directory inside the cloned repository when needed. The private project's web
server must listen on `0.0.0.0` and the port provided by Render's `PORT`
variable. The runner also adds `$HOME/.local/bin` to the private project's
`PATH`, so tools installed there can be launched normally.

The runner clones the repository on each start into a temporary directory,
runs the command there, and deletes the checkout when it exits. Runner settings
can also be supplied via `.env.repo`; if that file exists, it takes precedence
over runner settings in the process environment.

The private project runs directly with the runner's OS user privileges and
network access; it is not sandboxed. Only run code from a repository you trust.
The GitHub token and Git authentication variables are withheld from the
project process. Never commit `.env.repo`; keep the token narrowly scoped.

## Test

Run `cargo test` to execute the configuration and environment tests.
