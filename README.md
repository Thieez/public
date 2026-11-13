# Private repository runner

Clone a private GitHub repository with a personal access token, then run its
configured start command directly from a temporary checkout.

## Run locally

Copy `.env.repo.example` to `.env.repo`, then set:

- `REPO_URL`: HTTPS URL of the private GitHub repository.
- `GITHUB_TOKEN`: a narrowly scoped, read-only GitHub token.
- `RUN_COMMAND`: the command that starts the private project.

Run `cargo run`. Git and the runtime required by the private project must be
installed. An optional `.env` beside `.env.repo` supplies variables to the
private project.

## Deploy on Render

Create a Render Web Service from this public repository. Set the build command
to `cargo build --release` and the start command to
`./target/release/private-repository-runner`. Add `REPO_URL`, `GITHUB_TOKEN`,
and `RUN_COMMAND` in the service's environment settings, along with any
environment variables required by the private project. Set `RUN_COMMAND` to
whatever installs/builds and starts that project (for example,
`npm install && npm start`). The private project's web server must listen on
`0.0.0.0` and the port provided by Render's `PORT` variable.

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
