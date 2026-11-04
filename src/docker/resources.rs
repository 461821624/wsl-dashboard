// SPDX-License-Identifier: GPL-3.0-only

use super::{Container, MAX_OUTPUT, Target, valid_id};
use crate::i18n;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug)]
pub struct Image {
    pub id: String,
    pub tags: Vec<String>,
    pub size: String,
    pub created: String,
    pub references: usize,
}

#[derive(Debug)]
pub struct Project {
    pub name: String,
    pub total: usize,
    pub running: usize,
    pub services: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImageRow {
    #[serde(rename = "ID")]
    id: String,
    repository: String,
    tag: String,
    size: String,
    created_since: String,
}

fn valid_image_id(id: &str) -> bool {
    id.strip_prefix("sha256:").is_some_and(valid_id)
}

fn parse_images(output: &str) -> Result<Vec<Image>, String> {
    let mut images = BTreeMap::<String, Image>::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let row: ImageRow = serde_json::from_str(line)
            .map_err(|e| format!("{}: {e}", i18n::t("docker.invalid_output")))?;
        if !valid_image_id(&row.id) {
            return Err(i18n::t("docker.invalid_output"));
        }
        let tag = format!("{}:{}", row.repository, row.tag);
        let image = images.entry(row.id.clone()).or_insert(Image {
            id: row.id,
            tags: Vec::new(),
            size: row.size,
            created: row.created_since,
            references: 0,
        });
        if !image.tags.contains(&tag) {
            image.tags.push(tag);
        }
    }
    Ok(images.into_values().collect())
}

pub fn normalize_pull_source(source: &str) -> Result<String, String> {
    let source = source.trim();
    if source.is_empty() {
        return Ok(String::new());
    }
    let source = source
        .strip_prefix("https://")
        .unwrap_or(source)
        .trim_end_matches('/');
    let authority = source.split('/').next().unwrap_or("");
    if source.is_empty()
        || source.len() > 256
        || !source.as_bytes()[0].is_ascii_alphanumeric()
        || !source
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-/:".contains(&c))
        || source.contains("://")
        || source.contains("//")
        || source.split('/').any(|part| matches!(part, "." | ".."))
        || authority.ends_with(':')
        || !(authority.contains('.') || authority.contains(':') || authority == "localhost")
        || reqwest::Url::parse(&format!("https://{source}")).is_err()
    {
        return Err(i18n::t("docker.invalid_pull_source"));
    }
    Ok(source.to_owned())
}

pub fn resolve_pull_reference(reference: &str, source: &str) -> Result<String, String> {
    let reference = reference.trim();
    // Docker performs the complete reference grammar check; reject flags and shell/control characters here.
    if reference.is_empty()
        || reference.len() > 512
        || !reference.as_bytes()[0].is_ascii_alphanumeric()
        || !reference
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/:@-".contains(&c))
    {
        return Err(i18n::t("docker.invalid_image"));
    }
    let source = normalize_pull_source(source)?;
    if source.is_empty() {
        return Ok(reference.to_owned());
    }
    let (host, path) = reference.split_once('/').unwrap_or(("", reference));
    let path = match host {
        "docker.io" | "index.docker.io" => path,
        host if host.contains('.') || host.contains(':') || host == "localhost" => {
            return Ok(reference.to_owned());
        }
        _ => reference,
    };
    let path = if path.contains('/') {
        path.to_owned()
    } else {
        format!("library/{path}")
    };
    let target = format!("{source}/{path}");
    if target.len() > 512 {
        return Err(i18n::t("docker.invalid_image"));
    }
    Ok(target)
}

fn pull_args(reference: &str, source: &str) -> Result<Vec<String>, String> {
    Ok(vec![
        "image".into(),
        "pull".into(),
        "--".into(),
        resolve_pull_reference(reference, source)?,
    ])
}

fn remove_args(id: &str) -> Result<Vec<String>, String> {
    if !valid_image_id(id) {
        return Err(i18n::t("docker.invalid_image"));
    }
    Ok(["image", "rm", "--", id].map(str::to_owned).to_vec())
}

pub fn projects(containers: &[Container]) -> Vec<Project> {
    let mut projects = BTreeMap::<String, Project>::new();
    for c in containers {
        if c.project.is_empty() {
            continue;
        }
        let project = projects.entry(c.project.clone()).or_insert(Project {
            name: c.project.clone(),
            total: 0,
            running: 0,
            services: String::new(),
        });
        project.total += 1;
        project.running += usize::from(c.state == "running");
        if !project.services.is_empty() {
            project.services.push('\n');
        }
        project.services.push_str(&format!(
            "{} · {} · {}",
            if c.service.is_empty() {
                &c.name
            } else {
                &c.service
            },
            c.state,
            c.name
        ));
    }
    projects.into_values().collect()
}

fn project_args(
    action: &str,
    containers: &[Container],
    project: &str,
) -> Result<Vec<String>, String> {
    if !["start", "stop"].contains(&action) {
        return Err(i18n::t("docker.invalid_action"));
    }
    let ids: Vec<_> = containers
        .iter()
        .filter(|c| !c.project.is_empty() && c.project == project)
        .map(|c| c.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(i18n::t("docker.project_missing"));
    }
    Ok([vec![action.to_owned(), "--".to_owned()], ids].concat())
}

#[derive(Deserialize)]
struct Stats {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "CPUPerc")]
    cpu: String,
    #[serde(rename = "MemUsage")]
    memory: String,
    #[serde(rename = "MemPerc")]
    memory_percent: String,
    #[serde(rename = "NetIO")]
    network: String,
    #[serde(rename = "BlockIO")]
    disk: String,
    #[serde(rename = "PIDs")]
    pids: String,
}

fn parse_stats(output: &str, id: &str) -> Result<String, String> {
    let stats: Stats = serde_json::from_str(output.trim())
        .map_err(|e| format!("{}: {e}", i18n::t("docker.invalid_output")))?;
    if stats.id != id {
        return Err(i18n::t("docker.invalid_output"));
    }
    Ok(format!(
        "CPU: {}\n{}: {} ({})\n{}: {}\n{}: {}\nPIDs: {}",
        stats.cpu,
        i18n::t("docker.memory"),
        stats.memory,
        stats.memory_percent,
        i18n::t("docker.network_io"),
        stats.network,
        i18n::t("docker.disk_io"),
        stats.disk,
        stats.pids
    ))
}

impl Target {
    pub async fn images(&self, containers: &[Container]) -> Result<Vec<Image>, String> {
        let (output, _) = self
            .run(
                &[
                    "image",
                    "ls",
                    "--all",
                    "--no-trunc",
                    "--format",
                    "{{json .}}",
                ]
                .map(str::to_owned),
            )
            .await?;
        let mut images = parse_images(&output)?;
        // Inspect immutable image IDs, because a tag may have moved since the container was created.
        for chunk in containers.chunks(100) {
            let mut args = vec![
                "container".into(),
                "inspect".into(),
                "--format".into(),
                "{{.Image}}".into(),
                "--".into(),
            ];
            args.extend(chunk.iter().map(|c| c.id.clone()));
            let (output, _) = self.run(&args).await?;
            let ids: Vec<_> = output.lines().filter(|s| !s.trim().is_empty()).collect();
            if ids.len() != chunk.len() || ids.iter().any(|id| !valid_image_id(id)) {
                return Err(i18n::t("docker.invalid_output"));
            }
            for id in ids {
                if let Some(image) = images.iter_mut().find(|i| i.id == id) {
                    image.references += 1;
                }
            }
        }
        Ok(images)
    }

    pub async fn pull_image(&self, reference: &str, source: &str) -> Result<(), String> {
        self.run_with_timeout(&pull_args(reference, source)?, Duration::from_secs(600))
            .await?;
        Ok(())
    }

    pub async fn remove_image(&self, id: &str) -> Result<(), String> {
        let args = remove_args(id)?;
        let (output, _) = self
            .run(
                &[
                    "ps",
                    "--all",
                    "--quiet",
                    "--filter",
                    &format!("ancestor={id}"),
                ]
                .map(str::to_owned),
            )
            .await?;
        if !output.trim().is_empty() {
            return Err(i18n::t("docker.image_in_use"));
        }
        self.run(&args).await?;
        Ok(())
    }

    pub async fn project_action(&self, action: &str, name: &str) -> Result<String, String> {
        let (_, containers) = self.list().await?;
        if action == "logs" {
            let ids = project_args("start", &containers, name)?;
            let mut logs = String::new();
            for id in &ids[2..] {
                let c = containers
                    .iter()
                    .find(|c| &c.id == id)
                    .ok_or_else(|| i18n::t("docker.project_missing"))?;
                let text = self.action("logs", id).await?;
                if logs.len() + text.len() + c.name.len() + 16 > MAX_OUTPUT as usize {
                    return Err(i18n::t("docker.output_limit"));
                }
                logs.push_str(&format!("\n=== {} ===\n{text}", c.name));
            }
            return Ok(logs);
        }
        let args = project_args(action, &containers, name)?;
        // Chunk large projects to stay below the Windows command-line length limit.
        for ids in args[2..].chunks(100) {
            let batch = [args[..2].to_vec(), ids.to_vec()].concat();
            self.run(&batch).await?;
        }
        Ok(String::new())
    }

    pub async fn stats(&self, id: &str) -> Result<String, String> {
        let (output, _) = self.run(&super::action_args("stats", id)?).await?;
        parse_stats(&output, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_source_is_optional_and_only_rewrites_docker_hub_images() {
        assert_eq!(
            resolve_pull_reference(" nginx:latest ", "").unwrap(),
            "nginx:latest"
        );
        assert_eq!(
            resolve_pull_reference("nginx:latest", "mirror.example.com").unwrap(),
            "mirror.example.com/library/nginx:latest"
        );
        assert_eq!(
            resolve_pull_reference("team/app:v1", "https://mirror.example.com/cache/").unwrap(),
            "mirror.example.com/cache/team/app:v1"
        );
        assert_eq!(
            resolve_pull_reference("docker.io/library/nginx", "localhost:5000").unwrap(),
            "localhost:5000/library/nginx"
        );
        assert_eq!(
            resolve_pull_reference("index.docker.io/team/app:v2", "mirror.example.com").unwrap(),
            "mirror.example.com/team/app:v2"
        );
        for image in [
            "ghcr.io/org/app:v1",
            "localhost:5000/app:v1",
            "registry.example.com/app",
        ] {
            assert_eq!(
                resolve_pull_reference(image, "mirror.example.com").unwrap(),
                image
            );
        }
        let digest = format!("nginx@sha256:{}", "a".repeat(64));
        assert_eq!(
            resolve_pull_reference(&digest, "mirror.example.com").unwrap(),
            format!("mirror.example.com/library/{digest}")
        );
        assert!(resolve_pull_reference("--help", "mirror.example.com").is_err());
        assert!(resolve_pull_reference("nginx;cmd", "mirror.example.com").is_err());
    }

    #[test]
    fn custom_source_accepts_registry_prefixes_and_rejects_urls_with_credentials_or_options() {
        assert_eq!(
            normalize_pull_source("  https://mirror.example.com:5000/cache/  ").unwrap(),
            "mirror.example.com:5000/cache"
        );
        assert_eq!(normalize_pull_source("   ").unwrap(), "");
        for source in [
            "--help",
            "http://mirror.example.com",
            "ftp://mirror.example.com",
            "https://user:pass@mirror.example.com",
            "https://mirror.example.com?token=abc",
            "https://mirror.example.com/#section",
            "mirror.example.com/a b",
            "mirror.example.com/../cache",
            "mirror.example.com//cache",
            "mirror",
            "mirror.example.com:99999",
            "mirror.example.com/测试",
        ] {
            assert!(normalize_pull_source(source).is_err(), "{source}");
        }
    }

    #[test]
    fn images_group_tags_by_full_id_and_reject_partial_output() {
        let id = format!("sha256:{}", "a".repeat(64));
        let row = |tag: &str| {
            format!(
                r#"{{"ID":"{id}","Repository":"nginx","Tag":"{tag}","Size":"10MB","CreatedSince":"2 days ago"}}"#
            )
        };
        let images = parse_images(&format!("{}\n{}\n", row("latest"), row("stable"))).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].tags, ["nginx:latest", "nginx:stable"]);
        assert!(parse_images(&format!("{}\n{{}}", row("latest"))).is_err());
        assert!(parse_images(&row("latest").replace(&id, "--help")).is_err());
        assert!(parse_images("").unwrap().is_empty());
    }

    #[test]
    fn image_commands_reject_flags_whitespace_and_never_force_delete() {
        for reference in [
            "",
            "--help",
            "nginx latest",
            "nginx\nredis",
            "$(whoami)",
            "nginx;cmd",
            "-x",
        ] {
            assert!(pull_args(reference, "").is_err(), "{reference}");
        }
        for reference in [
            "nginx",
            "nginx:latest",
            "localhost:5000/team/app:v1",
            "ghcr.io/org/app@sha256:abcd",
        ] {
            assert_eq!(
                pull_args(reference, "").unwrap(),
                ["image", "pull", "--", reference]
            );
        }
        assert!(remove_args("--help").is_err());
        let id = format!("sha256:{}", "a".repeat(64));
        assert_eq!(remove_args(&id).unwrap(), ["image", "rm", "--", &id]);
    }

    #[test]
    fn projects_match_exact_names_and_keep_stopped_services() {
        let rows = format!(
            "{}\n{}\n{}",
            container_json('a', "api", "running", "demo", "api"),
            container_json('b', "db", "exited", "demo", "db"),
            container_json('c', "other", "running", "", "")
        );
        let containers = super::super::parse_containers(&rows).unwrap();
        let groups = projects(&containers);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "demo");
        assert_eq!(groups[0].running, 1);
        assert_eq!(groups[0].total, 2);
        assert!(groups[0].services.contains("db · exited"));
        assert!(project_args("rm", &containers, "demo").is_err());
        assert!(project_args("stop", &containers, "dem").is_err());
        let args = project_args("stop", &containers, "demo").unwrap();
        assert_eq!(args, ["stop", "--", &"a".repeat(64), &"b".repeat(64)]);
    }

    #[test]
    fn stats_require_fields_and_a_full_matching_container_id() {
        let id = "a".repeat(64);
        let output = format!(
            r#"{{"ID":"{id}","CPUPerc":"1.2%","MemUsage":"3MiB / 1GiB","MemPerc":"0.3%","NetIO":"1kB / 2kB","BlockIO":"0B / 0B","PIDs":"3"}}"#
        );
        let text = parse_stats(&output, &id).unwrap();
        assert!(text.contains("1.2%"));
        assert!(text.contains("3MiB / 1GiB"));
        assert!(parse_stats(&output, &"b".repeat(64)).is_err());
        assert!(parse_stats("{}", &id).is_err());
        assert!(parse_stats("", &id).is_err());
    }

    fn container_json(id: char, name: &str, state: &str, project: &str, service: &str) -> String {
        format!(
            r#"{{"ID":"{}","Names":"{name}","Image":"nginx","State":"{state}","Status":"Up","Ports":"","Project":"{project}","Service":"{service}","Labels":"note=x,com.docker.compose.project=ignored"}}"#,
            id.to_string().repeat(64)
        )
    }
}
