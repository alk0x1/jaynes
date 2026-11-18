//! What only a build for tests has. A test drives the app as the person would (JAYNES_APP_TEST_DO: answers, clicks,
//! keys), draws its windows into pictures (JAYNES_APP_SHOT), keeps its notices in a file, uses a key of its own:
//! anything that reads these variables could act as the person, signed with their key. So a build for people, made
//! without the cargo feature `test-driver` (the release, `bundle.sh` unless TEST=1), reads none of them and has no
//! driver at all; `cargo build --features test-driver` and `TEST=1 ./bundle.sh` make one for tests.

/// Every variable only a test build reads.
pub const VARIABLES: &[&str] = &[
    "JAYNES_APP_TEST_DO",
    "JAYNES_APP_SHOT",
    "JAYNES_APP_SHOT_MS",
    "JAYNES_APP_TEST_NOTES",
    "JAYNES_APP_TEST_KEY",
    "JAYNES_APP_CHANGES",
    "JAYNES_APP_NOSIDE",
    "JAYNES_APP_STATE",
];

/// Whether this build is one for tests.
pub const BUILT_FOR_TESTS: bool = cfg!(feature = "test-driver");

/// A test's variable, in a build for tests; in a build for people, never.
pub fn var(name: &str) -> Option<std::ffi::OsString> {
    debug_assert!(VARIABLES.contains(&name), "{name} is not a test's variable");
    if BUILT_FOR_TESTS { std::env::var_os(name) } else { None }
}

#[cfg(test)]
mod tests {
    use super::{BUILT_FOR_TESTS, VARIABLES, var};

    #[test]
    #[cfg(not(feature = "test-driver"))]
    fn a_build_for_people_reads_no_test_variable() {
        assert!(!BUILT_FOR_TESTS);
        for name in VARIABLES {
            // SAFETY: tests in this module read and set only their own variables, and nothing else reads them here.
            unsafe { std::env::set_var(name, "/tmp/anything") };
            assert!(var(name).is_none(), "{name} is read by a build for people");
        }
    }

    #[test]
    #[cfg(feature = "test-driver")]
    fn a_build_for_tests_reads_them() {
        assert!(BUILT_FOR_TESTS);
        // SAFETY: as above.
        unsafe { std::env::set_var("JAYNES_APP_SHOT", "/tmp/a.png") };
        assert_eq!(var("JAYNES_APP_SHOT").as_deref(), Some(std::ffi::OsStr::new("/tmp/a.png")));
    }

    #[test]
    fn the_release_is_built_without_the_driver_and_a_test_bundle_with_it() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let release = std::fs::read_to_string(root.join("../.github/workflows/release.yml")).expect("release.yml");
        assert!(!release.contains("test-driver") && !release.contains("TEST="), "the release must not build the test driver");
        let bundle = std::fs::read_to_string(root.join("bundle.sh")).expect("bundle.sh");
        assert!(bundle.contains(r#"[ -n "${TEST:-}" ] && features="--features test-driver""#), "bundle.sh adds the driver only with TEST=1");
    }
}
