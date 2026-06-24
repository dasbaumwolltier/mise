use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use eyre::WrapErr;
use serde::Deserialize;

use super::{prefix, resolve, state};
use crate::result::Result;
use crate::system::packages::{CleanupOpts, CleanupResult, PackageRequest};

#[derive(Debug, Default)]
struct InstalledFormulae {
    versions: BTreeMap<String, String>,
    dependencies: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Deserialize)]
struct Receipt {
    #[serde(default)]
    runtime_dependencies: Vec<ReceiptDependency>,
}

#[derive(Debug, Deserialize)]
struct ReceiptDependency {
    full_name: String,
}

pub async fn cleanup(requested: &[PackageRequest], opts: &CleanupOpts) -> Result<CleanupResult> {
    let ledger = state::Ledger::load();
    if ledger.kegs.is_empty() {
        return Ok(CleanupResult::default());
    }
    let keep = requested_closure(requested).await?;
    let installed = installed_formulae()?;
    let removable = removable_formulae(&ledger, &keep, &installed);
    let mut result = CleanupResult::default();
    for name in removable {
        let Some(version) = installed.versions.get(&name) else {
            result.skipped.push(format!("brew:{name} (not linked)"));
            continue;
        };
        if opts.dry_run && opts.show_output {
            miseprintln!("remove brew:{name}@{version}");
        } else {
            super::pour::remove_formula(&name, version)?;
            let mut ledger = state::Ledger::load();
            ledger.kegs.remove(&name);
            ledger.save()?;
        }
        result.removed.push(format!("brew:{name}@{version}"));
    }
    Ok(result)
}

async fn requested_closure(requested: &[PackageRequest]) -> Result<BTreeSet<String>> {
    if requested.is_empty() {
        return Ok(BTreeSet::new());
    }
    let closure = resolve::resolve_closure_with_taps(requested).await?;
    Ok(closure
        .into_iter()
        .map(|formula| formula.formula.name)
        .collect())
}

fn removable_formulae(
    ledger: &state::Ledger,
    keep: &BTreeSet<String>,
    installed: &InstalledFormulae,
) -> Vec<String> {
    let candidates: BTreeSet<String> = ledger
        .kegs
        .keys()
        .filter(|name| !keep.contains(*name))
        .filter(|name| installed.versions.contains_key(*name))
        .cloned()
        .collect();
    let mut protected = keep.clone();
    for (dependent, deps) in &installed.dependencies {
        if !candidates.contains(dependent) {
            protected.extend(deps.iter().cloned());
        }
    }
    candidates
        .into_iter()
        .filter(|name| !protected.contains(name))
        .collect()
}

fn installed_formulae() -> Result<InstalledFormulae> {
    let mut installed = InstalledFormulae::default();
    for rack in crate::file::ls(&prefix::cellar()).unwrap_or_default() {
        if !rack.is_dir() {
            continue;
        }
        let Some(name) = rack.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let Some(version) = super::pour::linked_version(&name) else {
            continue;
        };
        let keg = rack.join(&version);
        installed.versions.insert(name.clone(), version);
        installed
            .dependencies
            .insert(name, receipt_dependencies(&keg)?);
    }
    Ok(installed)
}

fn receipt_dependencies(keg: &Path) -> Result<BTreeSet<String>> {
    let path = keg.join("INSTALL_RECEIPT.json");
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let body = crate::file::read_to_string(&path)?;
    let receipt: Receipt = serde_json::from_str(&body)
        .wrap_err_with(|| format!("failed to parse {}", path.display()))?;
    Ok(receipt
        .runtime_dependencies
        .into_iter()
        .map(|dep| dep.full_name)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(names: &[&str]) -> state::Ledger {
        let mut ledger = state::Ledger::default();
        for name in names {
            ledger.record(name, "1.0.0", false);
        }
        ledger
    }

    #[test]
    fn removes_unprotected_candidates() {
        let ledger = ledger(&["leaf", "dep", "kept"]);
        let keep = BTreeSet::from(["kept".to_string()]);
        let installed = InstalledFormulae {
            versions: BTreeMap::from([
                ("leaf".to_string(), "1.0.0".to_string()),
                ("dep".to_string(), "1.0.0".to_string()),
                ("kept".to_string(), "1.0.0".to_string()),
            ]),
            dependencies: BTreeMap::from([(
                "leaf".to_string(),
                BTreeSet::from(["dep".to_string()]),
            )]),
        };

        assert_eq!(
            removable_formulae(&ledger, &keep, &installed),
            vec!["dep", "leaf"]
        );
    }

    #[test]
    fn preserves_dependencies_of_non_candidates() {
        let ledger = ledger(&["dep"]);
        let keep = BTreeSet::new();
        let installed = InstalledFormulae {
            versions: BTreeMap::from([
                ("dep".to_string(), "1.0.0".to_string()),
                ("external".to_string(), "1.0.0".to_string()),
            ]),
            dependencies: BTreeMap::from([(
                "external".to_string(),
                BTreeSet::from(["dep".to_string()]),
            )]),
        };

        assert!(removable_formulae(&ledger, &keep, &installed).is_empty());
    }
}
