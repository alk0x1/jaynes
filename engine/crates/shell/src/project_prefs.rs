//! What the person chose for one project, kept in the engine's home and never in the project: no file of the
//! repository, of the run's copy or of an agent's can say it. One TOML file per project, named after its path,
//! a section per matter; a section this engine does not know is kept as it is when another is written.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The folder of every project's file, under the engine's home. The boxes hide it.
pub const FOLDER: &str = "projects";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ports {
    /// The project's checks reach any local port of this machine, not only the ones a run reserves or a step
    /// declares. The person's, never an agent's.
    #[serde(default)]
    pub any: bool,
    /// The project's tests may start their own test servers on ports the system picks (`jaynes project
    /// test-servers`, macOS's box). The person's, never an agent's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub servers: bool,
    /// When the person opened them, in seconds since 1970.
    #[serde(default)]
    pub opened: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub ports: Option<Ports>,
    /// Every other section, kept as it was read.
    pub rest: toml::Table,
}

/// The file of `project`, whose path is made canonical first.
pub fn path(home: &Path, project: &Path) -> PathBuf {
    let real = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let digest = Sha256::digest(real.display().to_string().as_bytes());
    let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    home.join(FOLDER).join(format!("{name}.toml"))
}

/// What the person chose for `project`: nothing, when they chose nothing.
pub fn load(home: &Path, project: &Path) -> Result<Prefs, String> {
    let file = path(home, project);
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Prefs::default()),
        Err(e) => return Err(format!("{} does not read: {e}", file.display())),
    };
    let mut rest: toml::Table = toml::from_str(&text).map_err(|e| format!("{} is not TOML: {e}", file.display()))?;
    let ports = match rest.remove("ports") {
        Some(v) => Some(v.try_into::<Ports>().map_err(|e| format!("{}: [ports] {e}", file.display()))?),
        None => None,
    };
    rest.remove("project");
    Ok(Prefs { ports, rest })
}

/// Write `prefs` for `project`, whole or not at all: to a file beside it, then renamed over it, so two writers at
/// once (the page and the app) never leave half a file.
pub fn save(home: &Path, project: &Path, prefs: &Prefs) -> Result<(), String> {
    let file = path(home, project);
    let dir = file.parent().ok_or("the projects' folder has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let mut table = toml::Table::new();
    let real = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    // the project's path, for whoever opens the file by hand
    table.insert("project".into(), toml::Value::String(real.display().to_string()));
    if let Some(p) = &prefs.ports {
        table.insert("ports".into(), toml::Value::try_from(p).map_err(|e| e.to_string())?);
    }
    for (k, v) in &prefs.rest {
        table.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let text = toml::to_string(&table).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{}.{}", file.file_name().and_then(|n| n.to_str()).unwrap_or("prefs"), std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("cannot put {} in place: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_kept_by_the_project_s_real_path_and_other_sections_survive_a_write() {
        let base = std::env::temp_dir().join(format!("jaynes-prefs-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let (home, project) = (base.join("home"), base.join("project"));
        std::fs::create_dir_all(&project).expect("dir");
        assert_eq!(load(&home, &project).expect("none"), Prefs::default());
        // a section another part of the engine wrote stays when the ports are written
        std::fs::create_dir_all(home.join(FOLDER)).expect("dir");
        std::fs::write(path(&home, &project), "[delivery]\nsign = true\n").expect("write");
        let mut prefs = load(&home, &project).expect("read");
        prefs.ports = Some(Ports { any: true, servers: false, opened: 1_795_000_000 });
        save(&home, &project, &prefs).expect("save");
        let again = load(&home, &project).expect("read");
        assert_eq!(again.ports, Some(Ports { any: true, servers: false, opened: 1_795_000_000 }));
        assert_eq!(again.rest["delivery"]["sign"].as_bool(), Some(true));
        // the same project by another spelling of its path is the same file
        let other = base.join("home/../project");
        assert_eq!(path(&home, &other), path(&home, &project));
        let text = std::fs::read_to_string(path(&home, &project)).expect("text");
        assert!(text.contains("project = "), "{text}");
        let left: Vec<_> = std::fs::read_dir(home.join(FOLDER)).expect("dir").flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no half-written file is left beside it: {left:?}");
        let _gone = std::fs::remove_dir_all(&base);
    }
}
