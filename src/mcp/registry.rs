use super::{
    canonical::McpServer,
    detection::{Installation, Platform},
    harnesses::{self, HarnessAdapter, Target},
    operations::{
        self, ConflictResolution, TargetActionTaken, TargetOpResult, TargetStatus,
        apply_target_write, check_target_status,
    },
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct NormalizedInventoryEntry {
    pub name: String,
    pub canonical: McpServer,
    pub installations: Vec<(Target, McpServer)>,
    pub has_drift: bool,
}

pub struct HarnessRegistry {
    adapters: Vec<Box<dyn HarnessAdapter>>,
}

impl Default for HarnessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessRegistry {
    pub fn new() -> Self {
        Self {
            adapters: harnesses::all(),
        }
    }

    pub fn adapters(&self) -> &[Box<dyn HarnessAdapter>] {
        &self.adapters
    }

    pub fn adapter(&self, id: &str) -> Option<&dyn HarnessAdapter> {
        self.adapters
            .iter()
            .find(|a| a.id() == id)
            .map(|a| a.as_ref())
    }

    /// Detects all installed harnesses on the platform.
    pub fn detect_installations(&self, platform: &Platform) -> Vec<Installation> {
        self.adapters
            .iter()
            .filter_map(|a| a.detect(platform))
            .collect()
    }

    /// Returns all targets for all registered adapters.
    pub fn all_targets(&self, platform: &Platform) -> Vec<Target> {
        self.adapters
            .iter()
            .flat_map(|a| a.targets(platform))
            .collect()
    }

    /// Checks the status of a server in a specific target.
    pub fn check_status(&self, server: &McpServer, target: &Target) -> Result<TargetStatus> {
        let adapter = self
            .adapter(&target.adapter_id)
            .context("Unknown harness adapter for target")?;
        check_target_status(adapter, target, server)
    }

    /// Installs a canonical server to a set of targets with conflict resolution.
    pub fn install_server(
        &self,
        server: &McpServer,
        targets: &[Target],
        resolution: ConflictResolution,
    ) -> Result<Vec<TargetOpResult>> {
        server.validate()?;
        let mut results = Vec::new();

        for target in targets {
            let adapter = self
                .adapter(&target.adapter_id)
                .with_context(|| format!("Unknown adapter {}", target.adapter_id))?;

            let status = check_target_status(adapter, target, server)?;
            let mut server_to_write = server.clone();

            match status {
                TargetStatus::Identical => {
                    results.push(TargetOpResult {
                        target: target.clone(),
                        action: TargetActionTaken::AlreadyUpToDate,
                    });
                    continue;
                }
                TargetStatus::Conflict { .. } => match &resolution {
                    ConflictResolution::Skip => {
                        results.push(TargetOpResult {
                            target: target.clone(),
                            action: TargetActionTaken::Skipped,
                        });
                        continue;
                    }
                    ConflictResolution::Rename(new_name) => {
                        server_to_write.name.clone_from(new_name);
                    }
                    ConflictResolution::Overwrite => {}
                },
                TargetStatus::NotInstalled => {}
            }

            apply_target_write(
                adapter,
                target,
                &server_to_write.name,
                Some(&server_to_write),
            )?;

            results.push(TargetOpResult {
                target: target.clone(),
                action: match status {
                    TargetStatus::Conflict { .. } => TargetActionTaken::Replaced,
                    _ => TargetActionTaken::Installed,
                },
            });
        }

        Ok(results)
    }

    /// Removes a server by name from specified targets.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn remove_server(
        &self,
        server_name: &str,
        targets: &[Target],
    ) -> Result<Vec<TargetOpResult>> {
        let mut results = Vec::new();

        for target in targets {
            let adapter = self
                .adapter(&target.adapter_id)
                .with_context(|| format!("Unknown adapter {}", target.adapter_id))?;

            if !target.path.exists() {
                results.push(TargetOpResult {
                    target: target.clone(),
                    action: TargetActionTaken::Skipped,
                });
                continue;
            }

            apply_target_write(adapter, target, server_name, None)?;

            results.push(TargetOpResult {
                target: target.clone(),
                action: TargetActionTaken::Removed,
            });
        }

        Ok(results)
    }

    /// Scans all targets and returns a deduplicated, normalized inventory
    /// showing where each MCP is installed and detecting configuration drift.
    pub fn scan_inventory(&self, platform: &Platform) -> Result<Vec<NormalizedInventoryEntry>> {
        let targets = self.all_targets(platform);
        let mut map: BTreeMap<String, Vec<(Target, McpServer)>> = BTreeMap::new();

        for target in targets {
            if !target.path.exists() {
                continue;
            }
            let Some(adapter) = self.adapter(&target.adapter_id) else {
                continue;
            };
            let Ok(config) = operations::read_target_value(&target) else {
                continue;
            };
            let Ok(entries) = adapter.entries(&target, &config) else {
                continue;
            };

            for (name, entry_val) in entries {
                if let Ok(server) = adapter.decode(&name, &entry_val) {
                    map.entry(name).or_default().push((target.clone(), server));
                }
            }
        }

        let mut inventory = Vec::new();
        for (name, installations) in map {
            let canonical = installations[0].1.clone();
            let has_drift = installations
                .windows(2)
                .any(|pair| !pair[0].1.equivalent(&pair[1].1));

            inventory.push(NormalizedInventoryEntry {
                name,
                canonical,
                installations,
                has_drift,
            });
        }

        Ok(inventory)
    }
}
