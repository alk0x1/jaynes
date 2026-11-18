//! G19's measure of a package manifest: whether its change holds a merge for the person, and what changes in it.
//!
//! It holds when it changes what runs or where code comes from: any of its scripts (`test` and `build` included,
//! which run on the person's machine and in their CI), its build configuration (`build-system` in pyproject.toml,
//! `build` and `links` in Cargo.toml), a dependency it did not have, or one of the same name from another source (a
//! git address, a local path, another registry). A version up or down from the same source, its metadata, or a lock
//! file alone does not. A dependency that comes in only through the lock file, without being in the manifest, holds
//! nothing: that is said in the list's documentation.

use serde_json::Value;
use std::collections::BTreeMap;

/// The measure's word on one manifest's change.
#[derive(Debug, PartialEq, Eq)]
pub struct Judged {
    pub holds: bool,
    pub about: String,
}

/// What of a manifest decides: its scripts, its build, and each dependency with where it comes from.
#[derive(Default, PartialEq, Eq, Debug)]
struct Facts {
    scripts: BTreeMap<String, String>,
    build: String,
    deps: BTreeMap<String, String>,
}

/// Judge the change of the manifest at `path` from its text `before` (none when it is new) to `after` (none when
/// it is gone). A manifest that cannot be read holds the merge.
pub fn judge(path: &str, before: Option<&str>, after: Option<&str>) -> Judged {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let read = |text: Option<&str>| -> Result<Facts, String> {
        let Some(text) = text else { return Ok(Facts::default()) };
        match name.as_str() {
            "package.json" => npm(text),
            "pyproject.toml" => python(text),
            "cargo.toml" => cargo(text),
            _ => Err(format!("{name} is not a manifest Jaynes reads")),
        }
    };
    let (b, a) = match (read(before), read(after)) {
        (Ok(b), Ok(a)) => (b, a),
        (_, Err(e)) | (Err(e), _) => return Judged { holds: true, about: format!("it could not be read as a manifest ({e}), so it is shown whole") },
    };
    let mut said = Vec::new();
    let scripts: Vec<&str> = a
        .scripts
        .keys()
        .chain(b.scripts.keys())
        .filter(|k| a.scripts.get(*k) != b.scripts.get(*k))
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !scripts.is_empty() {
        said.push(format!("its scripts change: {}", scripts.join(", ")));
    }
    if a.build != b.build {
        said.push("its build configuration changes".to_string());
    }
    for (dep, from) in &a.deps {
        match b.deps.get(dep) {
            None => said.push(format!("a new dependency, {dep}, from {from}")),
            Some(was) if was != from => said.push(format!("{dep} now comes from {from} (it came from {was})")),
            Some(_) => {}
        }
    }
    if said.is_empty() {
        return Judged { holds: false, about: "only versions from the same source or metadata change".to_string() };
    }
    Judged { holds: true, about: said.join("; ") }
}

/// Some settings as one text, empty when none is set: a manifest without them reads as one that is gone.
fn present(values: Vec<Value>) -> String {
    if values.iter().all(Value::is_null) { String::new() } else { Value::Array(values).to_string() }
}

/// Where an npm dependency's spec takes it: the registry for a version or a range, else the spec itself (a git or
/// web address, a local path, a workspace, an alias to another package).
fn npm_source(spec: &str) -> String {
    let s = spec.trim();
    let elsewhere = ["git", "github:", "gitlab:", "bitbucket:", "gist:", "file:", "link:", "workspace:", "npm:", "http:", "https:", "/", "./", "../", "~/"];
    if elsewhere.iter().any(|p| s.starts_with(p)) || s.contains("://") || (s.contains('/') && !s.starts_with('@')) {
        s.to_string()
    } else {
        "the npm registry".to_string()
    }
}

fn npm(text: &str) -> Result<Facts, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut f = Facts::default();
    if let Some(s) = v["scripts"].as_object() {
        f.scripts = s.iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
    }
    for table in ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies", "bundleDependencies", "bundledDependencies"] {
        if let Some(d) = v[table].as_object() {
            for (k, spec) in d {
                f.deps.insert(k.clone(), npm_source(spec.as_str().unwrap_or("")));
            }
        }
    }
    // what npm runs or follows at install, beside the scripts
    f.build = present(vec![v["bin"].clone(), v["gypfile"].clone(), v["overrides"].clone(), v["resolutions"].clone(), v["workspaces"].clone()]);
    Ok(f)
}

/// A PEP 508 requirement's name, lowercase with `-` for `_` and `.`, and where it comes from: its `@ URL`, or PyPI.
fn requirement(r: &str) -> (String, String) {
    let name: String = r.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let name = name.to_lowercase().replace(['_', '.'], "-");
    let from = match r.split_once(" @ ").or_else(|| r.split_once("@ ")) {
        Some((_, url)) => url.split(';').next().unwrap_or("").trim().to_string(),
        None => "PyPI".to_string(),
    };
    (name, from)
}

fn python(text: &str) -> Result<Facts, String> {
    let v: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut f = Facts::default();
    let at = |path: &[&str]| path.iter().try_fold(&v, |x, k| x.get(*k));
    for (path, prefix) in [(&["project", "scripts"][..], "script"), (&["project", "gui-scripts"][..], "gui-script"), (&["tool", "poetry", "scripts"][..], "poetry script")] {
        if let Some(t) = at(path).and_then(|x| x.as_table()) {
            for (k, x) in t {
                f.scripts.insert(format!("{prefix} {k}"), x.to_string());
            }
        }
    }
    if let Some(t) = at(&["project", "entry-points"]).and_then(|x| x.as_table()) {
        f.scripts.insert("entry-points".into(), toml::Value::Table(t.clone()).to_string());
    }
    f.build = at(&["build-system"]).map(|x| x.to_string()).unwrap_or_default();
    let mut add = |r: &str| {
        let (n, from) = requirement(r);
        if !n.is_empty() {
            f.deps.insert(n, from);
        }
    };
    for r in at(&["project", "dependencies"]).and_then(|x| x.as_array()).into_iter().flatten() {
        add(r.as_str().unwrap_or(""));
    }
    for group in at(&["project", "optional-dependencies"]).and_then(|x| x.as_table()).into_iter().flatten().map(|(_, g)| g) {
        for r in group.as_array().into_iter().flatten() {
            add(r.as_str().unwrap_or(""));
        }
    }
    for path in [&["tool", "poetry", "dependencies"][..], &["tool", "poetry", "dev-dependencies"][..]] {
        for (k, spec) in at(path).and_then(|x| x.as_table()).into_iter().flatten() {
            let from = ["git", "path", "url"].iter().find_map(|key| spec.get(*key).and_then(|x| x.as_str())).unwrap_or("PyPI").to_string();
            f.deps.insert(k.to_lowercase().replace(['_', '.'], "-"), from);
        }
    }
    Ok(f)
}

/// Where a Cargo dependency comes from: its git address, its path, its registry, or crates.io.
fn crate_source(spec: &toml::Value) -> String {
    for key in ["git", "path", "registry"] {
        if let Some(s) = spec.get(key).and_then(|x| x.as_str()) {
            return s.to_string();
        }
    }
    "crates.io".to_string()
}

fn cargo(text: &str) -> Result<Facts, String> {
    let v: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut f = Facts::default();
    let package = v.get("package");
    let text = |x: Option<&toml::Value>| x.map(|x| Value::String(x.to_string())).unwrap_or(Value::Null);
    f.build = present(vec![
        text(package.and_then(|p| p.get("build"))),
        text(package.and_then(|p| p.get("links"))),
        text(v.get("lib").and_then(|l| l.get("proc-macro"))),
    ]);
    let mut tables: Vec<&toml::Value> = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        tables.extend(v.get(key));
        tables.extend(v.get("workspace").and_then(|w| w.get(key)));
        for t in v.get("target").and_then(|t| t.as_table()).into_iter().flatten().map(|(_, t)| t) {
            tables.extend(t.get(key));
        }
    }
    for t in tables {
        for (k, spec) in t.as_table().into_iter().flatten() {
            // a dependency renamed takes its package's name
            let name = spec.get("package").and_then(|x| x.as_str()).unwrap_or(k).to_string();
            f.deps.insert(name, crate_source(spec));
        }
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_holds_a_merge_is_what_runs_or_where_code_comes_from() {
        let npm = |deps: &str, scripts: &str| format!("{{\"name\":\"p\",\"dependencies\":{{{deps}}},\"scripts\":{{{scripts}}}}}");
        let j = |b: &str, a: &str| judge("package.json", Some(b), Some(a));
        let base = npm("\"left-pad\":\"1.3.0\"", "\"test\":\"node t.js\"");
        assert!(!j(&base, &npm("\"left-pad\":\"^1.4.0\"", "\"test\":\"node t.js\"")).holds);
        assert!(j(&base, &npm("\"left-pad\":\"1.3.0\"", "\"test\":\"node u.js\"")).holds);
        let new = j(&base, &npm("\"left-pad\":\"1.3.0\",\"@scope/x\":\"2\"", "\"test\":\"node t.js\""));
        assert!(new.holds && new.about.contains("@scope/x") && new.about.contains("the npm registry"), "{new:?}");
        let moved = j(&base, &npm("\"left-pad\":\"user/left-pad#main\"", "\"test\":\"node t.js\""));
        assert!(moved.holds && moved.about.contains("user/left-pad#main"), "{moved:?}");
        // a new manifest with scripts holds; one that cannot be read holds; one gone with no scripts does not
        assert!(judge("package.json", None, Some(&base)).holds);
        assert!(judge("package.json", Some(&base), Some("{not json")).holds);
        assert!(!judge("package.json", Some("{\"name\":\"p\"}"), None).holds);
        // Python: a requirement's version does not, its source does
        let py = |deps: &str| format!("[project]\nname = \"p\"\ndependencies = [{deps}]\n");
        assert!(!judge("pyproject.toml", Some(&py("\"Requests>=2\"")), Some(&py("\"requests>=2.31\""))).holds);
        assert!(judge("pyproject.toml", Some(&py("\"requests>=2\"")), Some(&py("\"requests @ git+https://example.org/r\""))).holds);
        // Cargo: a renamed dependency is its package
        let c = |deps: &str| format!("[package]\nname = \"p\"\nversion = \"1.0.0\"\n\n[dependencies]\n{deps}\n");
        assert!(!judge("Cargo.toml", Some(&c("serde = \"1\"")), Some(&c("serde = { version = \"1.0.200\", features = [\"derive\"] }"))).holds);
        assert!(judge("Cargo.toml", Some(&c("serde = \"1\"")), Some(&c("serde = \"1\"\nmy = { package = \"libc\", version = \"0.2\" }"))).holds);
    }
}
