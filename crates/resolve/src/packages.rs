//! `package`: installed packages (`pacman -Q`) and repository packages (`pacman -Sl`).

use std::collections::BTreeMap;

use sancho_core::value::valid_package;
use sancho_core::{Query, System};

use crate::{ResolveError, Value};

pub fn enumerate(sys: &dyn System) -> Result<Vec<Value>, ResolveError> {
    let installed_out = sys
        .query(Query::InstalledPackages)
        .ok_or(ResolveError::Unavailable(Query::InstalledPackages))?;
    // name -> (version, repo)
    let mut pkgs: BTreeMap<String, (String, Option<String>, bool)> = BTreeMap::new();
    for line in installed_out.lines() {
        let mut cols = line.split_whitespace();
        if let (Some(name), Some(ver)) = (cols.next(), cols.next()) {
            pkgs.insert(name.to_string(), (ver.to_string(), None, true));
        }
    }
    // The sync list is optional: without it only installed packages resolve.
    for line in sys.query(Query::SyncPackages).unwrap_or_default().lines() {
        let mut cols = line.split_whitespace();
        if let (Some(repo), Some(name), Some(ver)) = (cols.next(), cols.next(), cols.next()) {
            pkgs.entry(name.to_string())
                .and_modify(|e| e.1 = Some(repo.to_string()))
                .or_insert((ver.to_string(), Some(repo.to_string()), false));
        }
    }
    Ok(pkgs
        .into_iter()
        .filter(|(name, _)| valid_package(name))
        .map(|(name, (ver, repo, installed))| {
            let where_ = match (installed, repo) {
                (true, Some(_)) => "installed".to_string(),
                (true, None) => "installed, not from a repository".to_string(),
                (false, Some(repo)) => repo,
                (false, None) => unreachable!("only sync entries lack the installed flag"),
            };
            Value::new(&name, format!("{name} {ver} ({where_})")).tag("installed", installed)
        })
        .collect())
}
