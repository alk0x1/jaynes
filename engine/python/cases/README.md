# The cases that come with a check

A check is trusted for what it refuses. So every check comes with inputs it must reject and inputs it must accept,
and it is run on them before it judges anything real: a check of Jaynes's menu in the engine's own tests, a role's
own check by `jaynes check`.

Each check has a folder named after it, with two files:

- `reject.json`: inputs the check must fail. Each is a near miss, one small change away from an input it accepts,
  and names in `hole` the hole it closes.
- `accept.json`: inputs the check must pass. Each names in `keeps` what a check too strict would wrongly refuse.

A case is an object with `id` (its own, among the check's cases), `expect` (`reject` or `accept`, as its file
says), `hole` or `keeps`, and the check's input:

- `output`: the output it judges.
- `given`: the outputs of the steps its step uses, a list of `{"step": NAME, "output": ...}`. Left out: none.
- `project`: the folder it sees, a map of paths to text. Left out: it sees no project.
- `start`: the project as the run began, a map of paths to text, for a check that compares with it (suite-passes
  and tests-pass, on a failure the project's Makefile told make to ignore). Left out: the start is not known, and
  such a check counts all it finds as the step's.
- `with`: for a check of the menu, its parameters, those the role writes and those the engine fills in
  (`suite_step`, `cases_step`, `schema`, `write`).

A check whose input is easier to write another way reads its cases with a reader of its own, kept beside the check.

A step of the menu that judges what it is given comes with cases too, in a folder of the same kind (find-suite,
refuse-when-false): a reject is an input it must refuse, an accept one it must go on with.

A reject must fail and an accept must pass. A check that could not judge a case (it broke, there was no box to run
it in) has not rejected it: the case did not run, and that is said, never counted.

Every hole found later in a check becomes a case here. The cases a check must reject are written by someone other
than the check's author, and only they change them.
