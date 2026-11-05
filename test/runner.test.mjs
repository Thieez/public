import assert from "node:assert/strict";
import test from "node:test";
import {
  buildDockerArgs,
  buildGitEnv,
  parseEnv,
  parseRepoUrl,
} from "../src/runner.mjs";

test("parses runner configuration and applies the default image", () => {
  assert.deepEqual(
    parseEnv(
      [
        "# Private repository",
        "REPO_URL=https://github.com/acme/private.git",
        "GITHUB_TOKEN=ghp_example123",
        'RUN_COMMAND="npm start"',
      ].join("\n"),
    ),
    {
      repoUrl: "https://github.com/acme/private.git",
      token: "ghp_example123",
      image: "node:22-alpine",
      command: "npm start",
    },
  );
});

test("accepts only plain HTTPS github.com owner/repository URLs", () => {
  assert.equal(
    parseRepoUrl("https://github.com/acme/private"),
    "https://github.com/acme/private.git",
  );
  for (const url of [
    "http://github.com/acme/private",
    "https://user:pass@github.com/acme/private",
    "https://github.com.evil.example/acme/private",
    "https://github.com/acme/private?token=secret",
    "https://github.com/acme/private/tree/main",
  ]) {
    assert.throws(() => parseRepoUrl(url), /HTTPS GitHub repository URL/);
  }
});

test("rejects duplicate, unknown, and missing configuration", () => {
  assert.throws(
    () =>
      parseEnv(
        "REPO_URL=https://github.com/acme/private\nREPO_URL=https://github.com/acme/other",
      ),
    /Duplicate/,
  );
  assert.throws(() => parseEnv("TOKEN=secret"), /Unsupported/);
  assert.throws(
    () => parseEnv("REPO_URL=https://github.com/acme/private\nGITHUB_TOKEN=secret"),
    /RUN_COMMAND/,
  );
});

test("passes clone credentials through Git config environment, not the URL", () => {
  const env = buildGitEnv("ghp_secret", "C:\\temp\\empty-config");
  assert.equal(env.GIT_CONFIG_COUNT, "2");
  assert.equal(env.GIT_CONFIG_KEY_0, "http.https://github.com/.extraheader");
  assert.equal(env.GIT_TERMINAL_PROMPT, "0");
  assert.equal(env.GITHUB_TOKEN, undefined);
  assert.equal(env.GIT_CONFIG_GLOBAL, "C:\\temp\\empty-config");
});

test("builds a restricted Docker invocation without forwarding credentials", () => {
  const args = buildDockerArgs("C:\\temp\\checkout", "node:22-alpine", "npm start");
  assert.ok(args.includes("--network=none"));
  assert.ok(args.includes("--read-only"));
  assert.ok(args.includes("--cap-drop=ALL"));
  assert.ok(args.includes("--pids-limit=128"));
  assert.equal(args.at(-1), "npm start");
  assert.ok(!args.some((arg) => arg.includes("GITHUB_TOKEN")));
});

test("passes the runner directory .env to Docker as the private repo environment", () => {
  const args = buildDockerArgs(
    "C:\\temp\\checkout",
    "node:22-alpine",
    "npm start",
    "C:\\work\\public\\.env",
  );
  const envFileIndex = args.indexOf("--env-file");
  assert.notEqual(envFileIndex, -1);
  assert.equal(args[envFileIndex + 1], "C:\\work\\public\\.env");
});

test("does not add an env file when none is configured", () => {
  const args = buildDockerArgs("C:\\temp\\checkout", "node:22-alpine", "npm start");
  assert.ok(!args.includes("--env-file"));
});
