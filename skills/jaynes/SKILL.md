---
name: jaynes
description: Carry out an agreed code change through Jaynes, which runs Claude Code through a method one step at a time, checks each step by code and stops for the person's answer where it matters. Use whenever the person approves a plan or a change to a project's code in this conversation ("faça", "pode fazer", "go ahead", "do it", "ship it"), when they ask to run a task with Jaynes, and when they want a new Jaynes role (a method for a kind of work). Not when they say to do it without Jaynes ("sem a Jaynes", "without Jaynes"): then make the change yourself.
argument-hint: <task>
---

Carry out this change through Jaynes: $ARGUMENTS

When the skill starts from an approved plan, the task is that plan, in full, with what was agreed in conversation.

1. Run `jaynes update`. If `jaynes` is not found, or `jaynes guide` does not print how to start a run with
   `jaynes start --role`, install it: `cargo install --locked --git https://github.com/alk0x1/jaynes jaynes-shell
   --root ~/.local` (install Rust with rustup first if `cargo` is missing).
2. Run `jaynes guide` and follow what it prints.

## When the person wants a new role

Instead of running a change: after step 1 above, write the role. A role is a folder with a `role.toml`; the person
describes the method, you write it, and `jaynes check` decides.

1. `jaynes new NAME` writes a starting `role.toml` whose comments list every key and the whole menu of steps and checks.
2. Write the person's method in it: the steps in order, what each reaches, its checks from the menu, where the person
   is asked, and the promises the person wants the method to keep.
3. Run `jaynes check NAME` and fix what it reports, until every line is ✔. A promise marked ✘ comes with the path
   of a run that breaks it: change the method, or tell the person the promise does not hold and why.
4. `jaynes add NAME` installs it.

