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
        let catalog: Self = if path.exists() {
            serde_json::from_slice(&fs::read(path)?)?
        } else {
            // Committed pre-workspace source is a supported rollback input, not
            // a runtime alias. Its direct engines still belong to herdr-kit.
            let mut old: Self =
                serde_json::from_str(include_str!("../../../../kit-packages.json"))?;
            for package in old.packages.values_mut() {
                if let Some(binary) = &mut package.binary {
                    binary.cargo_package = "herdr-kit".into();
                }
                package
                    .mods
                    .retain(|base| source.join("mods").join(base).is_dir());
            }
            old
        };
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
                package.mods.is_empty() || package.binary.is_some(),
                "mod package requires its own executable binding"
            );
            for base in &package.mods {
                identifier(base)?;
                ensure!(mods.insert(base), "mod has multiple package owners");
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
        for id in &selected {
            ensure!(self.packages.contains_key(id), "unknown component: {id}");
        }
        Ok(selected)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PackageReceipt {
    pub definition: Package,
    pub binary_hash: Option<String>,
    pub mods: BTreeMap<String, BTreeMap<String, String>>,
    pub layout_hash: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct FileReceipt {
    pub source: PathBuf,
    pub hash: String,
    pub executable: bool,
    // Readiness descriptors are applied after every binary/mod/layout target.
    #[serde(default)]
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
}

pub(super) fn project_layout(
    value: &mut Value,
    known: &std::collections::BTreeSet<String>,
    bindings: &BTreeMap<String, String>,
) -> bool {
    if let Some(base) = value.pointer("/content/mod").and_then(Value::as_str) {
        if known.contains(base) {
            let Some(name) = bindings.get(base) else {
                return false;
            };
            value["content"]["mod"] = json!(name);
        }
    }
    if let Some(children) = value.get_mut("children").and_then(Value::as_array_mut) {
        children.retain_mut(|child| project_layout(child, known, bindings));
    }
    true
}
