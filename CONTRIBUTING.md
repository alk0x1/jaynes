# Contributing

A pull request is welcome. Run the engine's suite before you send it (`cd engine && cargo test`), and the app's when
you change the app (`cd app && cargo test`, with Zig 0.16 as app/NOTICE.md says).

Every commit of a pull request is signed off by its author, the Developer Certificate of Origin
([developercertificate.org](https://developercertificate.org)): the line `Signed-off-by: Your Name <you@example.com>`,
which `git commit -s` adds, says you wrote the change or otherwise have the right to send it under this project's
license, Apache-2.0. A check on each pull request refuses a commit without it; `git rebase --signoff main` adds it to
commits already made.
