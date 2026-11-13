use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const ALLOWED_KEYS: &[&str] = &["REPO_URL", "GITHUB_TOKEN", "RUN_COMMAND"];

#[derive(Debug, PartialEq, Eq)]
struct Config {
    repo_url: String,
    token: String,
    command: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct DetectedRun {
    directory: PathBuf,
    command: String,
}

fn parse_env(content: &str) -> Result<Config, String> {
    let mut values = HashMap::new();

    for (index, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, raw_value) = line
            .split_once('=')
            .ok_or_else(|| format!("Invalid .env.repo entry on line {}", index + 1))?;
        if key.is_empty()
            || !key.bytes().enumerate().all(|(i, byte)| {
                byte.is_ascii_uppercase() || (i > 0 && (byte.is_ascii_digit() || byte == b'_'))
            })
            || !key.as_bytes()[0].is_ascii_uppercase()
        {
            return Err(format!("Invalid .env.repo entry on line {}", index + 1));
        }
        if !ALLOWED_KEYS.contains(&key) {
            return Err(format!(
                "Unsupported .env.repo key on line {}: {key}",
                index + 1
            ));
        }
        if values.contains_key(key) {
            return Err(format!("Duplicate .env.repo key on line {}", index + 1));
        }

        let raw_value = raw_value.trim();
        let value = if raw_value.starts_with('"') {
            match serde_json::from_str::<Value>(raw_value) {
                Ok(Value::String(value)) => value,
                _ => return Err(format!("Invalid quoted value on line {}", index + 1)),
            }
        } else if raw_value.starts_with('\'') {
            if !raw_value.ends_with('\'') || raw_value.len() < 2 {
                return Err(format!("Invalid quoted value on line {}", index + 1));
            }
            raw_value[1..raw_value.len() - 1].to_owned()
        } else {
            raw_value.to_owned()
        };
        if value.contains(['\0', '\r', '\n']) {
            return Err(format!("Invalid value on line {}", index + 1));
        }
        values.insert(key.to_owned(), value);
    }

    config_from_values(values)
}

fn config_from_values(values: HashMap<String, String>) -> Result<Config, String> {
    for key in ["REPO_URL", "GITHUB_TOKEN"] {
        if !matches!(values.get(key), Some(value) if !value.trim().is_empty()) {
            return Err(format!("Missing required .env.repo key: {key}"));
        }
    }

    let repo_url = parse_repo_url(values.get("REPO_URL").expect("required key checked"))?;
    let token = values
        .get("GITHUB_TOKEN")
        .expect("required key checked")
        .trim()
        .to_owned();
    let command = values
        .get("RUN_COMMAND")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    if token.chars().any(char::is_whitespace) {
        return Err("GITHUB_TOKEN must not contain whitespace".to_owned());
    }
    Ok(Config {
        repo_url,
        token,
        command,
    })
}

fn detect_run_command(checkout: &Path) -> Result<DetectedRun, Box<dyn Error>> {
    fn scan(directory: &Path, found: &mut Vec<DetectedRun>) -> Result<(), Box<dyn Error>> {
        let package_path = directory.join("package.json");
        if package_path.is_file() {
            let package: Value = serde_json::from_slice(&fs::read(&package_path)?)?;
            if let Some(scripts) = package.get("scripts").and_then(Value::as_object) {
                let script = if scripts.contains_key("start") {
                    Some("start")
                } else if scripts.contains_key("dev") {
                    Some("dev")
                } else {
                    None
                };
                if let Some(script) = script {
                    let (install, runner) = if directory.join("pnpm-lock.yaml").is_file() {
                        ("pnpm install --frozen-lockfile", "pnpm")
                    } else if directory.join("yarn.lock").is_file() {
                        ("yarn install --frozen-lockfile", "yarn")
                    } else if directory.join("package-lock.json").is_file()
                        || directory.join("npm-shrinkwrap.json").is_file()
                    {
                        ("npm ci", "npm")
                    } else {
                        ("npm install", "npm")
                    };
                    found.push(DetectedRun {
                        directory: directory.to_path_buf(),
                        command: format!("{install} && {runner} {script}"),
                    });
                }
            }
        }

        if directory.join("Cargo.toml").is_file() {
            found.push(DetectedRun {
                directory: directory.to_path_buf(),
                command: "cargo run".to_owned(),
            });
        }
        if directory.join("go.mod").is_file() && directory.join("main.go").is_file() {
            found.push(DetectedRun {
                directory: directory.to_path_buf(),
                command: "go run .".to_owned(),
            });
        }
        if directory.join("main.py").is_file() {
            let install = if directory.join("requirements.txt").is_file() {
                "python -m pip install -r requirements.txt && "
            } else {
                ""
            };
            found.push(DetectedRun {
                directory: directory.to_path_buf(),
                command: format!("{install}python main.py"),
            });
        }

        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            if matches!(
                name.to_string_lossy().as_ref(),
                ".git" | ".venv" | "build" | "dist" | "node_modules" | "target" | "vendor"
            ) {
                continue;
            }
            scan(&entry.path(), found)?;
        }
        Ok(())
    }

    let mut found = Vec::new();
    scan(checkout, &mut found)?;
    let root_projects = found
        .iter()
        .filter(|candidate| candidate.directory == checkout)
        .count();
    if root_projects == 1 {
        return Ok(found
            .into_iter()
            .find(|candidate| candidate.directory == checkout)
            .expect("one root project was found"));
    }
    if root_projects > 1 {
        found.retain(|candidate| candidate.directory == checkout);
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(
            "Could not detect how to run this repository. Set RUN_COMMAND to the project's start command."
                .into(),
        ),
        _ => {
            let locations = found
                .iter()
                .map(|candidate| {
                    candidate
                        .directory
                        .strip_prefix(checkout)
                        .unwrap_or(&candidate.directory)
                        .display()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "Multiple runnable projects were found ({locations}). Set RUN_COMMAND to choose one."
            )
            .into())
        }
    }
}

fn config_from_environment() -> Result<Config, String> {
    let values = ["REPO_URL", "GITHUB_TOKEN", "RUN_COMMAND"]
        .into_iter()
        .filter_map(|key| env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect();
    config_from_values(values)
}

fn load_config(directory: &Path) -> Result<Config, Box<dyn Error>> {
    match fs::read_to_string(directory.join(".env.repo")) {
        Ok(contents) => parse_env(&contents).map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            config_from_environment().map_err(Into::into)
        }
        Err(error) => Err(error.into()),
    }
}

fn parse_repo_url(value: &str) -> Result<String, String> {
    let invalid = || "REPO_URL must be an HTTPS GitHub repository URL".to_owned();
    let value = value.trim();
    let (scheme, rest) = value.split_once("://").ok_or_else(invalid)?;
    let (authority, suffix) = rest.split_once('/').ok_or_else(invalid)?;
    if !scheme.eq_ignore_ascii_case("https")
        || !(authority.eq_ignore_ascii_case("github.com")
            || authority.eq_ignore_ascii_case("github.com:443"))
        || suffix.contains(['?', '#', '@', ':', '\\'])
        || value.contains(['\r', '\n'])
    {
        return Err(invalid());
    }

    let path = suffix.strip_suffix('/').unwrap_or(suffix);
    let mut parts = path.split('/');
    let owner = parts.next().ok_or_else(invalid)?;
    let mut repository = parts.next().ok_or_else(invalid)?;
    if parts.next().is_some() || !valid_repo_part(owner) {
        return Err(invalid());
    }
    if let Some(stripped) = repository.strip_suffix(".git") {
        repository = stripped;
    }
    if !valid_repo_part(repository) {
        return Err(invalid());
    }

    Ok(format!("https://github.com/{owner}/{repository}.git"))
}

fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn build_git_env(token: &str, empty_config_path: &Path) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = env::vars()
        .filter(|(key, _)| {
            let key = key.to_ascii_uppercase();
            !key.starts_with("GIT_") && key != "GITHUB_TOKEN" && key != "GH_TOKEN"
        })
        .collect();
    env.extend([
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        (
            "GIT_CONFIG_GLOBAL".to_owned(),
            empty_config_path.to_string_lossy().into_owned(),
        ),
        ("GIT_CONFIG_COUNT".to_owned(), "2".to_owned()),
        (
            "GIT_CONFIG_KEY_0".to_owned(),
            "http.https://github.com/.extraheader".to_owned(),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_owned(),
            format!(
                "AUTHORIZATION: basic {}",
                base64(format!("x-access-token:{token}").as_bytes())
            ),
        ),
        (
            "GIT_CONFIG_KEY_1".to_owned(),
            "credential.helper".to_owned(),
        ),
        ("GIT_CONFIG_VALUE_1".to_owned(), String::new()),
        ("GIT_LFS_SKIP_SMUDGE".to_owned(), "1".to_owned()),
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
    ]);
    env
}

fn parse_runtime_env(content: &str) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    for (index, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("Invalid .env entry on line {}", index + 1))?;
        if key.is_empty()
            || (!key.as_bytes()[0].is_ascii_alphabetic() && key.as_bytes()[0] != b'_')
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!("Invalid .env key on line {}", index + 1));
        }
        if values.contains_key(key) {
            return Err(format!("Duplicate .env key on line {}", index + 1));
        }
        let value = value.trim();
        let value = if value.starts_with('"') {
            match serde_json::from_str::<Value>(value) {
                Ok(Value::String(value)) => value,
                _ => return Err(format!("Invalid quoted .env value on line {}", index + 1)),
            }
        } else if value.starts_with('\'') {
            if !value.ends_with('\'') || value.len() < 2 {
                return Err(format!("Invalid quoted .env value on line {}", index + 1));
            }
            value[1..value.len() - 1].to_owned()
        } else {
            value.to_owned()
        };
        if value.contains(['\0', '\r', '\n']) {
            return Err(format!("Invalid .env value on line {}", index + 1));
        }
        values.insert(key.to_owned(), value);
    }
    Ok(values)
}

fn is_runner_or_git_variable(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.starts_with("GIT_")
        || matches!(
            key.as_str(),
            "GITHUB_TOKEN" | "GH_TOKEN" | "REPO_URL" | "RUN_COMMAND"
        )
}

fn build_runtime_env(extra: HashMap<String, String>) -> HashMap<String, String> {
    let mut values: HashMap<String, String> = env::vars()
        .filter(|(key, _)| !is_runner_or_git_variable(key))
        .collect();
    for (key, value) in extra {
        if !is_runner_or_git_variable(&key) {
            values.insert(key, value);
        }
    }
    values
}

fn find_env_file(directory: &Path) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let path = directory.join(".env");
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(path)),
        Ok(_) => Err(".env exists but is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn run_process<I, S>(
    program: &str,
    args: I,
    environment: Option<&HashMap<String, String>>,
    directory: Option<&Path>,
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(environment) = environment {
        command.env_clear().envs(environment);
    }
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let status = command
        .status()
        .map_err(|error| format!("Could not start {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited {}", exit_description(status)))
    }
}

#[cfg(unix)]
fn exit_description(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(signal) => format!("after signal {signal}"),
        None => format!("with code {}", status.code().unwrap_or(-1)),
    }
}

#[cfg(not(unix))]
fn exit_description(status: ExitStatus) -> String {
    format!("with code {}", status.code().unwrap_or(-1))
}

fn create_temp_root() -> Result<PathBuf, Box<dyn Error>> {
    let temp_dir = env::temp_dir();
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    for attempt in 0..100 {
        let path = temp_dir.join(format!(
            "private-repo-runner-{}-{timestamp}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("Could not create a unique temporary directory".into())
}

fn run() -> Result<(), Box<dyn Error>> {
    let directory = env::current_dir()?;
    let config = load_config(&directory)?;
    let env_file_path = find_env_file(&directory)?;
    let extra_env = match env_file_path {
        Some(path) => parse_runtime_env(&fs::read_to_string(path)?)?,
        None => HashMap::new(),
    };
    let runtime_env = build_runtime_env(extra_env);
    let temp_root = create_temp_root()?;
    let result = run_with_temp(&config, &temp_root, &runtime_env);
    let cleanup = fs::remove_dir_all(&temp_root);

    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(error), Err(cleanup_error)) => Err(format!(
            "{error}; additionally, temporary directory cleanup failed: {cleanup_error}"
        )
        .into()),
    }
}

fn run_with_temp(
    config: &Config,
    temp_root: &Path,
    runtime_env: &HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    let checkout_path = temp_root.join("checkout");
    let template_path = temp_root.join("empty-template");
    let empty_config_path = temp_root.join("empty-git-config");
    fs::create_dir(&template_path)?;
    fs::write(&empty_config_path, [])?;

    println!("Cloning the private repository...");
    let git_env = build_git_env(&config.token, &empty_config_path);
    run_process(
        "git",
        [
            "clone",
            "--quiet",
            "--single-branch",
            "--no-tags",
            "--template",
            template_path.to_string_lossy().as_ref(),
            "--",
            &config.repo_url,
            checkout_path.to_string_lossy().as_ref(),
        ],
        Some(&git_env),
        None,
    )?;

    let run = match &config.command {
        Some(command) => DetectedRun {
            directory: checkout_path.clone(),
            command: command.clone(),
        },
        None => detect_run_command(&checkout_path)?,
    };

    println!("Running the private repository with: {}", run.command);
    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/C".to_owned(), run.command]);
    #[cfg(not(windows))]
    let (program, args) = ("/bin/sh", vec!["-lc".to_owned(), run.command]);
    run_process(program, args, Some(runtime_env), Some(&run.directory))?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runner_configuration() {
        assert_eq!(
            parse_env(
                "# Private repository\nREPO_URL=https://github.com/acme/private.git\nGITHUB_TOKEN=ghp_example123\nRUN_COMMAND=\"npm start\""
            )
            .unwrap(),
            Config {
                repo_url: "https://github.com/acme/private.git".to_owned(),
                token: "ghp_example123".to_owned(),
                command: Some("npm start".to_owned()),
            }
        );
    }

    #[test]
    fn reads_runner_configuration_values_without_a_config_file() {
        let values = HashMap::from([
            (
                "REPO_URL".to_owned(),
                "https://github.com/acme/private".to_owned(),
            ),
            ("GITHUB_TOKEN".to_owned(), "ghp_token".to_owned()),
            ("RUN_COMMAND".to_owned(), "npm start".to_owned()),
        ]);
        assert_eq!(
            config_from_values(values).unwrap(),
            Config {
                repo_url: "https://github.com/acme/private.git".to_owned(),
                token: "ghp_token".to_owned(),
                command: Some("npm start".to_owned()),
            }
        );
        let values = HashMap::from([
            (
                "REPO_URL".to_owned(),
                "https://github.com/acme/private".to_owned(),
            ),
            ("GITHUB_TOKEN".to_owned(), "ghp_token".to_owned()),
        ]);
        assert_eq!(config_from_values(values).unwrap().command, None);
    }

    #[test]
    fn accepts_only_plain_https_github_repository_urls() {
        assert_eq!(
            parse_repo_url("https://github.com/acme/private").unwrap(),
            "https://github.com/acme/private.git"
        );
        assert_eq!(
            parse_repo_url("HTTPS://GITHUB.COM:443/acme/private/").unwrap(),
            "https://github.com/acme/private.git"
        );
        for url in [
            "http://github.com/acme/private",
            "https://user:pass@github.com/acme/private",
            "https://github.com.evil.example/acme/private",
            "https://github.com/acme/private?token=secret",
            "https://github.com/acme/private/tree/main",
        ] {
            assert!(parse_repo_url(url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn rejects_duplicate_unknown_and_missing_configuration() {
        assert!(parse_env(
            "REPO_URL=https://github.com/acme/private\nREPO_URL=https://github.com/acme/other"
        )
        .unwrap_err()
        .contains("Duplicate"));
        assert!(parse_env("TOKEN=secret")
            .unwrap_err()
            .contains("Unsupported"));
        let config =
            parse_env("REPO_URL=https://github.com/acme/private\nGITHUB_TOKEN=secret").unwrap();
        assert_eq!(config.command, None);
    }

    #[test]
    fn encodes_clone_credentials_only_in_git_configuration() {
        let env = build_git_env("ghp_secret", Path::new("C:\\temp\\empty-config"));
        assert_eq!(env["GIT_CONFIG_COUNT"], "2");
        assert_eq!(
            env["GIT_CONFIG_KEY_0"],
            "http.https://github.com/.extraheader"
        );
        assert_eq!(
            env["GIT_CONFIG_VALUE_0"],
            "AUTHORIZATION: basic eC1hY2Nlc3MtdG9rZW46Z2hwX3NlY3JldA=="
        );
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        assert!(!env.contains_key("GITHUB_TOKEN"));
    }

    #[test]
    fn parses_target_environment_and_filters_runner_secrets() {
        let values = parse_runtime_env("PORT=8080\nMESSAGE=\"hello world\"\n").unwrap();
        assert_eq!(values["PORT"], "8080");
        assert_eq!(values["MESSAGE"], "hello world");
        assert!(parse_runtime_env("invalid").is_err());
        assert!(is_runner_or_git_variable("GITHUB_TOKEN"));
        assert!(is_runner_or_git_variable("GIT_CONFIG_VALUE_0"));
        assert!(!is_runner_or_git_variable("PORT"));
    }

    #[test]
    fn detects_node_project_and_installs_its_dependencies() {
        let directory = create_temp_root().unwrap();
        fs::write(
            directory.join("package.json"),
            r#"{"scripts":{"start":"node server.js"}}"#,
        )
        .unwrap();
        fs::write(directory.join("package-lock.json"), "{}").unwrap();

        let run = detect_run_command(&directory).unwrap();
        assert_eq!(run.directory, directory);
        assert_eq!(run.command, "npm ci && npm start");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn detects_a_nested_project_and_rejects_ambiguous_projects() {
        let directory = create_temp_root().unwrap();
        let nested = directory.join("service");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("Cargo.toml"), "[package]").unwrap();

        let run = detect_run_command(&directory).unwrap();
        assert_eq!(run.directory, nested);
        assert_eq!(run.command, "cargo run");

        let other = directory.join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("main.py"), "print('hello')").unwrap();
        assert!(detect_run_command(&directory)
            .unwrap_err()
            .to_string()
            .contains("Multiple runnable projects"));
        fs::remove_dir_all(directory).unwrap();
    }
}
