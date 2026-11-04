// SPDX-License-Identifier: GPL-3.0-only

use super::{apply_list, clear_context, finish_request, selected_target};
use crate::docker::{
    Container, Image, Target, normalize_pull_source, projects, resolve_pull_reference,
};
use crate::{AppWindow, DockerResource, i18n};
use slint::{Model, ModelRc, VecModel};

pub(super) fn filter_list(app: &AppWindow) {
    let query = app.get_docker_query().trim().to_lowercase();
    let rows = app
        .get_docker_resources()
        .iter()
        .filter(|r| {
            r.name.to_lowercase().contains(&query) || r.details.to_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    app.set_docker_filtered_resources(ModelRc::new(VecModel::from(rows)));
}

type Snapshot = (String, Vec<Container>, Option<Vec<Image>>);

async fn load(target: &Target, section: i32) -> Result<Snapshot, String> {
    let (version, containers) = target.list().await?;
    let images = if section == 1 {
        Some(target.images(&containers).await?)
    } else {
        None
    };
    Ok((version, containers, images))
}

fn apply(app: &AppWindow, result: Result<Snapshot, String>) {
    match result {
        Ok((version, containers, images)) => {
            let rows: Vec<_> = if let Some(images) = images {
                images
                    .into_iter()
                    .map(|image| {
                        let tags = image.tags.join("\n");
                        let summary = i18n::tr(
                            "docker.image_summary",
                            &[
                                image.size.clone(),
                                image.references.to_string(),
                                image.created.clone(),
                            ],
                        );
                        DockerResource {
                            id: image.id.clone().into(),
                            name: image
                                .tags
                                .first()
                                .cloned()
                                .unwrap_or_else(|| image.id.clone())
                                .into(),
                            details: format!("{tags}\n{}\n{summary}", image.id).into(),
                            summary: summary.into(),
                            in_use: image.references > 0,
                            ..Default::default()
                        }
                    })
                    .collect()
            } else {
                projects(&containers)
                    .into_iter()
                    .map(|p| DockerResource {
                        id: p.name.clone().into(),
                        name: p.name.into(),
                        summary: i18n::tr(
                            "docker.project_summary",
                            &[p.running.to_string(), p.total.to_string()],
                        )
                        .into(),
                        details: p.services.into(),
                        running: p.running as i32,
                        total: p.total as i32,
                        ..Default::default()
                    })
                    .collect()
            };
            app.set_docker_resources(ModelRc::new(VecModel::from(rows)));
            apply_list(app, Ok((version, containers)));
        }
        Err(error) => {
            app.set_docker_status(i18n::t("docker.cached_results").into());
            app.set_docker_error(error.into());
            finish_request(app);
        }
    }
}

pub(super) fn refresh(app: &AppWindow, ah: slint::Weak<AppWindow>, target: Target) {
    let section = app.get_docker_section();
    tokio::spawn(async move {
        let result = load(&target, section).await;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = ah.upgrade() {
                apply(&app, result);
            }
        });
    });
}

fn action(app: &AppWindow, ah: slint::Weak<AppWindow>, operation: &str, id: &str, confirmed: bool) {
    if app.get_docker_busy() || !app.get_docker_error().is_empty() {
        return;
    }
    let section = app.get_docker_section();
    let resource = app.get_docker_resources().iter().find(|r| r.id == id);
    let valid = match operation {
        "pull" => section == 1,
        "remove" => section == 1 && resource.as_ref().is_some_and(|r| !r.in_use),
        "project-start" | "project-stop" | "project-logs" => section == 2 && resource.is_some(),
        _ => false,
    };
    if !valid {
        return;
    }
    let Some(target) = selected_target(app) else {
        return;
    };
    if matches!(operation, "remove" | "project-stop") && !confirmed {
        app.set_docker_pending_action(operation.into());
        app.set_docker_pending_id(id.into());
        let key = if operation == "remove" {
            "docker.confirm_remove"
        } else {
            "docker.confirm_project_stop"
        };
        let name = resource
            .as_ref()
            .map(|r| r.name.to_string())
            .unwrap_or_else(|| id.to_owned());
        app.set_docker_confirm_message(i18n::tr(key, &[name]).into());
        app.set_show_docker_confirm(true);
        return;
    }
    let is_logs = operation == "project-logs";
    if is_logs {
        if app.get_docker_log_id() != id
            || app.get_docker_log_kind() != "project"
            || !app.get_show_docker_logs()
        {
            app.set_docker_logs("".into());
            app.set_docker_log_auto(false);
        }
        app.set_docker_log_id(id.into());
        app.set_docker_log_kind("project".into());
        app.set_docker_log_title(id.into());
        app.set_docker_log_error("".into());
        app.set_show_docker_logs(true);
    }
    app.set_docker_busy(true);
    app.set_docker_busy_id(id.into());
    app.set_docker_operation(
        i18n::t(if operation == "pull" {
            "docker.pulling"
        } else {
            "docker.loading"
        })
        .into(),
    );
    app.set_docker_notice("".into());
    let operation = operation.to_owned();
    let id = id.to_owned();
    let source = app.get_docker_saved_pull_source().to_string();
    tokio::spawn(async move {
        let result = match operation.as_str() {
            "pull" => target
                .pull_image(&id, &source)
                .await
                .map(|()| String::new()),
            "remove" => target.remove_image(&id).await.map(|()| String::new()),
            _ => {
                target
                    .project_action(operation.strip_prefix("project-").unwrap_or(""), &id)
                    .await
            }
        };
        let snapshot = if !is_logs {
            Some(load(&target, section).await)
        } else {
            None
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = ah.upgrade() else { return };
            if let Some(snapshot) = snapshot {
                apply(&app, snapshot);
            } else {
                finish_request(&app);
            }
            match result {
                Ok(logs) if is_logs => app.set_docker_logs(logs.into()),
                Ok(_) => {
                    app.set_docker_notice(i18n::t("docker.resource_success").into());
                    if operation == "pull" {
                        app.set_docker_pull_reference("".into());
                        app.invoke_preview_docker_pull();
                    }
                }
                Err(error) if is_logs => {
                    app.set_docker_log_error(error.into());
                    app.set_docker_log_auto(false);
                }
                Err(error) => app.set_docker_error(error.into()),
            }
        });
    });
}

pub(super) fn setup(
    app: &AppWindow,
    ah: slint::Weak<AppWindow>,
    app_state: std::sync::Arc<tokio::sync::Mutex<crate::AppState>>,
) {
    let handle = ah.clone();
    app.on_preview_docker_pull(move || {
        let Some(app) = handle.upgrade() else { return };
        let source = app.get_docker_saved_pull_source();
        let reference = app.get_docker_pull_reference();
        let result = if reference.trim().is_empty() {
            normalize_pull_source(&source).map(|_| String::new())
        } else {
            resolve_pull_reference(&reference, &source)
        };
        match result {
            Ok(target) => {
                app.set_docker_pull_target(target.into());
                app.set_docker_pull_error("".into());
            }
            Err(error) => {
                app.set_docker_pull_target("".into());
                app.set_docker_pull_error(error.into());
            }
        }
    });
    let handle = ah.clone();
    app.on_docker_pull_source_action(move |operation, value| {
        let Some(app) = handle.upgrade() else { return };
        if app.get_docker_busy() {
            return;
        }
        let value = match operation.as_str() {
            "add" => match normalize_pull_source(&value) {
                Ok(source) if !source.is_empty() => source,
                _ => {
                    app.set_docker_pull_source_error(i18n::t("docker.invalid_pull_source").into());
                    return;
                }
            },
            "select" => value.to_string(),
            "remove" if !value.is_empty() => value.to_string(),
            _ => return,
        };
        app.set_docker_busy(true);
        app.set_docker_operation(i18n::t("docker.saving_pull_source").into());
        app.set_docker_pull_source_error("".into());
        let handle = handle.clone();
        let state = app_state.clone();
        tokio::spawn(async move {
            let result = {
                let mut state = state.lock().await;
                let mut settings = state.config_manager.get_settings().clone();
                match operation.as_str() {
                    "add" => {
                        if !settings.docker_pull_sources.contains(&value) {
                            settings.docker_pull_sources.push(value.clone());
                        }
                        settings.docker_pull_source = value;
                    }
                    "select" => settings.docker_pull_source = value,
                    "remove" => {
                        settings
                            .docker_pull_sources
                            .retain(|source| source != &value);
                        if settings.docker_pull_source == value {
                            settings.docker_pull_source.clear();
                        }
                    }
                    _ => unreachable!(),
                }
                let sources = settings.docker_pull_sources.clone();
                let current = settings.docker_pull_source.clone();
                state
                    .config_manager
                    .update_settings(settings)
                    .map(|()| (sources, current))
                    .map_err(|e| e.to_string())
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = handle.upgrade() else { return };
                finish_request(&app);
                match result {
                    Ok((sources, current)) => {
                        super::set_pull_sources(&app, sources, current);
                        if operation == "add" {
                            app.set_docker_pull_source("".into());
                        }
                        app.set_docker_notice(i18n::t("docker.pull_source_saved").into());
                        app.invoke_preview_docker_pull();
                    }
                    Err(error) => {
                        let current = app.get_docker_saved_pull_source();
                        let index = if current.is_empty() {
                            0
                        } else {
                            app.get_docker_pull_sources()
                                .iter()
                                .position(|source| source == current)
                                .unwrap_or(0)
                        };
                        app.set_docker_pull_source_index(index as i32);
                        app.set_docker_pull_source_error(
                            format!("{}: {error}", i18n::t("docker.pull_source_save_failed"))
                                .into(),
                        );
                    }
                }
            });
        });
    });
    let handle = ah.clone();
    app.on_select_docker_section(move |section| {
        let Some(app) = handle.upgrade() else { return };
        if app.get_docker_busy() || !(0..=2).contains(&section) {
            return;
        }
        clear_context(&app);
        app.set_docker_section(section);
        app.invoke_refresh_docker();
    });
    let handle = ah.clone();
    app.on_docker_resource_action(move |operation, id| {
        if let Some(app) = handle.upgrade() {
            action(&app, handle.clone(), &operation, &id, false);
        }
    });
    app.on_confirm_docker_resource(move || {
        let Some(app) = ah.upgrade() else { return };
        if !app.get_show_docker_confirm() {
            return;
        }
        let operation = app.get_docker_pending_action();
        let id = app.get_docker_pending_id();
        app.set_show_docker_confirm(false);
        app.set_docker_pending_action("".into());
        app.set_docker_pending_id("".into());
        action(&app, ah.clone(), &operation, &id, true);
    });
}
