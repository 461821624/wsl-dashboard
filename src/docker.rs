// SPDX-FileCopyrightText: Copyright (c) 2026 owu <wqh@live.com>
// SPDX-License-Identifier: GPL-3.0-only

use crate::i18n;
use serde::Deserialize;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_OUTPUT: u64 = 1024 * 1024;

#[path = "docker/resources.rs"]
mod resources;
pub use resources::{Image, normalize_pull_source, projects, resolve_pull_reference};

pub fn matches_filter(name: &str, image: &str, state: &str, query: &str, filter: i32) -> bool {
    let state_matches = match filter {
        0 => true,
        1 => state == "running",
        2 => state == "exited",
        _ => false,
    };
    let query = query.trim().to_lowercase();
    state_matches && (name.to_lowercase().contains(&query) || image.to_lowercase().contains(&query))
}

async fn read_output(stream: impl tokio::io::AsyncRead + Unpin) -> Result<String, String> {
    let mut data = Vec::new();
    stream
        .take(MAX_OUTPUT + 1)
        .read_to_end(&mut data)
        .await
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_OUTPUT {
        return Err(i18n::t("docker.output_limit"));
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

pub enum Target {
    Desktop,
    Wsl { distro: String, as_root: bool },
}

#[derive(Debug, Deserialize)]
pub struct Container {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Names")]
    pub name: String,
    #[serde(rename = "Image")]
    pub image: String,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "Ports")]
    pub ports: String,
    #[serde(rename = "Project", default)]
    pub project: String,
    #[serde(rename = "Service", default)]
    pub service: String,
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit())
}

fn parse_containers(output: &str) -> Result<Vec<Container>, String> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let container: Container = serde_json::from_str(line)
                .map_err(|e| format!("{}: {e}", i18n::t("docker.invalid_output")))?;
            if !valid_id(&container.id) {
                return Err(i18n::t("docker.invalid_output"));
            }
            Ok(container)
        })
        .collect()
}

fn action_args(action: &str, id: &str) -> Result<Vec<String>, String> {
    if !valid_id(id) {
        return Err(i18n::t("docker.invalid_container"));
    }
    let args = match action {
        "start" | "stop" | "restart" => vec![action, "--", id],
        "logs" => vec!["logs", "--tail", "200", "--timestamps", "--", id],
        "stats" => vec![
            "stats",
            "--no-stream",
            "--no-trunc",
            "--format",
            "{{json .}}",
            "--",
            id,
        ],
        _ => return Err(i18n::t("docker.invalid_action")),
    };
    Ok(args.into_iter().map(str::to_owned).collect())
}

impl Target {
    fn command(&self, args: &[String]) -> Command {
        let mut command = match self {
            Self::Desktop => {
                let mut command = Command::new("docker.exe");
                command.args(["--context", "desktop-linux"]);
                command
            }
            Self::Wsl { distro, as_root } => {
                let mut command = Command::new("wsl.exe");
                command.args(["--distribution", distro]);
                if *as_root {
                    command.args(["--user", "root"]);
                }
                // shortcut: standard rootful socket only; add socket selection when rootless support is requested.
                command.args([
                    "--exec",
                    "env",
                    "-u",
                    "DOCKER_CONTEXT",
                    "-u",
                    "DOCKER_HOST",
                    "-u",
                    "DOCKER_TLS",
                    "-u",
                    "DOCKER_TLS_VERIFY",
                    "-u",
                    "DOCKER_CERT_PATH",
                    "docker",
                    "--host",
                    "unix:///var/run/docker.sock",
                ]);
                command
            }
        };
        for key in [
            "DOCKER_CONTEXT",
            "DOCKER_HOST",
            "DOCKER_TLS",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
        ] {
            command.env_remove(key);
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        command
    }

    async fn run(&self, args: &[String]) -> Result<(String, String), String> {
        self.run_with_timeout(args, Duration::from_secs(45)).await
    }

    async fn run_with_timeout(
        &self,
        args: &[String],
        timeout: Duration,
    ) -> Result<(String, String), String> {
        let mut command = self.command(args);
        let execute = async {
            let mut child = command
                .spawn()
                .map_err(|e| format!("{}: {e}", i18n::t("docker.cli_unavailable")))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| i18n::t("docker.command_failed"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| i18n::t("docker.command_failed"))?;
            let (stdout, stderr) = tokio::try_join!(read_output(stdout), read_output(stderr))?;
            let status = child.wait().await.map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!(
                    "{} ({status}):\n{stdout}{stderr}",
                    i18n::t("docker.command_failed")
                ));
            }
            Ok((stdout, stderr))
        };
        tokio::time::timeout(timeout, execute).await.map_err(|_| {
            i18n::t(if timeout.as_secs() > 45 {
                "docker.pull_timeout"
            } else {
                "docker.timeout"
            })
        })?
    }

    pub async fn list(&self) -> Result<(String, Vec<Container>), String> {
        self.run(&["--version".into()]).await?;
        let (version, _) = self
            .run(&[
                "info".into(),
                "--format".into(),
                "{{.ServerVersion}}".into(),
            ])
            .await
            .map_err(|e| format!("{}\n{e}", i18n::t("docker.engine_unavailable")))?;
        let (output, _) = self
            .run(&[
                "ps".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--format".into(),
                concat!(
                    r#"{"ID":{{json .ID}},"Names":{{json .Names}},"Image":{{json .Image}},"State":{{json .State}},"Status":{{json .Status}},"Ports":{{json .Ports}},"#,
                    r#""Project":{{json (.Label "com.docker.compose.project")}},"Service":{{json (.Label "com.docker.compose.service")}}}"#,
                ).into(),
            ])
            .await?;
        Ok((version.trim().to_owned(), parse_containers(&output)?))
    }

    pub async fn action(&self, action: &str, id: &str) -> Result<String, String> {
        let (stdout, stderr) = self.run(&action_args(action, id)?).await?;
        Ok(format!("{stdout}{stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_names_and_images_without_losing_other_container_states() {
        assert!(matches_filter(
            "Web-测试",
            "nginx:1.27",
            "running",
            " WEB ",
            1
        ));
        assert!(matches_filter(
            "Web-测试",
            "nginx:1.27",
            "running",
            "测试",
            0
        ));
        assert!(matches_filter("web", "NGINX:1.27", "exited", "nginx", 2));
        assert!(matches_filter("web", "nginx", "paused", "", 0));
        assert!(!matches_filter("web", "nginx", "running", "redis", 0));
        assert!(!matches_filter("web", "nginx", "exited", "", 1));
        assert!(!matches_filter("web", "nginx", "restarting", "", 2));
        assert!(!matches_filter("web", "nginx", "running", "", 99));
    }

    #[tokio::test]
    async fn output_limit_fails_without_waiting_for_eof() {
        assert_eq!(read_output(&b"hello"[..]).await.unwrap(), "hello");
        let (reader, mut writer) = tokio::io::duplex(4096);
        let writer_task = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            writer
                .write_all(&vec![b'x'; MAX_OUTPUT as usize + 1])
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(Duration::from_secs(3), read_output(reader)).await;
        writer_task.abort();
        assert!(result.expect("must not wait for EOF").is_err());
    }

    #[test]
    fn parses_running_and_stopped_containers_with_unicode() {
        let input = concat!(
            "{\"ID\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"Names\":\"web\",\"Image\":\"nginx:latest\",\"State\":\"running\",\"Status\":\"Up 2 hours\",\"Ports\":\"127.0.0.1:8080->80/tcp\"}\r\n",
            "{\"ID\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"Names\":\"测试\",\"Image\":\"redis\",\"State\":\"exited\",\"Status\":\"Exited (0)\",\"Ports\":\"\"}\n"
        );
        let containers = parse_containers(input).unwrap();
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].ports, "127.0.0.1:8080->80/tcp");
        assert_eq!(containers[0].state, "running");
        assert_eq!(containers[1].name, "测试");
        assert_eq!(containers[1].state, "exited");
        assert!(parse_containers(" \n\r\n").unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_or_incomplete_output_instead_of_partial_results() {
        for input in ["not json", "{}", "{\"ID\":\"--help\"}", "{}\nnot json"] {
            assert!(parse_containers(input).is_err(), "{input}");
        }
        let valid = format!(
            r#"{{"ID":"{}","Names":"web","Image":"nginx","State":"running","Status":"Up","Ports":""}}"#,
            "a".repeat(64)
        );
        assert!(parse_containers(&format!("{valid}\nnot json")).is_err());
        assert!(parse_containers(&valid.replace(&"a".repeat(64), "--help")).is_err());
    }

    #[test]
    fn rejects_option_injection_and_unsupported_actions() {
        for id in ["", "--help", "abc;shutdown", "abcdef", &"a".repeat(65)] {
            assert!(action_args("stop", id).is_err(), "{id}");
        }
        let id = "a".repeat(64);
        assert!(action_args("rm", &id).is_err());
        assert_eq!(action_args("stop", &id).unwrap(), ["stop", "--", &id]);
        assert_eq!(
            action_args("logs", &id).unwrap(),
            ["logs", "--tail", "200", "--timestamps", "--", &id]
        );
        assert_eq!(
            action_args("stats", &id).unwrap(),
            [
                "stats",
                "--no-stream",
                "--no-trunc",
                "--format",
                "{{json .}}",
                "--",
                &id
            ]
        );
    }

    #[test]
    fn commands_pin_the_target_and_preserve_distro_as_one_argument() {
        let desktop = Target::Desktop.command(&["ps".into()]);
        assert_eq!(desktop.as_std().get_program(), "docker.exe");
        let args: Vec<_> = desktop
            .as_std()
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect();
        assert_eq!(args, ["--context", "desktop-linux", "ps"]);

        let wsl = Target::Wsl {
            distro: "Ubuntu Test & echo test".into(),
            as_root: false,
        }
        .command(&["ps".into()]);
        assert_eq!(wsl.as_std().get_program(), "wsl.exe");
        let args: Vec<_> = wsl
            .as_std()
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect();
        assert_eq!(
            &args[..3],
            ["--distribution", "Ubuntu Test & echo test", "--exec"]
        );
        assert!(
            args.windows(3)
                .any(|args| args == ["docker", "--host", "unix:///var/run/docker.sock"])
        );
        assert!(!args.contains(&"sh"));
        assert!(!args.contains(&"--user"));
    }

    #[test]
    fn root_access_explicitly_selects_root_in_the_requested_distro() {
        let command = Target::Wsl {
            distro: "Ubuntu-24.04".into(),
            as_root: true,
        }
        .command(&["ps".into()]);
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect();
        assert_eq!(
            &args[..5],
            ["--distribution", "Ubuntu-24.04", "--user", "root", "--exec"]
        );
        assert!(
            args.windows(3)
                .any(|args| args == ["docker", "--host", "unix:///var/run/docker.sock"])
        );
    }
}
