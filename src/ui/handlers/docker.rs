// SPDX-FileCopyrightText: Copyright (c) 2026 owu <wqh@live.com>
// SPDX-License-Identifier: GPL-3.0-only

use crate::docker::{Container, Target, matches_filter};
use crate::{AppWindow, DockerContainer, i18n};
use slint::{Model, ModelRc, VecModel};

#[path = "docker/resources.rs"]
mod resources;

fn selected_target(app: &AppWindow) -> Option<Target> {
    let index = usize::try_from(app.get_docker_target_index()).ok()?;
    if index == 0 {
        return Some(Target::Desktop);
    }
    let name = app.get_docker_targets().row_data(index)?.to_string();
    Some(Target::Wsl {
        distro: name.strip_prefix("WSL: ")?.to_owned(),
        as_root: app.get_docker_as_root(),
    })
}

pub fn set_pull_sources(app: &AppWindow, mut sources: Vec<String>, current: String) {
    if !current.is_empty() && !sources.contains(&current) {
        sources.push(current.clone());
    }
    let index = if current.is_empty() {
        0
    } else {
        sources
            .iter()
            .position(|source| source == &current)
            .map(|i| i + 1)
            .unwrap_or(0)
    };
    let mut choices = vec![slint::SharedString::from(i18n::t(
        "docker.default_pull_source",
    ))];
    choices.extend(sources.into_iter().map(slint::SharedString::from));
    app.set_docker_pull_sources(ModelRc::new(VecModel::from(choices)));
    app.set_docker_pull_source_index(index as i32);
    app.set_docker_saved_pull_source(current.into());
}

fn filter_list(app: &AppWindow) {
    let query = app.get_docker_query();
    let filter = app.get_docker_filter();
    let rows = app
        .get_docker_containers()
        .iter()
        .filter(|c| matches_filter(&c.name, &c.image, &c.state, &query, filter))
        .collect::<Vec<_>>();
    app.set_docker_filtered_containers(ModelRc::new(VecModel::from(rows)));
    resources::filter_list(app);
}

fn clear_context(app: &AppWindow) {
    app.set_docker_containers(ModelRc::default());
    app.set_docker_filtered_containers(ModelRc::default());
    app.set_docker_query("".into());
    app.set_docker_filter(0);
    app.set_docker_expanded_id("".into());
    app.set_docker_scroll_y(0.0);
    app.set_show_docker_logs(false);
    app.set_docker_logs("".into());
    app.set_docker_log_id("".into());
    app.set_docker_log_title("".into());
    app.set_docker_log_error("".into());
    app.set_docker_notice("".into());
    app.set_docker_resources(ModelRc::default());
    app.set_docker_filtered_resources(ModelRc::default());
    app.set_docker_pull_reference("".into());
    app.set_docker_pull_target("".into());
    app.set_docker_pull_error("".into());
    app.set_docker_log_auto(false);
    app.set_docker_log_kind("logs".into());
    app.set_show_docker_confirm(false);
    app.set_docker_pending_action("".into());
    app.set_docker_pending_id("".into());
}

fn finish_request(app: &AppWindow) {
    app.set_docker_busy(false);
    app.set_docker_busy_id("".into());
    app.set_docker_operation("".into());
}

fn apply_list(app: &AppWindow, result: Result<(String, Vec<Container>), String>) {
    match result {
        Ok((version, containers)) => {
            let rows = containers
                .into_iter()
                .map(|c| DockerContainer {
                    id: c.id.into(),
                    name: c.name.into(),
                    image: c.image.into(),
                    state: c.state.into(),
                    status: c.status.into(),
                    ports: c.ports.into(),
                })
                .collect::<Vec<_>>();
            app.set_docker_containers(ModelRc::new(VecModel::from(rows)));
            filter_list(app);
            app.set_docker_status(format!("{} · {version}", i18n::t("docker.ready")).into());
            app.set_docker_error("".into());
        }
        Err(error) => {
            app.set_docker_status(if app.get_docker_containers().row_count() > 0 {
                i18n::t("docker.cached_results").into()
            } else {
                "".into()
            });
            app.set_docker_error(error.into());
        }
    }
    finish_request(app);
}

pub fn setup(
    app: &AppWindow,
    app_handle: slint::Weak<AppWindow>,
    app_state: std::sync::Arc<tokio::sync::Mutex<crate::AppState>>,
) {
    resources::setup(app, app_handle.clone(), app_state);
    let ah = app_handle.clone();
    app.on_filter_docker(move || {
        if let Some(app) = ah.upgrade() {
            app.set_docker_scroll_y(0.0);
            filter_list(&app);
        }
    });

    let ah = app_handle.clone();
    app.on_copy_docker_text(move |text| {
        let ah = ah.clone();
        tokio::spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || crate::utils::system::copy_to_clipboard(&text))
                    .await;
            let message = match result {
                Ok(Ok(())) => i18n::t("docker.copied"),
                Ok(Err(error)) => format!("{}: {error}", i18n::t("docker.copy_failed")),
                Err(error) => format!("{}: {error}", i18n::t("docker.copy_failed")),
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = ah.upgrade() {
                    app.set_docker_notice(message.into());
                }
            });
        });
    });

    let ah = app_handle.clone();
    app.on_refresh_docker(move || {
        let Some(app) = ah.upgrade() else { return };
        if app.get_docker_busy() {
            return;
        }
        let previous = app
            .get_docker_targets()
            .row_data(app.get_docker_target_index() as usize);
        let mut targets = vec![slint::SharedString::from("Docker Desktop")];
        targets.extend(
            app.get_distros()
                .iter()
                .filter(|d| d.version == "2" && !d.name.starts_with("docker-desktop"))
                .map(|d| format!("WSL: {}", d.name).into()),
        );
        let index = previous
            .as_ref()
            .and_then(|name| targets.iter().position(|target| target == name))
            .unwrap_or(0);
        if previous.as_ref() != Some(&targets[index]) {
            clear_context(&app);
        }
        app.set_docker_targets(ModelRc::new(VecModel::from(targets)));
        app.set_docker_target_index(index as i32);
        if index == 0 {
            app.set_docker_as_root(false);
        }
        let Some(target) = selected_target(&app) else {
            return;
        };
        app.set_docker_initialized(true);
        app.set_docker_busy(true);
        app.set_docker_error("".into());
        app.set_docker_notice("".into());
        app.set_docker_status(i18n::t("docker.loading").into());
        if app.get_docker_section() != 0 {
            resources::refresh(&app, ah.clone(), target);
            return;
        }
        let ah = ah.clone();
        tokio::spawn(async move {
            let result = target.list().await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = ah.upgrade() {
                    apply_list(&app, result);
                }
            });
        });
    });

    let ah = app_handle.clone();
    app.on_select_docker_target(move |index| {
        if let Some(app) = ah.upgrade() {
            if !app.get_docker_busy()
                && index >= 0
                && (index as usize) < app.get_docker_targets().row_count()
            {
                if index != app.get_docker_target_index() {
                    clear_context(&app);
                    app.set_docker_as_root(false);
                }
                app.set_docker_target_index(index);
                app.invoke_refresh_docker();
            }
        }
    });

    let ah = app_handle.clone();
    app.on_set_docker_root(move |enabled| {
        if let Some(app) = ah.upgrade() {
            if !app.get_docker_busy() && app.get_docker_target_index() > 0 {
                clear_context(&app);
                app.set_docker_as_root(enabled);
                app.invoke_refresh_docker();
            }
        }
    });

    let ah = app_handle;
    app.on_docker_action(move |action, id| {
        let Some(app) = ah.upgrade() else { return };
        if app.get_docker_busy() || !app.get_docker_error().is_empty() {
            return;
        }
        let Some(container) = app.get_docker_containers().iter().find(|c| c.id == id) else {
            return;
        };
        let operation = match action.as_str() {
            "start" => "docker.starting",
            "stop" => "docker.stopping",
            "restart" => "docker.restarting",
            "logs" => "docker.loading_logs",
            "stats" if container.state == "running" => "docker.loading",
            _ => return,
        };
        let Some(target) = selected_target(&app) else {
            return;
        };
        app.set_docker_busy(true);
        app.set_docker_busy_id(id.clone());
        app.set_docker_operation(i18n::t(operation).into());
        app.set_docker_notice("".into());
        if action == "logs" || action == "stats" {
            if app.get_docker_log_id() != id
                || app.get_docker_log_kind() != action
                || !app.get_show_docker_logs()
            {
                app.set_docker_logs("".into());
                app.set_docker_log_auto(action == "stats");
            }
            app.set_docker_log_kind(action.clone());
            app.set_docker_log_id(id.clone());
            app.set_docker_log_title(container.name.clone());
            app.set_docker_log_error("".into());
            app.set_show_docker_logs(true);
        }
        let ah = ah.clone();
        tokio::spawn(async move {
            let result = if action == "stats" {
                target.stats(&id).await
            } else {
                target.action(&action, &id).await
            };
            let list = if action != "logs" && action != "stats" && result.is_ok() {
                Some(target.list().await)
            } else {
                None
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = ah.upgrade() {
                    if let Some(list) = list {
                        apply_list(&app, list);
                        app.set_docker_notice(
                            i18n::tr(
                                "docker.action_success",
                                &[
                                    i18n::t(&format!("docker.{action}")),
                                    container.name.to_string(),
                                ],
                            )
                            .into(),
                        );
                    } else {
                        finish_request(&app);
                        match result {
                            Ok(logs) => app.set_docker_logs(logs.into()),
                            Err(error) if action == "logs" || action == "stats" => {
                                app.set_docker_log_error(error.into());
                                app.set_docker_log_auto(false);
                            }
                            Err(error) => app.set_docker_error(error.into()),
                        }
                    }
                }
            });
        });
    });
}
