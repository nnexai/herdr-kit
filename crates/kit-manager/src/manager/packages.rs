use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binary {
    pub name: String,
    pub cargo_package: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Package {
    #[serde(default)]
    pub binary: Option<Binary>,
    #[serde(default)]
    pub mods: Vec<String>,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub runtime_package: Option<String>,
    #[serde(default)]
    pub runtime_mod: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Catalog {
    pub schema_version: u32,
    pub packages: BTreeMap<String, Package>,
    pub bundles: BTreeMap<String, Vec<String>>,
}

pub(super) fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
        "invalid package/artifact name"
    );
    Ok(())
}

impl Catalog {
    pub fn load(source: &Path) -> Result<Self> {
        let path = source.join("kit-packages.json");
        let catalog: Self = serde_json::from_slice(&fs::read(path)
            .context("committed kit-packages.json is required")?)?;
        ensure!(
            catalog.schema_version == 1,
            "unsupported package catalog schema"
        );
        let mut mods = std::collections::BTreeSet::new();
        for (id, package) in &catalog.packages {
            identifier(id)?;
            if let Some(binary) = &package.binary {
                identifier(&binary.name)?;
                identifier(&binary.cargo_package)?;
            }
            ensure!(
                package.mods.is_empty()
                    || package.binary.is_some()
                    || package.runtime_package.is_some()
                    || package.layout.is_some(),
                "capability mod package requires an executable binding"
            );
            for base in &package.mods {
                identifier(base)?;
                ensure!(mods.insert(base), "mod has multiple package owners");
            }
            for required in &package.requires {
                ensure!(
                    catalog.packages.contains_key(required),
                    "unknown required component: {required}"
                );
            }
            if let Some(required) = &package.runtime_package {
                let dependency = catalog
                    .packages
                    .get(required)
                    .context("unknown runtime package")?;
                ensure!(
                    package.binary.is_none()
                        && package.requires.contains(required)
                        && dependency.binary.is_some(),
                    "invalid runtime dependency"
                );
                if let Some(base) = &package.runtime_mod {
                    ensure!(
                        dependency.mods.contains(base),
                        "runtime mod is not owned by its dependency"
                    );
                }
            } else {
                ensure!(
                    package.runtime_mod.is_none(),
                    "runtime mod requires its dependency package"
                );
            }
            if let Some(layout) = &package.layout {
                ensure!(
                    id == "herdr-kit" && layout == "layouts/herdr-kit.json",
                    "unsupported owned layout"
                );
            }
        }
        for packages in catalog.bundles.values() {
            ensure!(
                packages.iter().all(|id| catalog.packages.contains_key(id)),
                "bundle references unknown package"
            );
        }
        Ok(catalog)
    }
    pub fn selected(&self, requested: &[String]) -> Result<Vec<String>> {
        let mut selected = if requested.is_empty() {
            self.bundles
                .get("herdr-kit")
                .context("missing default bundle")?
                .clone()
        } else {
            requested.to_vec()
        };
        selected.sort();
        selected.dedup();
        ensure!(!selected.is_empty(), "empty component selection");
        let mut result = Vec::with_capacity(selected.len());
        let mut visited = std::collections::BTreeSet::new();
        let mut visiting = std::collections::BTreeSet::new();
        for id in &selected {
            self.visit(id, &mut visited, &mut visiting, &mut result)?;
        }
        Ok(result)
    }
    fn visit<'a>(
        &'a self,
        id: &'a str,
        visited: &mut std::collections::BTreeSet<&'a str>,
        visiting: &mut std::collections::BTreeSet<&'a str>,
        result: &mut Vec<String>,
    ) -> Result<()> {
        if visited.contains(id) {
            return Ok(());
        }
        let package = self
            .packages
            .get(id)
            .with_context(|| format!("unknown component: {id}"))?;
        ensure!(visiting.insert(id), "component dependency cycle");
        for required in &package.requires {
            self.visit(required, visited, visiting, result)?;
        }
        visiting.remove(id);
        visited.insert(id);
        result.push(id.to_owned());
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PackageReceipt {
    pub definition: Package,
    pub binary_hash: Option<String>,
    pub mods: BTreeMap<String, BTreeMap<String, String>>,
    pub layout_hash: Option<String>,
    pub activated: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct FileReceipt {
    pub source: PathBuf,
    pub hash: String,
    pub executable: bool,
    // Readiness descriptors are applied after every binary/mod/layout target.
    pub ready: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct FileChange {
    pub target: PathBuf,
    pub old_hash: Option<String>,
    pub next: Option<FileReceipt>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ModChange {
    pub target: PathBuf,
    pub source: PathBuf,
    pub hashes: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Transaction {
    pub schema_version: u32,
    pub state: State,
    pub files: Vec<FileChange>,
    pub mods: Vec<ModChange>,
    pub obsolete_deployments: BTreeMap<PathBuf, String>,
}

pub(super) fn project_layout(
    value: &mut Value,
    known: &std::collections::BTreeSet<String>,
    bindings: &BTreeMap<String, String>,
    manager: Option<&Path>,
    data: &Path,
) -> bool {
    if let Some(base) = value.pointer("/content/mod").and_then(Value::as_str) {
        if known.contains(base) {
            let Some(name) = bindings.get(base) else {
                return false;
            };
            if base == "herdr-kit-bar" {
                value["content"]["props"] = json!({
                    "coord_mod": bindings.get("kit-coord"),
                    "usage_mod": bindings.get("kit-usage"),
                    "manager": manager,
                    "data_dir": data,
                });
            }
            value["content"]["mod"] = json!(name);
        }
    }
    if let Some(children) = value.get_mut("children").and_then(Value::as_array_mut) {
        children.retain_mut(|child| project_layout(child, known, bindings, manager, data));
    }
    true
}
