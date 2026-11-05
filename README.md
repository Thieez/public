# Private repository runner

Clone a private GitHub repository with a personal access token, then run a
configured command inside a temporary Docker container.

## Setup

1. Copy `.env.repo.example` to `.env.repo`.
2. Set `REPO_URL` to an HTTPS GitHub repository URL and `GITHUB_TOKEN` to a
   token with read-only access to that repository.
3. Set `DOCKER_IMAGE` and `RUN_COMMAND` for the repository's language and
   start command.
4. Run `npm start` from this project directory. Docker and Git must be
   installed.

For example, use `node:22-alpine` with `npm start` for a Node.js project, or
`python:3.12-slim` with `python main.py` for a Python project.

Each run uses a fresh temporary checkout, removed when the command exits. The
token is used only by Git during cloning and is not passed to the container.
If a `.env` file is present beside `.env.repo`, its variables are passed to the
container as the private repository's environment; the file itself is not
copied into the checkout.
The container has no network access, receives only the checkout, and runs with
resource limits and reduced privileges. This limits exposure but is not a
perfect security boundary; run code only from repositories you trust. Never
commit `.env.repo`, and use a narrowly scoped token.

## Test

Run `npm test` to execute the configuration and sandbox-option tests.
