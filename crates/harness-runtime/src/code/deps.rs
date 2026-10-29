//! Declared dependencies from build manifests. Pure parsing: each function takes the
//! text of one manifest and returns the dependencies it declares. Names are compared after
//! normalising (case-insensitive, `-` and `_` equal), as Cargo and PyPI do.

use serde::Serialize;

/// One dependency declared by a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dependency {
    pub name: String,
    /// The version requirement as written, when the manifest states one.
    pub version: Option<String>,
    /// Where it is declared: `dependencies`, `dev-dependencies`, `build-dependencies`,
    /// `workspace.dependencies`, `optional-dependencies`, or `requirements` for requirement files.
    pub section: String,
}

/// Manifest file names this module understands, matched on the last path component.
pub fn is_manifest(file_name: &str) -> bool {
    matches!(file_name, "Cargo.toml" | "pyproject.toml" | "package.json")
        || (file_name.starts_with("requirements") && file_name.ends_with(".txt"))
}

/// Normalised package name: lowercase, with `-` and `.` read as `_`.
pub fn normalise(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect()
}

/// The package name at the start of a PEP 508 requirement (`requests[socks]>=2 ; python_version`).
fn pep508_name(requirement: &str) -> Option<(String, Option<String>)> {
    let trimmed = requirement.trim();
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .unwrap_or(trimmed.len());
    let name = &trimmed[..end];
    if name.is_empty() {
        return None;
    }
    let rest = trimmed[end..].split(';').next().unwrap_or_default().trim();
    let version = (!rest.is_empty()).then(|| rest.trim_start_matches('[').to_owned());
    Some((name.to_owned(), version))
}

fn toml_section(table: &toml::Table, path: &[&str]) -> Option<toml::Table> {
    let mut current = table;
    for key in path {
        current = current.get(*key)?.as_table()?;
    }
    Some(current.clone())
}

fn cargo_entries(table: &toml::Table, section: &str, out: &mut Vec<Dependency>) {
    for (name, spec) in table {
        let version = match spec {
            toml::Value::String(version) => Some(version.clone()),
            toml::Value::Table(fields) => fields
                .get("version")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            _ => None,
        };
        out.push(Dependency {
            name: name.clone(),
            version,
            section: section.to_owned(),
        });
    }
}

/// Dependencies in a `Cargo.toml`: its own sections and the workspace table.
pub fn parse_cargo(text: &str) -> Vec<Dependency> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get(section).and_then(toml::Value::as_table) {
            cargo_entries(table, section, &mut out);
        }
    }
    if let Some(table) = toml_section(&doc, &["workspace", "dependencies"]) {
        cargo_entries(&table, "workspace.dependencies", &mut out);
    }
    out
}

/// Dependencies in a `pyproject.toml`: PEP 621 `[project]` and Poetry tables.
pub fn parse_pyproject(text: &str) -> Vec<Dependency> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(list) = toml_section(&doc, &["project"])
        .and_then(|project| project.get("dependencies").cloned())
        .and_then(|value| value.as_array().cloned())
    {
        for item in list.iter().filter_map(toml::Value::as_str) {
            if let Some((name, version)) = pep508_name(item) {
                out.push(Dependency {
                    name,
                    version,
                    section: "dependencies".to_owned(),
                });
            }
        }
    }
    if let Some(table) = toml_section(&doc, &["tool", "poetry", "dependencies"]) {
        for (name, spec) in &table {
            if name == "python" {
                continue;
            }
            let version = spec.as_str().map(str::to_owned);
            out.push(Dependency {
                name: name.clone(),
                version,
                section: "tool.poetry.dependencies".to_owned(),
            });
        }
    }
    out
}

/// Dependencies in a `requirements*.txt` file. Comments, options and URLs are skipped.
pub fn parse_requirements(text: &str) -> Vec<Dependency> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty() && !line.starts_with('-') && !line.contains("://"))
        .filter_map(pep508_name)
        .map(|(name, version)| Dependency {
            name,
            version,
            section: "requirements".to_owned(),
        })
        .collect()
}

/// Dependencies in a `package.json`.
pub fn parse_package_json(text: &str) -> Vec<Dependency> {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if let Some(map) = doc.get(section).and_then(serde_json::Value::as_object) {
            for (name, version) in map {
                out.push(Dependency {
                    name: name.clone(),
                    version: version.as_str().map(str::to_owned),
                    section: section.to_owned(),
                });
            }
        }
    }
    out
}

/// Dispatches on the manifest's file name.
pub fn parse_manifest(file_name: &str, text: &str) -> Vec<Dependency> {
    match file_name {
        "Cargo.toml" => parse_cargo(text),
        "pyproject.toml" => parse_pyproject(text),
        "package.json" => parse_package_json(text),
        _ => parse_requirements(text),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn cargo_dependencies_in_every_section() {
        let text = "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1.0\"\ntokio = { version = \"1\", features = [\"rt\"] }\n\n[dev-dependencies]\ntempfile = \"3\"\n\n[workspace.dependencies]\ntoml = \"1.1.8\"\n";
        let deps = parse_cargo(text);
        let found: Vec<(&str, &str)> = deps
            .iter()
            .map(|d| (d.name.as_str(), d.section.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("serde", "dependencies"),
                ("tokio", "dependencies"),
                ("tempfile", "dev-dependencies"),
                ("toml", "workspace.dependencies"),
            ]
        );
        assert_eq!(deps[1].version.as_deref(), Some("1"));
    }

    #[test]
    fn pyproject_and_requirements_use_pep508_names() {
        let text = "[project]\nname = \"p\"\ndependencies = [\"requests[socks]>=2.31 ; python_version > '3'\", \"Flask-Cors\"]\n";
        let deps = parse_pyproject(text);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[1].name, "Flask-Cors");
        let req = "# pins\nnumpy==1.26.0\n-r other.txt\ngit+https://example.com/x.git\npandas>=2 # note\n";
        let names: Vec<String> = parse_requirements(req)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["numpy", "pandas"]);
    }

    #[test]
    fn package_json_sections_and_name_normalisation() {
        let text =
            r#"{"dependencies":{"left-pad":"^1.3.0"},"devDependencies":{"typescript":"5.6.3"}}"#;
        let deps = parse_package_json(text);
        assert_eq!(deps.len(), 2);
        assert_eq!(normalise("Flask-Cors"), normalise("flask_cors"));
        assert!(is_manifest("requirements-dev.txt"));
        assert!(!is_manifest("README.md"));
    }
}
