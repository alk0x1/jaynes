Find the bug the task describes and write the smallest test that fails because of it, next to the project's other
tests and in their style.

The task:

{{request}}

When the task lists what must hold, write a test for each item now, even one that passes today: the fix may not
touch the tests, so what you leave out here goes unproven. If the fix will change what an existing test expects,
change that test now too. Change test files only; do not fix the bug yet.

How Jaynes runs the tests: {{runner.each}} followed by test files runs only them; {{runner.suite}} runs them all.
Run your tests that way before you answer. Jaynes runs them again in a box without internet and requires them to
fail, printing the line you quote in failure.

If you find no bug a test can show, set reproduced to false, say what you tried and why in bug, and change nothing.
