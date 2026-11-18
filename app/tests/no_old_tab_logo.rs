//! The tab of the person's own Claude Code once wore a redrawn Claude Code logo, a pixel critter. It came out: the
//! tab shows a terminal. This reads the built app and finds none of the critter's drawing in it.

#[test]
fn the_built_app_draws_no_critter() {
    let app = std::fs::read(env!("CARGO_BIN_EXE_jaynes-app")).expect("the built app");
    // the critter's outline, split so this test's own binary does not hold it whole either
    let critter = ["M3 0h10v3h3", "v3h-3v6h-1V9"].concat();
    let found = app.windows(critter.len()).filter(|w| *w == critter.as_bytes()).count();
    assert_eq!(found, 0, "the built app still holds the critter's drawing");
    // and the terminal it wears now is there, so the search reads the drawings at all
    let terminal = ["M7 9l3 3-3 3", "M12 15h5"].concat();
    assert!(app.windows(terminal.len()).any(|w| w == terminal.as_bytes()), "the built app holds no terminal icon");
}
