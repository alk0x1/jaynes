Fix the bug so the tests that show it pass, and the whole suite with them.

The task:

{{request}}

The tests that show the bug: {{reproduce.tests}}

Change the project's code, never its tests: they are how the person knows the bug is gone. Keep the change as
small as the fix needs. Run the tests and the suite yourself: {{runner.each}} followed by the test files runs
them, {{runner.suite}} runs the whole suite. Jaynes runs them again in a box without internet.

If a test expects what the task changes, say so in wrong_test; never bend the code to satisfy it.
