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

The person is asked one question, "Is this the bug?", with your bug sentence and your first example under it. Write
both for the person who asked, not for a programmer. In bug, what goes wrong as they see it, in one or two
sentences. In examples, at most three, the one they know best first: each a situation they know (case), what they
see today (now) and what they will see once it is fixed (after). No code in any of them: no file names or paths, no
names from the code (none with an underscore or with parentheses), no braces or JSON. Jaynes refuses them otherwise.
