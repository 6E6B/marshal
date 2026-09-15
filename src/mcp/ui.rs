use super::{
    canonical::{ConfigValue, McpServer},
    detection::Platform,
    harnesses::Target,
    operations::{ConflictResolution, TargetActionTaken, TargetStatus},
    registry::HarnessRegistry,
};
use crate::backend::Manager;
use adw::prelude::*;
use anyhow::Context;
use gtk::glib;
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// Shows a dialog to install or remove the specified server in detected agent client harnesses.
pub fn manage_clients_dialog(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    manager: Arc<Manager>,
    server: &crate::config::Server,
) {
    let platform = match Platform::current(None) {
        Ok(p) => p,
        Err(e) => {
            overlay.add_toast(adw::Toast::new(&format!("Cannot detect platform: {e:#}")));
            return;
        }
    };

    let snap = manager.snapshot(&server.id);
    let local_bridge_url = snap.local_endpoint.clone().or_else(|| {
        server
            .bridge_port
            .map(|port| format!("http://127.0.0.1:{port}/{}/mcp", server.id))
    });

    let mut canonical_direct = McpServer::from_managed(server);
    // Direct installs need real header values: clients connect upstream
    // themselves, so keyring-backed secrets are resolved to literals here.
    if let Ok(resolved) = manager.resolved_headers(server) {
        for (key, value) in resolved {
            canonical_direct
                .headers
                .insert(key, ConfigValue::Literal { value });
        }
    }
    let canonical_bridge = local_bridge_url
        .as_ref()
        .map(|url| McpServer::from_managed_bridge(server, url));
    let has_bridge = canonical_bridge.is_some();
    let initial_canonical = canonical_bridge
        .clone()
        .unwrap_or_else(|| canonical_direct.clone());

    let registry = HarnessRegistry::new();
    let detected_installations = registry.detect_installations(&platform);
    let detected_ids: std::collections::HashSet<String> = detected_installations
        .iter()
        .map(|i| i.adapter_id.clone())
        .collect();

    let dialog = adw::PreferencesDialog::builder()
        .title("Manage MCP Clients")
        .content_width(620)
        .content_height(540)
        .build();

    let page = adw::PreferencesPage::new();

    // Mode Group
    let mode_group = adw::PreferencesGroup::builder()
        .title("Connection Type")
        .build();

    let mode_row = adw::ComboRow::builder()
        .title("Harness Connection")
        .model(&gtk::StringList::new(&[
            "Local Bridge (HTTP) — shared server instance",
            "Direct Launch (Stdio) — independent process",
        ]))
        .selected(if has_bridge { 0 } else { 1 })
        .build();

    if !has_bridge {
        mode_row.set_sensitive(false);
        mode_row.set_subtitle("Start the server in Marshal to enable the Local Bridge");
    } else if let Some(ref url) = local_bridge_url {
        mode_row.set_subtitle(&format!("Shared endpoint: {url}"));
    }
    mode_group.add(&mode_row);
    page.add(&mode_group);

    // Clients Group
    let clients_group = adw::PreferencesGroup::builder()
        .title("Detected MCP Clients")
        .description(format!(
            "Checked clients will be configured with '{}'; unchecked clients will have it removed.",
            server.name
        ))
        .build();

    struct TargetItem {
        target: Target,
        check: gtk::CheckButton,
        row: adw::ActionRow,
        base_subtitle: String,
        status: RefCell<TargetStatus>,
    }

    let items: Rc<RefCell<Vec<TargetItem>>> = Rc::new(RefCell::new(Vec::new()));
    let mut has_conflicts = false;

    // Iterate through all adapters, prioritizing detected ones
    for adapter in registry.adapters() {
        let is_detected = detected_ids.contains(adapter.id());
        let installation = detected_installations
            .iter()
            .find(|i| i.adapter_id == adapter.id());
        let targets = adapter.targets(&platform);
        if targets.is_empty() {
            continue;
        }

        // If not detected and config doesn't exist, we can skip or show collapsed
        let has_existing_config = targets.iter().any(|t| t.path.exists());
        if !is_detected && !has_existing_config {
            continue;
        }

        let has_multiple_targets = targets.len() > 1;
        for target in targets {
            let status = registry
                .check_status(&initial_canonical, &target)
                .unwrap_or(TargetStatus::NotInstalled);

            let check = gtk::CheckButton::new();
            check.set_valign(gtk::Align::Center);
            check.set_active(!matches!(status, TargetStatus::NotInstalled));

            let title = if has_multiple_targets {
                format!("{} ({})", adapter.display_name(), target.scope.label())
            } else {
                adapter.display_name().to_string()
            };

            let row = adw::ActionRow::builder()
                .title(&title)
                .activatable_widget(&check)
                .build();

            let mut subtitle = target.label();
            if let Some(inst) = installation
                && let Some(notice) = &inst.notice
            {
                subtitle = format!("{subtitle} · {notice}");
            }
            let base_subtitle = subtitle.clone();
            subtitle = target_status_subtitle(&base_subtitle, &status);

            match &status {
                TargetStatus::Identical => {
                    row.add_css_class("dim-label");
                }
                TargetStatus::Conflict { .. } => {
                    has_conflicts = true;
                    row.add_css_class("warning");
                }
                TargetStatus::NotInstalled => {}
            }

            row.set_subtitle(&subtitle);
            row.add_suffix(&check);
            clients_group.add(&row);

            items.borrow_mut().push(TargetItem {
                target,
                check,
                row,
                base_subtitle,
                status: RefCell::new(status),
            });
        }
    }

    if items.borrow().is_empty() {
        clients_group.add(
            &adw::ActionRow::builder()
                .title("No MCP Clients Detected")
                .subtitle("Install a client like Claude Code, VS Code, or Codex first")
                .build(),
        );
    }

    page.add(&clients_group);

    // Conflict resolution options
    let resolution_row = adw::ComboRow::builder()
        .title("When conflict is found")
        .model(&gtk::StringList::new(&[
            "Replace existing configuration",
            "Skip conflicting clients",
            "Add as renamed server (-copy)",
        ]))
        .selected(1)
        .build();

    if has_conflicts {
        let resolution_group = adw::PreferencesGroup::builder()
            .title("Conflict Options")
            .description(
                "How to handle clients that already configure this server name differently:",
            )
            .build();
        resolution_group.add(&resolution_row);
        page.add(&resolution_group);
    }

    // Apply button
    let apply_btn = gtk::Button::builder()
        .label("Apply Changes")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .sensitive(false)
        .build();

    let update_apply_sensitivity = {
        let items = items.clone();
        let apply_btn = apply_btn.clone();
        let resolution_row = resolution_row.clone();
        move || {
            let skip_conflicts = resolution_row.selected() == 1;
            let dirty = items.borrow().iter().any(|item| {
                let status = item.status.borrow();
                let installed = !matches!(*status, TargetStatus::NotInstalled);
                if item.check.is_active() != installed {
                    return true;
                }
                item.check.is_active()
                    && matches!(*status, TargetStatus::Conflict { .. })
                    && !skip_conflicts
            });
            apply_btn.set_sensitive(dirty);
        }
    };

    for item in items.borrow().iter() {
        let update = update_apply_sensitivity.clone();
        item.check.connect_toggled(move |_| update());
    }
    {
        let update = update_apply_sensitivity.clone();
        resolution_row.connect_selected_notify(move |_| update());
    }

    let update_statuses = {
        let items = items.clone();
        let canonical_bridge = canonical_bridge.clone();
        let canonical_direct = canonical_direct.clone();
        let update_sensitivity = update_apply_sensitivity.clone();
        move |selected_mode: u32| {
            let active_canonical = match (selected_mode, canonical_bridge.as_ref()) {
                (0, Some(bridge)) => bridge,
                _ => &canonical_direct,
            };
            let reg = HarnessRegistry::new();
            for item in items.borrow().iter() {
                let status = reg
                    .check_status(active_canonical, &item.target)
                    .unwrap_or(TargetStatus::NotInstalled);
                item.row
                    .set_subtitle(&target_status_subtitle(&item.base_subtitle, &status));
                item.row.remove_css_class("warning");
                item.row.remove_css_class("dim-label");
                match &status {
                    TargetStatus::Identical => item.row.add_css_class("dim-label"),
                    TargetStatus::Conflict { .. } => item.row.add_css_class("warning"),
                    TargetStatus::NotInstalled => {}
                }
                *item.status.borrow_mut() = status;
            }
            update_sensitivity();
        }
    };
    mode_row.connect_selected_notify(move |row| update_statuses(row.selected()));

    let dialog_weak = dialog.downgrade();
    let overlay_clone = overlay.clone();
    let server_clone = server.clone();

    apply_btn.connect_clicked(move |btn| {
        let Some(dialog) = dialog_weak.upgrade() else {
            return;
        };

        let skip_conflicts = resolution_row.selected() == 1;
        let resolution = match resolution_row.selected() {
            1 => ConflictResolution::Skip,
            2 => ConflictResolution::Rename(format!("{}-copy", server_clone.name)),
            _ => ConflictResolution::Overwrite,
        };

        let mut install_targets = Vec::new();
        let mut remove_targets = Vec::new();
        for item in items.borrow().iter() {
            let status = item.status.borrow().clone();
            let installed = !matches!(status, TargetStatus::NotInstalled);
            if item.check.is_active() {
                if !installed
                    || (matches!(status, TargetStatus::Conflict { .. }) && !skip_conflicts)
                {
                    install_targets.push(item.target.clone());
                }
            } else if installed {
                remove_targets.push(item.target.clone());
            }
        }

        if install_targets.is_empty() && remove_targets.is_empty() {
            dialog.add_toast(adw::Toast::new("No changes to apply"));
            return;
        }

        btn.set_sensitive(false);
        dialog.set_can_close(false);

        let mode_selected = mode_row.selected();
        let manager_clone = manager.clone();
        let canonical_direct_task = canonical_direct.clone();
        let local_bridge_url_task = local_bridge_url.clone();
        let server_id_task = server_clone.id.clone();
        let server_clone_task = server_clone.clone();
        let overlay_toast = overlay_clone.clone();
        let dialog_to_close = dialog.clone();

        let task = manager.runtime.spawn(async move {
            let install_reports = if install_targets.is_empty() {
                Vec::new()
            } else {
                let canonical_to_write = if mode_selected == 0 {
                    let bridge_url =
                        match manager_clone.ensure_local_bridge(&server_id_task).await {
                            Ok(url) => url,
                            Err(_) => local_bridge_url_task
                                .context("Local bridge could not be started for this server")?,
                        };
                    McpServer::from_managed_bridge(&server_clone_task, &bridge_url)
                } else {
                    canonical_direct_task
                };
                HarnessRegistry::new().install_server(
                    &canonical_to_write,
                    &install_targets,
                    resolution,
                )?
            };
            let remove_reports = if remove_targets.is_empty() {
                Vec::new()
            } else {
                HarnessRegistry::new().remove_server(&server_clone_task.name, &remove_targets)?
            };
            Ok::<_, anyhow::Error>((install_reports, remove_reports))
        });

        glib::spawn_future_local(async move {
            let result = task.await;
            dialog_to_close.set_can_close(true);
            match result {
                Ok(Ok((install_reports, remove_reports))) => {
                    let mut installed = 0;
                    let mut replaced = 0;
                    let mut skipped = 0;
                    let mut already_ok = 0;
                    for r in install_reports {
                        match r.action {
                            TargetActionTaken::Installed => installed += 1,
                            TargetActionTaken::Replaced => replaced += 1,
                            TargetActionTaken::Skipped => skipped += 1,
                            TargetActionTaken::AlreadyUpToDate => already_ok += 1,
                            TargetActionTaken::Removed => {}
                        }
                    }
                    let removed = remove_reports
                        .iter()
                        .filter(|r| matches!(r.action, TargetActionTaken::Removed))
                        .count();

                    dialog_to_close.close();
                    overlay_toast.add_toast(adw::Toast::new(&format!(
                        "Client configuration updated: {installed} installed, {replaced} replaced, {removed} removed{}{}",
                        if already_ok > 0 {
                            format!(", {already_ok} identical")
                        } else {
                            String::new()
                        },
                        if skipped > 0 {
                            format!(", {skipped} skipped")
                        } else {
                            String::new()
                        },
                    )));
                }
                Ok(Err(e)) => {
                    dialog_to_close.add_toast(adw::Toast::new(&format!("Update failed: {e:#}")));
                }
                Err(e) => {
                    dialog_to_close.add_toast(adw::Toast::new(&format!("System error: {e}")));
                }
            }
        });
    });

    // Add Apply button as a centered action at bottom of page
    let action_group = adw::PreferencesGroup::new();
    action_group.add(&apply_btn);
    page.add(&action_group);

    dialog.add(&page);
    dialog.present(Some(parent));
}

/// Shows the Agent Clients & Global MCP Inventory dialog.
pub fn clients_inventory_dialog(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    manager: Arc<Manager>,
    on_import: impl Fn() + 'static,
) {
    let platform = match Platform::current(None) {
        Ok(p) => p,
        Err(e) => {
            overlay.add_toast(adw::Toast::new(&format!("Cannot detect platform: {e:#}")));
            return;
        }
    };

    let registry = HarnessRegistry::new();
    let detected = registry.detect_installations(&platform);

    let dialog = adw::PreferencesDialog::builder()
        .title("Agent Clients & MCP Inventory")
        .content_width(680)
        .content_height(580)
        .build();

    let page = adw::PreferencesPage::new();

    // Group 1: Detected Clients
    let clients_group = adw::PreferencesGroup::builder()
        .title("Detected Agent Harnesses")
        .description("Agent tools and IDEs found on this system:")
        .build();

    if detected.is_empty() {
        let empty_row = adw::ActionRow::builder()
            .title("No Agent Clients Detected")
            .subtitle("Install Claude Code, OpenAI Codex, VS Code, Cursor, Gemini CLI, etc.")
            .build();
        clients_group.add(&empty_row);
    } else {
        for inst in &detected {
            let adapter = registry.adapter(&inst.adapter_id);
            let display_name = adapter
                .map(|a| a.display_name())
                .unwrap_or(&inst.adapter_id);
            let mut sub = format!("Confidence: {}", inst.confidence);
            if let Some(exe) = &inst.executable {
                sub = format!("{sub} · Binary: {}", exe.display());
            }
            if let Some(notice) = &inst.notice {
                sub = format!("{sub} · {notice}");
            }
            let row = adw::ActionRow::builder()
                .title(display_name)
                .subtitle(&sub)
                .build();
            clients_group.add(&row);
        }
    }
    page.add(&clients_group);

    // Group 2: Cross-Harness Inventory
    let inventory_group = adw::PreferencesGroup::builder()
        .title("Discovered MCP Servers")
        .description("Servers configured across all detected clients and project workspaces:")
        .build();

    let inventory = registry.scan_inventory(&platform).unwrap_or_default();
    let managed_servers = manager.servers();
    let on_import = Rc::new(on_import);

    if inventory.is_empty() {
        let empty_row = adw::ActionRow::builder()
            .title("No MCP Servers Configured")
            .subtitle("No servers found in client configuration files")
            .build();
        inventory_group.add(&empty_row);
    } else {
        for entry in inventory {
            let is_already_managed = managed_servers.iter().any(|s| s.name == entry.name);
            let client_names: Vec<_> = entry
                .installations
                .iter()
                .map(|(t, _)| {
                    let name = registry
                        .adapter(&t.adapter_id)
                        .map(|a| a.display_name())
                        .unwrap_or(&t.adapter_id);
                    format!("{} ({})", name, t.scope.label())
                })
                .collect();

            let row = adw::ActionRow::builder().title(&entry.name).build();

            let mut subtitle = format!(
                "{} · Configured in: {}",
                entry.canonical.summary(),
                client_names.join(", ")
            );

            if entry.has_drift {
                subtitle = format!("⚠️ Configuration drift · {subtitle}");
                row.add_css_class("warning");
            }

            row.set_subtitle(&subtitle);

            if is_already_managed {
                let badge = gtk::Label::builder()
                    .label("Managed in Marshal")
                    .css_classes(["dim-label"])
                    .valign(gtk::Align::Center)
                    .build();
                row.add_suffix(&badge);
            } else {
                let import_btn = gtk::Button::builder()
                    .label("Import to Marshal")
                    .css_classes(["pill"])
                    .valign(gtk::Align::Center)
                    .build();

                let manager_clone = manager.clone();
                let server_to_save = entry.canonical.to_managed();
                let overlay_clone = overlay.clone();
                let dialog_clone = dialog.clone();
                let on_import_clone = on_import.clone();

                import_btn.connect_clicked(move |btn| {
                    btn.set_sensitive(false);
                    let m = manager_clone.clone();
                    let s = server_to_save.clone();
                    let overlay_toast = overlay_clone.clone();
                    let dialog_toast = dialog_clone.clone();
                    let name = s.name.clone();
                    let callback = on_import_clone.clone();

                    let task = {
                        let m2 = m.clone();
                        m.runtime.spawn(async move { m2.save(s, vec![]).await })
                    };

                    glib::spawn_future_local(async move {
                        match task.await {
                            Ok(Ok(())) => {
                                dialog_toast.close();
                                overlay_toast.add_toast(adw::Toast::new(&format!(
                                    "Imported server '{}' into Marshal",
                                    name
                                )));
                                callback();
                            }
                            Ok(Err(e)) => {
                                dialog_toast
                                    .add_toast(adw::Toast::new(&format!("Import failed: {e:#}")));
                            }
                            Err(e) => {
                                dialog_toast
                                    .add_toast(adw::Toast::new(&format!("System error: {e}")));
                            }
                        }
                    });
                });

                row.add_suffix(&import_btn);
            }

            inventory_group.add(&row);
        }
    }

    page.add(&inventory_group);
    dialog.add(&page);
    dialog.present(Some(parent));
}

fn target_status_subtitle(base: &str, status: &TargetStatus) -> String {
    match status {
        TargetStatus::Identical => format!("{base} (Already configured, identical)"),
        TargetStatus::Conflict {
            existing,
            requested,
        } => format!(
            "{base} ⚠️ Conflict: {} vs {}",
            existing.summary(),
            requested.summary()
        ),
        TargetStatus::NotInstalled => format!("{base} (Not configured)"),
    }
}
