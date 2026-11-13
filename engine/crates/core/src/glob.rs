//! Path patterns, as the Python engine reads them (`fnmatch`): `*` matches any run of characters, slashes
//! included, `?` one character, and a pattern that starts with `**/` also matches at the top of the project.

pub fn matches(path: &str, pattern: &str) -> bool {
    fnmatch(path.as_bytes(), pattern.as_bytes())
        || pattern.strip_prefix("**/").is_some_and(|rest| fnmatch(path.as_bytes(), rest.as_bytes()))
}

/// Whether any pattern lets `path` through: with no patterns nothing does; a pattern that starts with `!` keeps
/// the paths it matches out even when another one lets them in.
pub fn allowed(path: &str, patterns: &[String]) -> bool {
    let mut inside = false;
    for p in patterns {
        if let Some(out) = p.strip_prefix('!') {
            if matches(path, out) {
                return false;
            }
        } else if matches(path, p) {
            inside = true;
        }
    }
    inside
}

fn fnmatch(s: &[u8], p: &[u8]) -> bool {
    let (mut si, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || (p[pi] != b'*' && p[pi] == s[si])) {
            si += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_fnmatch() {
        assert!(matches("src/a.py", "**"));
        assert!(matches("src/a.py", "*.py"));
        assert!(matches("pyproject.toml", "**/pyproject.toml"));
        assert!(matches("x/pyproject.toml", "**/pyproject.toml"));
        assert!(matches(".github/workflows/ci.yml", "**/.git*"));
        assert!(!matches("src/a.py", "*.rs"));
        assert!(matches("ab", "a?"));
        assert!(!matches("a", "a?"));
    }

    #[test]
    fn exclusion_wins() {
        let p = vec!["**".to_string(), "!tests/**".to_string()];
        assert!(allowed("src/a.py", &p));
        assert!(!allowed("tests/t.py", &p));
        assert!(!allowed("a", &[]));
    }
}
