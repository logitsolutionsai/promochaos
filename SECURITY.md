# Security Policy

## Supported Versions

promochaos is in early development. Security fixes go into the latest release only.

## Reporting a Vulnerability

Please do not open a public issue for a security problem. Use GitHub's private vulnerability reporting on this repository (the "Report a vulnerability" button on the Security tab of github.com/logitsolutionsai/promochaos), and include:

- the promochaos version (`promochaos --version`) and your operating system;
- what you ran, and what happened;
- a minimal settings file, catalog or function that shows the problem, if you have one.

We reply to confirm we have the report, and we keep you informed while we work on a fix. When the fix is released, we credit you in the changelog unless you ask us not to.

## What promochaos Does at Run Time

These are the boundaries promochaos is built to keep. A way around any of them is a security problem.

- **No network access.** promochaos does not open network connections while it runs. Functions run inside Wasmtime through function-runner, with no host access beyond what function-runner provides.
- **No shell.** The command target starts the configured program directly, without a shell, with a timeout and a 1 MB cap on its output.
- **Synthetic data only.** Carts are generated from your catalog file. `promochaos init` writes made-up ids. promochaos never reads a live store, places an order or touches payments.
- **Input limits.** At most 10,000 carts per run, 2,000 lines per cart and 10 MB per input file.

## Out of Scope

- Problems in the function or engine under test. Finding those is what promochaos is for.
- Problems in function-runner or Wasmtime themselves. Please report those to their maintainers; we will update the pinned version once a fix is released.
