import { spawn } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const allowedKeys = new Set([
  "REPO_URL",
  "GITHUB_TOKEN",
  "DOCKER_IMAGE",
  "RUN_COMMAND",
]);
const defaultImage = "node:22-alpine";

export function parseEnv(content) {
  const values = {};

  for (const [index, rawLine] of content.split(/\r?\n/).entries()) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;

    const match = /^([A-Z][A-Z0-9_]*)=(.*)$/.exec(line);
    if (!match) throw new Error(`Invalid .env.repo entry on line ${index + 1}`);

    const [, key] = match;
    if (!allowedKeys.has(key)) {
      throw new Error(`Unsupported .env.repo key on line ${index + 1}: ${key}`);
    }
    if (Object.hasOwn(values, key)) {
      throw new Error(`Duplicate .env.repo key on line ${index + 1}: ${key}`);
    }

    let value = match[2].trim();
    if (value.startsWith('"')) {
      try {
        value = JSON.parse(value);
      } catch {
        throw new Error(`Invalid quoted value on line ${index + 1}`);
      }
    } else if (value.startsWith("'")) {
      if (!value.endsWith("'") || value.length < 2) {
        throw new Error(`Invalid quoted value on line ${index + 1}`);
      }
      value = value.slice(1, -1);
    }
    if (typeof value !== "string" || /[\0\r\n]/.test(value)) {
      throw new Error(`Invalid value on line ${index + 1}`);
    }
    values[key] = value;
  }

  for (const key of ["REPO_URL", "GITHUB_TOKEN", "RUN_COMMAND"]) {
    if (!values[key]?.trim()) {
      throw new Error(`Missing required .env.repo key: ${key}`);
    }
  }

  const repoUrl = parseRepoUrl(values.REPO_URL);
  const token = values.GITHUB_TOKEN.trim();
  const image = values.DOCKER_IMAGE?.trim() || defaultImage;
  const command = values.RUN_COMMAND.trim();

  if (/\s/.test(token)) throw new Error("GITHUB_TOKEN must not contain whitespace");
  if (!/^[A-Za-z0-9][A-Za-z0-9._/:@-]*$/.test(image)) {
    throw new Error("DOCKER_IMAGE must be a valid image reference without arguments");
  }

  return { repoUrl, token, image, command };
}

export function parseRepoUrl(value) {
  let url;
  try {
    url = new URL(value.trim());
  } catch {
    throw new Error("REPO_URL must be an HTTPS GitHub repository URL");
  }

  const match = /^\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+?)(?:\.git)?\/?$/.exec(
    url.pathname,
  );
  if (
    url.protocol !== "https:" ||
    url.hostname !== "github.com" ||
    url.port ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    !match ||
    [match[1], match[2]].some((part) => part === "." || part === "..")
  ) {
    throw new Error("REPO_URL must be an HTTPS GitHub repository URL");
  }

  return `https://github.com/${match[1]}/${match[2]}.git`;
}

export function buildGitEnv(token, emptyConfigPath) {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(GIT_|GITHUB_TOKEN$|GH_TOKEN$)/i.test(key)) delete env[key];
  }
  Object.assign(env, {
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: emptyConfigPath,
    GIT_CONFIG_COUNT: "2",
    GIT_CONFIG_KEY_0: "http.https://github.com/.extraheader",
    GIT_CONFIG_VALUE_0: `AUTHORIZATION: basic ${Buffer.from(
      `x-access-token:${token}`,
    ).toString("base64")}`,
    GIT_CONFIG_KEY_1: "credential.helper",
    GIT_CONFIG_VALUE_1: "",
    GIT_LFS_SKIP_SMUDGE: "1",
    GIT_TERMINAL_PROMPT: "0",
  });
  return env;
}

export function buildDockerArgs(checkoutPath, image, command, envFilePath) {
  const source = path
    .resolve(checkoutPath)
    .replaceAll("\\", "/")
    .replaceAll(",", "\\,");
  const args = [
    "run",
    "--rm",
    "--init",
    "--pull=missing",
    "--network=none",
    "--read-only",
    "--cap-drop=ALL",
    "--security-opt=no-new-privileges",
    "--pids-limit=128",
    "--memory=1g",
    "--cpus=2",
    "--tmpfs",
    "/tmp:rw,nosuid,size=128m",
    "--env",
    "HOME=/tmp",
    "--env",
    "TMPDIR=/tmp",
    "--mount",
    `type=bind,source=${source},target=/workspace`,
  ];
  if (envFilePath) args.push("--env-file", path.resolve(envFilePath));
  args.push(
    "--workdir",
    "/workspace",
    "--entrypoint",
    "/bin/sh",
    image,
    "-lc",
    command,
  );
  return args;
}

async function findEnvFile(directory) {
  const envFilePath = path.join(directory, ".env");
  try {
    const file = await stat(envFilePath);
    if (!file.isFile()) throw new Error(".env exists but is not a regular file");
    return envFilePath;
  } catch (error) {
    if (error.code === "ENOENT") return undefined;
    throw error;
  }
}

function runProcess(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: "inherit", ...options });
    child.once("error", (error) => {
      reject(new Error(`Could not start ${command}: ${error.message}`));
    });
    child.once("close", (code, signal) => {
      if (code === 0) return resolve();
      reject(
        new Error(
          `${command} exited ${signal ? `after signal ${signal}` : `with code ${code}`}`,
        ),
      );
    });
  });
}

async function main() {
  const configPath = path.resolve(process.cwd(), ".env.repo");
  const config = parseEnv(await readFile(configPath, "utf8"));
  const envFilePath = await findEnvFile(process.cwd());
  const tempRoot = await mkdtemp(path.join(os.tmpdir(), "private-repo-runner-"));

  try {
    const checkoutPath = path.join(tempRoot, "checkout");
    const templatePath = path.join(tempRoot, "empty-template");
    const emptyConfigPath = path.join(tempRoot, "empty-git-config");
    await Promise.all([
      mkdir(templatePath),
      writeFile(emptyConfigPath, "", { mode: 0o600 }),
    ]);

    console.log("Cloning the private repository...");
    await runProcess(
      "git",
      [
        "clone",
        "--quiet",
        "--single-branch",
        "--no-tags",
        "--template",
        templatePath,
        "--",
        config.repoUrl,
        checkoutPath,
      ],
      { env: buildGitEnv(config.token, emptyConfigPath) },
    );

    console.log(`Running repository in Docker (${config.image})...`);
    await runProcess(
      "docker",
      buildDockerArgs(checkoutPath, config.image, config.command, envFilePath),
    );
  } finally {
    await rm(tempRoot, { recursive: true, force: true });
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
