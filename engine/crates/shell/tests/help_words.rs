//! The words of `jaynes`'s help say what a command does, not its opposite. The real-user read "with PORT use one Jaynes
//! holds for databases and caches" as Jaynes holding the port; it is the other way: with PORT Jaynes lets the tests have
//! a port it holds for databases and caches (the real user's of 23/11, a "confuses"). The words are the design-lead's.

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

fn help() -> String {
    let out = std::process::Command::new(BIN).env("JAYNES_ENGINE_HOME", std::env::temp_dir().join("jaynes-help-words")).output().expect("jaynes");
    String::from_utf8_lossy(&out.stdout).replace("\n                            ", " ")
}

#[test]
fn test_servers_with_a_port_says_it_lets_the_tests_use_a_port_jaynes_otherwise_holds() {
    let said = help();
    assert!(
        said.contains("With PORT, also let them use that port, which Jaynes otherwise holds for databases and caches."),
        "the help of test-servers reads: {}",
        said.lines().filter(|l| l.contains("test-servers")).collect::<Vec<_>>().join(" | ")
    );
}
