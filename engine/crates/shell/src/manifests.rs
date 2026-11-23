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

/// The files that say only how a project's tests run, each with what runs them: a change to one changes how they
/// run, whatever it is. Code that runs with the tests (`conftest.py`, a test setup file) is said as such.
const TESTS_SETUP: &[(&[&str], &str)] = &[
    (&["**/pytest.ini"], "pytest"),
    (&["**/tox.ini"], "tox"),
    (&["**/noxfile.py"], "nox"),
    (&["**/jest.config.*"], "jest"),
    (&["**/vitest.config.*", "**/vitest.workspace.*"], "vitest"),
    (&["**/karma.conf.*"], "karma"),
    (&["**/.mocharc*"], "mocha"),
    (&["**/ava.config.*"], "ava"),
    (&["**/playwright.config.*"], "playwright"),
    (&["**/cypress.config.*"], "cypress"),
    (&["**/phpunit.xml", "**/phpunit.xml.dist"], "phpunit"),
    (&["**/.rspec"], "rspec"),
    (&["**/.nycrc*"], "coverage"),
];

/// Code that runs on its own whenever the tests run.
const RUNS_WITH_TESTS: &[&str] = &["**/conftest.py", "**/jest.setup.*", "**/vitest.setup.*"];

/// The keys of a package.json that say how its tests run, beside its test scripts.
const NPM_TESTS: &[&str] = &["jest", "mocha", "ava", "vitest", "c8", "nyc"];

/// Whether `path` may change how the project's tests run, by its name: a file that says only that, or one that may
/// say it among other things (pyproject.toml, setup.cfg, package.json), which [`tests_run`] reads.
pub fn may_change_tests(path: &str) -> bool {
    let p = path.to_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    matches!(name, "pyproject.toml" | "setup.cfg" | "package.json")
        || RUNS_WITH_TESTS.iter().chain(TESTS_SETUP.iter().flat_map(|(g, _)| g.iter())).any(|g| jaynes_core::glob::matches(&p, g))
}

/// What of the file at `path` changes how the project's tests run, from its text `before` (none when it is new) to
/// `after` (none when it is gone), in plain words: "its pytest settings change". A file that says only how the tests
/// run changes it whatever changed in it; in pyproject.toml, setup.cfg and package.json, only the part that says it
/// does. None when nothing of it does; one that cannot be read says so, to be read whole.
pub fn tests_run(path: &str, before: Option<&str>, after: Option<&str>) -> Option<String> {
    let p = path.to_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p).to_string();
    if before == after {
        return None;
    }
    if RUNS_WITH_TESTS.iter().any(|g| jaynes_core::glob::matches(&p, g)) {
        return Some("it runs with your tests".to_string());
    }
    if let Some((_, tool)) = TESTS_SETUP.iter().find(|(globs, _)| globs.iter().any(|g| jaynes_core::glob::matches(&p, g))) {
        return Some(format!("its {tool} settings change"));
    }
    let unread = || Some("it could not be read, so read it whole".to_string());
    let differs = |part: &dyn Fn(&str) -> Result<String, String>| -> Result<bool, String> {
        Ok(before.map(part).transpose()?.unwrap_or_default() != after.map(part).transpose()?.unwrap_or_default())
    };
    match name.as_str() {
        "pyproject.toml" => {
            let pytest = |t: &str| -> Result<String, String> {
                let v: toml::Value = toml::from_str(t).map_err(|e| e.to_string())?;
                Ok(v.get("tool").and_then(|x| x.get("pytest")).map(|x| x.to_string()).unwrap_or_default())
            };
            match differs(&pytest) {
                Ok(true) => Some("its pytest settings change".to_string()),
                Ok(false) => None,
                Err(_) => unread(),
            }
        }
        "setup.cfg" => {
            // pytest reads its settings there from [tool:pytest]
            let pytest = |t: &str| -> Result<String, String> {
                let mut inside = false;
                let mut out = String::new();
                for line in t.lines() {
                    let l = line.trim();
                    if l.starts_with('[') && l.ends_with(']') {
                        inside = l[1..l.len() - 1].trim().eq_ignore_ascii_case("tool:pytest");
                    } else if inside && !l.is_empty() && !l.starts_with(['#', ';']) {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
                Ok(out)
            };
            differs(&pytest).ok().and_then(|d| d.then(|| "its pytest settings change".to_string()))
        }
        "package.json" => {
            let read = |t: &str| -> Result<Value, String> { serde_json::from_str(t).map_err(|e| e.to_string()) };
            let (b, a) = match (before.map(read).transpose(), after.map(read).transpose()) {
                (Ok(b), Ok(a)) => (b.unwrap_or(Value::Null), a.unwrap_or(Value::Null)),
                _ => return unread(),
            };
            let test_script = |k: &str| k == "test" || k == "pretest" || k == "posttest" || k.starts_with("test:") || k.starts_with("test-");
            let scripts = |v: &Value| -> BTreeMap<String, String> {
                v["scripts"].as_object().map(|s| s.iter().filter(|(k, _)| test_script(k)).map(|(k, v)| (k.clone(), v.to_string())).collect()).unwrap_or_default()
            };
            let mut said = Vec::new();
            let (sb, sa) = (scripts(&b), scripts(&a));
            let changed: Vec<&str> = sa.keys().chain(sb.keys()).filter(|k| sa.get(*k) != sb.get(*k)).map(String::as_str).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            match changed.as_slice() {
                [] => {}
                [one] => said.push(format!("its {one} script changes")),
                many => said.push(format!("its scripts {} change", many.join(", "))),
            }
            for k in NPM_TESTS {
                if a[*k] != b[*k] {
                    said.push(format!("its {k} settings change"));
                }
            }
            (!said.is_empty()).then(|| said.join("; "))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    // The real-user's contas, 23/11: the fix changed python_files under [tool.pytest.ini_options], which changes
    // which files are tests, and the card before the merge never said so.
    #[test]
    fn a_file_that_changes_how_the_tests_run_is_told_by_what_of_it_does() {
        use super::{may_change_tests, tests_run};
        let was = "[project]\nname = \"contas\"\n";
        let now = "[project]\nname = \"contas\"\n\n[tool.pytest.ini_options]\npython_files = [\"test_*.py\", \"*_test.py\"]\n";
        assert_eq!(tests_run("pyproject.toml", Some(was), Some(now)).as_deref(), Some("its pytest settings change"));
        assert_eq!(tests_run("pyproject.toml", None, Some(now)).as_deref(), Some("its pytest settings change"), "a new one");
        assert_eq!(tests_run("pyproject.toml", Some(was), Some("[project]\nname = \"contas\"\nversion = \"2\"\n")), None, "its metadata only");
        assert_eq!(tests_run("pyproject.toml", Some(was), Some("[project")).as_deref(), Some("it could not be read, so read it whole"));
        assert_eq!(tests_run("setup.cfg", Some("[metadata]\nname = a\n"), Some("[metadata]\nname = a\n[tool:pytest]\naddopts = -k fast\n")).as_deref(), Some("its pytest settings change"));
        assert_eq!(tests_run("setup.cfg", Some("[metadata]\nname = a\n"), Some("[metadata]\nname = b\n")), None);
        let pkg = |s: &str| format!("{{\"name\":\"a\",\"scripts\":{{{s}}}}}");
        assert_eq!(tests_run("web/package.json", Some(&pkg("\"test\":\"jest\"")), Some(&pkg("\"test\":\"jest --passWithNoTests\""))).as_deref(), Some("its test script changes"));
        assert_eq!(tests_run("package.json", Some(&pkg("\"build\":\"tsc\"")), Some(&pkg("\"build\":\"tsc -b\""))), None, "a build script is not how the tests run");
        assert_eq!(tests_run("package.json", Some("{}"), Some("{\"jest\":{\"testMatch\":[\"x\"]}}")).as_deref(), Some("its jest settings change"));
        assert_eq!(tests_run("pytest.ini", Some("[pytest]\n"), Some("[pytest]\naddopts = -x\n")).as_deref(), Some("its pytest settings change"));
        assert_eq!(tests_run("app/jest.config.js", None, Some("module.exports = {}\n")).as_deref(), Some("its jest settings change"));
        assert_eq!(tests_run("tests/conftest.py", Some("a\n"), Some("b\n")).as_deref(), Some("it runs with your tests"));
        assert_eq!(tests_run("pytest.ini", Some("x"), Some("x")), None, "unchanged");
        assert!(may_change_tests("pyproject.toml") && may_change_tests("a/Setup.cfg") && may_change_tests("tox.ini") && may_change_tests(".mocharc.yml"));
        assert!(!may_change_tests("contas/relatorio.py") && !may_change_tests("Cargo.toml"));
    }

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
