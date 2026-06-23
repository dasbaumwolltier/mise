use eyre::{Result, bail};

use crate::config::{Config, Settings};
use crate::system::packages::CleanupOpts;
use crate::system::{self, ManagerPackages};
use crate::ui::prompt;

/// Uninstall unneeded system packages
///
/// Removes packages that are no longer requested in `[bootstrap.packages]` and
/// are not dependencies of the remaining requested packages. Managers only run
/// cleanup when mise has reliable ownership and dependency data for that
/// package manager.
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment, after_long_help = AFTER_LONG_HELP)]
pub struct SystemCleanup {
    /// Only cleanup packages for this manager, e.g. `brew`
    #[clap(long, short, value_parser = ["apk", "apt", "brew", "brew-cask", "dnf", "mas", "pacman"])]
    manager: Option<String>,

    /// Print the packages that would be removed without removing them
    #[clap(long, short = 'n')]
    dry_run: bool,

    /// Skip the confirmation prompt
    #[clap(long, short)]
    yes: bool,
}

impl SystemCleanup {
    pub async fn run(self) -> Result<()> {
        Settings::get().ensure_experimental("mise bootstrap")?;
        let config = Config::get().await?;
        let mgrs = cleanup_managers(&config, self.manager.as_deref())?;
        if mgrs.is_empty() {
            info!("no package managers support cleanup");
            return Ok(());
        }
        for mp in mgrs {
            if let Some(only) = &self.manager
                && mp.manager.name() != only
            {
                continue;
            }
            let name = mp.manager.name();
            if mp.disabled {
                if self.manager.is_some() {
                    bail!("manager '{name}' is excluded by the system_packages.managers setting");
                }
                debug!("{name}: skipping, excluded by system_packages.managers");
                continue;
            }
            if !mp.manager.is_available() {
                if self.manager.is_some() {
                    bail!(
                        "{name} is not available: {}",
                        mp.manager.unavailable_reason()
                    );
                }
                debug!("{name}: skipping, {}", mp.manager.unavailable_reason());
                continue;
            }
            if !mp.manager.supports_cleanup() {
                warn!("{name}: package cleanup is not supported");
                continue;
            }
            let preview = mp
                .manager
                .cleanup(
                    &mp.requests,
                    &CleanupOpts {
                        dry_run: true,
                        show_output: false,
                    },
                )
                .await?;
            if preview.removed.is_empty() {
                info!("{name}: no packages to cleanup");
                for skipped in preview.skipped {
                    warn!("{name}: skipped {skipped}");
                }
                continue;
            }
            if self.dry_run {
                for removed in preview.removed {
                    miseprintln!("remove {removed}");
                }
                for skipped in preview.skipped {
                    warn!("{name}: skipped {skipped}");
                }
                continue;
            }
            if !self.dry_run && !self.yes && console::user_attended_stderr() {
                let msg = format!("{name}: cleanup {}?", preview.removed.join(", "));
                if !prompt::confirm(msg)? {
                    info!("{name}: skipped");
                    continue;
                }
            }
            let result = mp
                .manager
                .cleanup(
                    &mp.requests,
                    &CleanupOpts {
                        dry_run: false,
                        show_output: false,
                    },
                )
                .await?;
            info!("{name}: removed {}", result.removed.join(", "));
            for skipped in result.skipped {
                warn!("{name}: skipped {skipped}");
            }
        }
        Ok(())
    }
}

fn cleanup_managers(config: &Config, only: Option<&str>) -> Result<Vec<ManagerPackages>> {
    let enabled = Settings::get().system_packages.managers.clone();
    let mut mgrs = system::packages_from_config(config);
    if let Some(only) = only {
        if let Some(enabled) = &enabled
            && !enabled.contains(&only.to_string())
        {
            bail!(
                "manager '{only}' is excluded by the system_packages.managers setting \
                 (currently: {})",
                enabled.join(", ")
            );
        }
        if mgrs.iter().all(|mp| mp.manager.name() != only) {
            let manager = system::packages::get_manager(only)
                .ok_or_else(|| eyre::eyre!("unknown bootstrap package manager '{only}'"))?;
            mgrs.push(ManagerPackages {
                manager,
                requests: vec![],
                disabled: false,
            });
        }
        return Ok(mgrs
            .into_iter()
            .filter(|mp| mp.manager.name() == only)
            .collect());
    }

    for manager in system::packages::all_managers()
        .into_iter()
        .filter(|manager| manager.supports_cleanup())
    {
        let name = manager.name();
        if mgrs.iter().any(|mp| mp.manager.name() == name) {
            continue;
        }
        let disabled = enabled
            .as_ref()
            .is_some_and(|e| !e.contains(&name.to_string()));
        mgrs.push(ManagerPackages {
            manager,
            requests: vec![],
            disabled,
        });
    }
    Ok(mgrs)
}

static AFTER_LONG_HELP: &str = color_print::cstr!(
    r#"<bold><underline>Examples:</underline></bold>

    $ <bold>mise bootstrap packages cleanup</bold>
    $ <bold>mise bootstrap packages cleanup --manager brew</bold>
    $ <bold>mise bootstrap packages cleanup --dry-run</bold>
"#
);
